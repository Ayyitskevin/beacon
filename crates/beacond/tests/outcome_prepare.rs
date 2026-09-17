//! Outcome Document slice 3-A: the immutable manifest store and `prepare` (ADR 0040, §7 slice 3).
//!
//! One test per bullet of the slice's list, plus the mutation gate's probes. The fixture chain down
//! to `admitted_mission` is `tests/artifact.rs`'s, reused verbatim: the manifest store's foreign
//! keys reach `missions`, `mission_completions` and `artifacts`, so a row cannot exist without the
//! admission chain that produces one. Copied rather than shared, because the two suites must be
//! able to disagree about what a Mission is -- a shared fixture is how two tests agree about
//! something wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use beacond::{
    AccessOrigin, AppendixMode, AppendixRequest, ArtifactContext, ArtifactDigest, ArtifactError,
    ArtifactKind, ArtifactService, AutonomyProfile, BeaconCapability, COMPLETION_CANONICALIZER,
    CheckPreflightService, ClassificationActor, ClassificationBasis, ClassificationStoreError,
    ClassificationSupersedes, CollectionRequest, Collector, CompletionActor, CompletionDisposition,
    CompletionReadRefusal, CompletionSupersedes, ConditionOutcome, ConditionResult,
    ConditionResultRecord, Conflicts, DeclaredCheck, DiagnosticStatus, Evaluation,
    EvaluationDigest, EvaluationDraft, EvaluationError, EvaluationService, EvaluationSubjectDraft,
    EvaluationSupersedes, Evaluator, Finding, FindingSeverity, ImpactLevel, InconsistentEvaluation,
    LineAttribution, LocalErrorCode, LocalModelActivationService, LocalModelEndpoint,
    LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer, LocalTransportError,
    MISSION_IMPACT_CANONICALIZER, ManifestStoreRefusal, MissionCompletionV1, MissionDraft,
    MissionError, MissionImpactClassificationV1, MissionImpactService, MissionMemberDraft,
    MissionService, NarrativeItem, OperatorAccountError, OperatorAccountService,
    OperatorSessionPhase, OutcomeCandidate, OutcomeManifestV1, OutcomeService, OutcomeState,
    PolicyRevisionRef, PredecessorFault, PrepareRefusal, ProjectCharterDraft, ProjectDraft,
    ProjectError, ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService,
    ProjectService, Provenance, ProviderFeature, PublicationSeamRefusal, PublishRefusal,
    RepositoryDraft, RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService,
    RubricDeclaration, SecretString, SourceDigest, SourceKind, SourceRef, SourcesRefusal, SpendEra,
    SpendReference, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    SuccessCondition, Supervisor, SupervisorError, SupportRef, TeamDraft, TeamLeadAmbiguityRule,
    TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft,
    TeamService, TrackRecordEvidenceKind, TrackRecordRequirement, UnfinishedKind, Verdict,
    VerifiedSources, VerifiedText, declared_conditions_digest, initialize_local_auth_token,
    publication_candidacy, record_completion, render, verified_manifest_within,
};
use data_encoding::BASE64;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

const CONDITION_TEXT: &str = "Relevant gates pass";
const CONDITION_ID: &str = "sc-gates";

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-outcome-prepare-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
        }
    }

    fn connect(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

struct Fixture {
    store: TestStore,
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    other_resident_id: Uuid,
    other_configuration_id: Uuid,
    policy_revision: u32,
    charter_revision: u32,
    outcomes: OutcomeService,
}

fn tags_response() -> String {
    "{\"models\":[{\"name\":\"qwen3.8:27b\",\"model\":\"qwen3.8:27b\",\
     \"modified_at\":\"2026-08-14T14:38:20.13373148-04:00\",\"size\":17741872154,\
     \"digest\":\"22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643\",\
     \"details\":{\"format\":\"gguf\",\"family\":\"qwen35\",\"families\":[\"qwen35\"],\
     \"parameter_size\":\"27.3B\",\"quantization_level\":\"Q4_K_M\"}}]}"
        .to_owned()
}

fn read_http_request(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let size = stream.read(&mut chunk).unwrap();
        assert!(size > 0);
        request.extend_from_slice(&chunk[..size]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            return String::from_utf8(request).unwrap();
        }
    }
}

fn serve_ollama_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..count {
            for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(request.starts_with(expected), "unexpected: {request}");
                let body = if expected.starts_with("POST") {
                    "{\"capabilities\":[\"completion\",\"tools\"]}".to_owned()
                } else {
                    tags_response()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    (address, handle)
}

/// A Workspace with one Project, one published Policy revision, and one admitted Mission whose
/// roster holds two Residents at their exact admission Configurations.
fn admitted_mission(label: &str) -> Fixture {
    admitted_mission_with_members(label, 2)
}

fn admitted_mission_with_members(label: &str, member_count: usize) -> Fixture {
    admitted_mission_with_reviewer_capabilities(
        label,
        member_count,
        &[
            BeaconCapability::WorkCreateMission,
            BeaconCapability::WorkCoordinateMission,
        ],
    )
}

fn admitted_mission_with_reviewer_capabilities(
    label: &str,
    member_count: usize,
    reviewer_capabilities: &[BeaconCapability],
) -> Fixture {
    let store = TestStore::new(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let workspace_id = report.workspace.id;
    let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
    accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert_eq!(
        accounts.complete_operator_setup(true).unwrap().phase,
        OperatorSessionPhase::Unlocked
    );

    let admitted = admit_mission_in_with_reviewer_capabilities(
        &accounts,
        workspace_id,
        member_count,
        reviewer_capabilities,
    );
    Fixture {
        outcomes: OutcomeService::new(Arc::clone(&accounts), workspace_id),
        store,
        accounts,
        workspace_id,
        project_id: admitted.project_id,
        mission_id: admitted.mission_id,
        resident_id: admitted.resident_id,
        configuration_id: admitted.configuration_id,
        other_resident_id: admitted.other_resident_id,
        other_configuration_id: admitted.other_configuration_id,
        policy_revision: admitted.policy_revision,
        charter_revision: admitted.charter_revision,
    }
}

/// One Workspace's Project, Policy, Residents, Team and admitted Mission.
///
/// Split out of `admitted_mission` so a test can build a SECOND Workspace in the SAME store: the
/// block needs nothing but an authenticated account service and a Workspace id, and the startup
/// pass is about what happens across Workspaces.
struct AdmittedMission {
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    other_resident_id: Uuid,
    other_configuration_id: Uuid,
    policy_revision: u32,
    charter_revision: u32,
}

fn admit_mission_in(accounts: &Arc<OperatorAccountService>, workspace_id: Uuid) -> AdmittedMission {
    admit_mission_in_with_members(accounts, workspace_id, 2)
}

fn admit_mission_in_with_members(
    accounts: &Arc<OperatorAccountService>,
    workspace_id: Uuid,
    member_count: usize,
) -> AdmittedMission {
    admit_mission_in_with_reviewer_capabilities(
        accounts,
        workspace_id,
        member_count,
        &[
            BeaconCapability::WorkCreateMission,
            BeaconCapability::WorkCoordinateMission,
        ],
    )
}

fn admit_mission_in_with_reviewer_capabilities(
    accounts: &Arc<OperatorAccountService>,
    workspace_id: Uuid,
    member_count: usize,
    reviewer_capabilities: &[BeaconCapability],
) -> AdmittedMission {
    assert!((1..=2).contains(&member_count));
    let accounts = Arc::clone(accounts);
    let project = ProjectService::new(Arc::clone(&accounts), workspace_id)
        .create(ProjectDraft {
            name: "Beacon Alpha".to_owned(),
            charter: ProjectCharterDraft {
                purpose: "Prove the local agent office.".to_owned(),
                desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        })
        .unwrap();
    let policy = ProjectPolicyService::new(Arc::clone(&accounts), workspace_id)
        .publish(
            project.id,
            project.charter.revision,
            None,
            ProjectPolicyDraft {
                team_lead_selection: TeamLeadSelectionPolicy {
                    required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                    availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                    track_record_requirements: vec![TrackRecordRequirement {
                        evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                        minimum_count: 1,
                    }],
                    conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                    ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                },
            },
        )
        .unwrap();

    let residents = StarterOrganizationService::new(Arc::clone(&accounts), workspace_id)
        .accept(
            1,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: None,
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: None,
                },
            ],
        )
        .unwrap()
        .residents;
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), workspace_id);
    for resident in &residents {
        lifecycle.activate(resident.id, None).unwrap();
    }

    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(&accounts),
        workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models
        .inspect(
            "qwen3.8:27b",
            "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
        )
        .unwrap();
    let activation = models
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();

    let configurations = residents
        .iter()
        .enumerate()
        .map(|(index, resident)| {
            ResidentConfigurationService::new(Arc::clone(&accounts), workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    if index == 1 {
                        reviewer_capabilities.to_vec()
                    } else {
                        vec![
                            BeaconCapability::WorkCreateMission,
                            BeaconCapability::WorkCoordinateMission,
                        ]
                    },
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let team = TeamService::new(Arc::clone(&accounts), workspace_id)
        .form(TeamDraft {
            project_id: project.id,
            name: "Mission Team".to_owned(),
            members: configurations
                .iter()
                .take(member_count)
                .map(|configuration| TeamMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                })
                .collect(),
        })
        .unwrap();
    let mission = MissionService::new(Arc::clone(&accounts), workspace_id)
        .admit(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: team.id,
            desired_outcome: "Ship one reviewed Beacon feature.".to_owned(),
            success_conditions: vec![CONDITION_TEXT.to_owned()],
            constraints: vec!["Keep the change reversible".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: configurations
                .iter()
                .take(member_count)
                .map(|configuration| MissionMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                })
                .collect(),
        })
        .unwrap();

    AdmittedMission {
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configurations[0].resident_id,
        configuration_id: configurations[0].id,
        other_resident_id: configurations[1].resident_id,
        other_configuration_id: configurations[1].id,
        policy_revision: policy.revision,
        charter_revision: project.charter.revision,
    }
}

impl Fixture {
    fn latest_event_seq(&self) -> u64 {
        self.store
            .connect()
            .query_row(
                "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map(|sequence| u64::try_from(sequence).unwrap())
            .unwrap()
    }

    /// One recorded completion, accountable to `resident_id` at its admission Configuration.
    fn complete(&self, accountable: (Uuid, Uuid)) -> (String, MissionCompletionV1) {
        self.complete_with(accountable, Vec::new())
    }

    /// One recorded completion carrying the given unfinished-work items -- text the document must
    /// read back through the completion's own record, never through a collected projection.
    fn complete_with(
        &self,
        accountable: (Uuid, Uuid),
        unfinished_work: Vec<NarrativeItem<UnfinishedKind>>,
    ) -> (String, MissionCompletionV1) {
        let boundary = self.latest_event_seq();
        let record = MissionCompletionV1 {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            completion_id: Uuid::new_v4(),
            actor: CompletionActor::Operator,
            accountable_resident_id: accountable.0,
            accountable_resident_configuration_id: accountable.1,
            boundary_event_seq: boundary,
            disposition: CompletionDisposition::Concluded,
            declared_conditions_sha256: declared_conditions_digest(&[CONDITION_TEXT.to_owned()]),
            condition_results: vec![ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::Met,
                support: vec![],
                note: None,
            }],
            delivered_artifacts: vec![],
            unfinished_work,
            uncertainty: vec![],
            spend: SpendReference::NotMeasured(
                "No spend record exists for this Mission.".to_owned(),
            ),
            supersedes: CompletionSupersedes::None,
            summary: "One reviewed feature shipped.".to_owned(),
            completed_at_ms: 1_788_400_000_000,
        };
        let mut connection = self.store.connect();
        let digest = record_completion(&mut connection, &record).expect("the completion records");
        (digest, record)
    }

    /// One recorded Mission impact classification at the requested impact level.
    fn classify(&self, impact: ImpactLevel) -> (Uuid, u32, String) {
        self.classify_version(impact, 1)
    }

    /// A classification at an explicit version -- a Mission classified twice has a version 2, and
    /// which one an Outcome cites depends on where the completion's boundary falls (R23.4).
    fn classify_version(&self, impact: ImpactLevel, version: u32) -> (Uuid, u32, String) {
        let classification = MissionImpactClassificationV1 {
            workspace_id: self.workspace_id,
            mission_id: self.mission_id,
            classification_id: Uuid::new_v4(),
            version,
            impact,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: self.project_id,
                policy_revision: self.policy_revision,
                charter_revision: self.charter_revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes: ClassificationSupersedes::None,
        };
        let bytes = classification.canonical_bytes().unwrap();
        let recorded = MissionImpactService::new(Arc::clone(&self.accounts), self.workspace_id)
            .classify(&bytes)
            .expect("the classification records");
        (
            classification.classification_id,
            classification.version,
            recorded.digest().value.clone(),
        )
    }

    fn evaluations(&self) -> EvaluationService {
        EvaluationService::new(Arc::clone(&self.accounts), self.workspace_id)
    }

    /// One Evaluation of the given candidate, by the OTHER Resident -- the independent evaluator
    /// `publish` requires. `verdict` is a parameter because "not `meets`" is several answers.
    fn evaluate(&self, candidate_digest: &str, verdict: Verdict) -> Evaluation {
        self.evaluate_as(
            candidate_digest,
            verdict,
            (self.other_resident_id, self.other_configuration_id),
        )
    }

    fn evaluate_as(
        &self,
        candidate_digest: &str,
        verdict: Verdict,
        evaluator: (Uuid, Uuid),
    ) -> Evaluation {
        self.evaluations()
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Artifact(candidate_digest.to_owned()),
                evaluator: Evaluator {
                    resident_id: evaluator.0,
                    configuration_id: evaluator.1,
                },
                rubric: RubricDeclaration {
                    id: "outcome-document-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: data_encoding::HEXLOWER.encode(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict,
                findings: vec![Finding {
                    severity: FindingSeverity::Note,
                    summary: "Read the exact candidate bytes.".to_owned(),
                }],
                conflicts: Conflicts::None,
                rationale: "An independent Resident read the exact candidate bytes.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation records")
    }

    /// A completion whose one declared condition cites real supporting objects.
    ///
    /// The support is recorded in the completion's own bytes as `SupportRef`s, which is the only
    /// way it can be: the collector reads each one through its owning verified reader at the
    /// boundary, so a reference the completion did not record is not a reference at all.
    fn complete_with_support(
        &self,
        accountable: (Uuid, Uuid),
        support: Vec<SupportRef>,
    ) -> (String, MissionCompletionV1) {
        let boundary = self.latest_event_seq();
        let record = MissionCompletionV1 {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            completion_id: Uuid::new_v4(),
            actor: CompletionActor::Operator,
            accountable_resident_id: accountable.0,
            accountable_resident_configuration_id: accountable.1,
            boundary_event_seq: boundary,
            disposition: CompletionDisposition::Concluded,
            declared_conditions_sha256: declared_conditions_digest(&[CONDITION_TEXT.to_owned()]),
            condition_results: vec![ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::Met,
                support,
                note: None,
            }],
            delivered_artifacts: vec![],
            unfinished_work: vec![],
            uncertainty: vec![],
            spend: SpendReference::NotMeasured(
                "No spend record exists for this Mission.".to_owned(),
            ),
            supersedes: CompletionSupersedes::None,
            summary: "One reviewed feature shipped, with its evidence cited.".to_owned(),
            completed_at_ms: 1_788_400_000_000,
        };
        let mut connection = self.store.connect();
        let digest = record_completion(&mut connection, &record).expect("the completion records");
        (digest, record)
    }

    /// A completion that is NOT a success: abandoned, with its one declared condition not met.
    ///
    /// A Mission can complete without succeeding, and an Outcome Document reports that honestly --
    /// which is exactly why publishing one must not read as a success verdict.
    fn complete_unsuccessfully(&self, accountable: (Uuid, Uuid)) -> (String, MissionCompletionV1) {
        let boundary = self.latest_event_seq();
        let record = MissionCompletionV1 {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            completion_id: Uuid::new_v4(),
            actor: CompletionActor::Operator,
            accountable_resident_id: accountable.0,
            accountable_resident_configuration_id: accountable.1,
            boundary_event_seq: boundary,
            disposition: CompletionDisposition::Abandoned,
            declared_conditions_sha256: declared_conditions_digest(&[CONDITION_TEXT.to_owned()]),
            condition_results: vec![ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::NotMet,
                support: vec![],
                note: None,
            }],
            delivered_artifacts: vec![],
            unfinished_work: vec![],
            uncertainty: vec![],
            spend: SpendReference::NotMeasured(
                "No spend record exists for this Mission.".to_owned(),
            ),
            supersedes: CompletionSupersedes::None,
            summary: "The Mission was stopped before its condition was met.".to_owned(),
            completed_at_ms: 1_788_400_000_000,
        };
        let mut connection = self.store.connect();
        let digest = record_completion(&mut connection, &record).expect("the completion records");
        (digest, record)
    }

    /// A SECOND Configuration for the candidate's author, succeeding the one the completion names.
    ///
    /// Two Configurations of one Resident are two evaluators for configuration purposes and one
    /// author for non-authorship purposes; this is how a test reaches the second half of that.
    fn second_configuration_for_author(&self) -> Uuid {
        let configurations =
            ResidentConfigurationService::new(Arc::clone(&self.accounts), self.workspace_id);
        let current = configurations
            .list()
            .expect("configurations list")
            .into_iter()
            .find(|configuration| configuration.id == self.configuration_id)
            .expect("the author's configuration is listed");
        let activation_id = current
            .binding
            .as_ref()
            .expect("a configured Resident carries its binding")
            .activation_id;
        configurations
            .configure(
                self.resident_id,
                Some(current.id),
                activation_id,
                vec![
                    BeaconCapability::WorkCreateMission,
                    BeaconCapability::WorkCoordinateMission,
                ],
            )
            .expect("the author is reconfigured")
            .id
    }

    /// The draft `evaluate_as` records, exposed so a test can assert the RECORD path's refusal.
    fn artifact_evaluation_draft(
        &self,
        candidate_digest: &str,
        verdict: Verdict,
        evaluator: (Uuid, Uuid),
    ) -> EvaluationDraft {
        EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(candidate_digest.to_owned()),
            evaluator: Evaluator {
                resident_id: evaluator.0,
                configuration_id: evaluator.1,
            },
            rubric: RubricDeclaration {
                id: "outcome-document-v1".to_owned(),
                version: 1,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: data_encoding::HEXLOWER.encode(&[0x20_u8; 32]),
            },
            cited_runs: Vec::new(),
            verdict,
            findings: vec![Finding {
                severity: FindingSeverity::Note,
                summary: "Read the exact candidate bytes.".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "An independent Resident read the exact candidate bytes.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        }
    }

    /// A prepared candidate plus its qualifying Evaluation, when the caller already prepared.
    fn publishable_from(
        &self,
        candidate: OutcomeCandidate,
    ) -> (OutcomeCandidate, ArtifactDigest, EvaluationDigest) {
        let record = self
            .artifacts()
            .get(&candidate.candidate_artifact_digest)
            .expect("the candidate reads");
        let evaluation = self.evaluate(&candidate.candidate_artifact_digest, Verdict::Meets);
        let digest = evaluation.digest().clone();
        (candidate, record.digest().clone(), digest)
    }

    /// A completed, classified Mission with a prepared candidate and a `meets` Evaluation of it:
    /// everything `publish` needs, and nothing it is being tested on.
    fn publishable(&self) -> (OutcomeCandidate, ArtifactDigest, EvaluationDigest) {
        let candidate = self
            .outcomes
            .prepare_for_mission(self.mission_id, AppendixRequest::PolicyDefault)
            .expect("the Mission prepares");
        let record = self
            .artifacts()
            .get(&candidate.candidate_artifact_digest)
            .expect("the candidate reads");
        let evaluation = self.evaluate(&candidate.candidate_artifact_digest, Verdict::Meets);
        let evaluation_digest = evaluation.digest().clone();
        (candidate, record.digest().clone(), evaluation_digest)
    }

    /// A classification at an explicit version that supersedes an explicit predecessor, recorded
    /// through the Command like any other.
    fn classify_superseding(&self, version: u32, supersedes: ClassificationSupersedes) -> String {
        let classification = MissionImpactClassificationV1 {
            workspace_id: self.workspace_id,
            mission_id: self.mission_id,
            classification_id: Uuid::new_v4(),
            version,
            impact: ImpactLevel::Ordinary,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: self.project_id,
                policy_revision: self.policy_revision,
                charter_revision: self.charter_revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes,
        };
        MissionImpactService::new(Arc::clone(&self.accounts), self.workspace_id)
            .classify(&classification.canonical_bytes().unwrap())
            .expect("the classification records")
            .digest()
            .value
            .clone()
    }

    /// One classification written PAST the Command with foreign keys off -- the shape a restore, an
    /// older binary or a hand-run INSERT produces. Every projection column agrees with its bytes,
    /// so nothing but a reader can refuse it.
    fn plant_classification(&self, record: &MissionImpactClassificationV1) -> String {
        let bytes = record.canonical_bytes().unwrap();
        let digest = record.digest().unwrap().value;
        let connection = self.store.connect();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute(
                "INSERT INTO mission_impact_classifications (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, classification_id, mission_id, project_id, version, impact,
                     basis, policy_revision, charter_revision, classified_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                           'operator_declared', ?9, ?10, ?11)",
                params![
                    self.workspace_id.to_string(),
                    digest,
                    bytes,
                    record.classification_id.to_string(),
                    record.mission_id.to_string(),
                    record.policy.project_id.to_string(),
                    record.version,
                    record.impact.as_str(),
                    record.policy.policy_revision,
                    record.policy.charter_revision,
                    i64::try_from(record.classified_at_ms).unwrap(),
                ],
            )
            .expect("the row is storable: every projection agrees with its bytes");
        digest
    }

    /// A planted classification **with the Event it is owed** (R58.3).
    ///
    /// `plant_classification` above is unchanged and still plants a row alone: that is the shape
    /// every corruption fixture in this file depends on, and turning it valid globally would quietly
    /// repair stores those tests exist to break.
    ///
    /// This wrapper is for the two bars whose subject is a SUPPORTED store -- a fork that must
    /// reopen Ready, and concurrent supervisors that must not be refused. Under the owner relation a
    /// stored classification must be accounted for by exactly one coherent Event, so a supported
    /// store has to have one. The payload is exactly what `MissionImpactService::classify` writes,
    /// and `occurred_at_ms` is the record's own `classified_at_ms` because the owner compares those
    /// two.
    ///
    /// ERA AND CHAIN COHERENCE, stated rather than assumed: `append_event` is crate-internal, so a
    /// test cannot mint a linked row. The Event is planted `unchained` and the store then says so --
    /// `genesis_kind = 'migrated'` with the boundary moved to this row -- the same declaration
    /// `astra_control_a_candidate_event_in_another_workspace_refuses_startup` makes at :2932. An
    /// `unchained` row at or below the boundary is a pre-migration Event the chain makes no byte
    /// claim about; one above it is `UnchainedAfterBoundary`, which is why the boundary moves with
    /// the plant. Rows already chained stay chained and still verify against the stored head.
    fn plant_supported_classification(&self, record: &MissionImpactClassificationV1) -> String {
        let digest = self.plant_classification(record);
        let bytes = record.canonical_bytes().unwrap();
        let connection = self.store.connect();
        connection
            .execute(
                "INSERT INTO events (
                     workspace_id, kind, payload_json, occurred_at_ms,
                     predecessor_kind, predecessor_digest, link_digest
                 ) VALUES (?1, 'mission.impact_classified', ?2, ?3, 'unchained', NULL, NULL)",
                params![
                    self.workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": self.workspace_id.to_string(),
                        "classification_id": record.classification_id.to_string(),
                        "mission_id": record.mission_id.to_string(),
                        "project_id": record.policy.project_id.to_string(),
                        "version": record.version,
                        "impact": record.impact.as_str(),
                        "basis": "operator_declared",
                        "policy_revision": record.policy.policy_revision,
                        "charter_revision": record.policy.charter_revision,
                        "digest": {
                            "algorithm": "sha256",
                            "canonicalizer": "beacon.mission-impact.v1",
                            "value": digest,
                        },
                        "canonical_bytes_base64": BASE64.encode(&bytes),
                        "classified_at_ms": record.classified_at_ms,
                    })
                    .to_string(),
                    i64::try_from(record.classified_at_ms).unwrap(),
                ],
            )
            .expect("the Event this classification is owed plants beside it");
        connection
            .execute_batch(
                "UPDATE event_chain_state
                    SET genesis_kind = 'migrated',
                        migrated_through_sequence = (SELECT max(sequence) FROM events)
                  WHERE id = 1;",
            )
            .expect("the store declares the era the plant put it in");
        digest
    }

    /// A second Mission in the SAME Workspace, Project and Team, reusing the admitted roster.
    ///
    /// `admit_mission_in` cannot be called twice for one Workspace -- it admits the starter
    /// organization, and a Resident's initial Configuration decision is recorded once
    /// (`DecisionAlreadyRecorded`). What a second Mission needs is a second `MissionService::admit`
    /// over the roster that already exists.
    fn admit_second_mission(&self, desired_outcome: &str) -> Fixture {
        let team_id: Uuid = self
            .store
            .connect()
            .query_row(
                "SELECT id FROM teams WHERE workspace_id = ?1 AND project_id = ?2 LIMIT 1",
                params![self.workspace_id.to_string(), self.project_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .map(|id| Uuid::parse_str(&id).unwrap())
            .expect("the fixture's Team exists");
        let members = vec![
            MissionMemberDraft {
                resident_id: self.resident_id,
                expected_configuration_id: self.configuration_id,
            },
            MissionMemberDraft {
                resident_id: self.other_resident_id,
                expected_configuration_id: self.other_configuration_id,
            },
        ];
        let mission = MissionService::new(Arc::clone(&self.accounts), self.workspace_id)
            .admit(MissionDraft {
                project_id: self.project_id,
                expected_charter_revision: self.charter_revision,
                team_id,
                desired_outcome: desired_outcome.to_owned(),
                success_conditions: vec![CONDITION_TEXT.to_owned()],
                constraints: vec!["Keep the change reversible".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                members,
            })
            .expect("a second Mission is admitted in the same Workspace");
        Fixture {
            outcomes: OutcomeService::new(Arc::clone(&self.accounts), self.workspace_id),
            store: TestStore {
                path: self.store.path.clone(),
            },
            accounts: Arc::clone(&self.accounts),
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: mission.id,
            resident_id: self.resident_id,
            configuration_id: self.configuration_id,
            other_resident_id: self.other_resident_id,
            other_configuration_id: self.other_configuration_id,
            policy_revision: self.policy_revision,
            charter_revision: self.charter_revision,
        }
    }

    fn artifacts(&self) -> ArtifactService {
        ArtifactService::new(Arc::clone(&self.accounts), self.workspace_id)
    }

    fn restart(&self) {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .expect("the store restarts");
    }
}

/// One hex record digest as the raw 32 bytes `ArtifactService::record` takes for `supersedes`.
fn digest_bytes(digest: &str) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    data_encoding::HEXLOWER
        .decode_mut(digest.as_bytes(), &mut bytes)
        .expect("a record digest is 32 hex-encoded bytes");
    bytes
}

fn sol1_admit_other_mission(fixture: &Fixture, desired_outcome: &str) -> Uuid {
    let team_id: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT team_id FROM missions WHERE id = ?1",
            params![fixture.mission_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .admit(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: fixture.charter_revision,
            team_id: Uuid::parse_str(&team_id).unwrap(),
            desired_outcome: desired_outcome.to_owned(),
            success_conditions: vec![CONDITION_TEXT.to_owned()],
            constraints: vec!["Keep changes reversible".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: vec![
                MissionMemberDraft {
                    resident_id: fixture.resident_id,
                    expected_configuration_id: fixture.configuration_id,
                },
                MissionMemberDraft {
                    resident_id: fixture.other_resident_id,
                    expected_configuration_id: fixture.other_configuration_id,
                },
            ],
        })
        .expect("the second Mission admits")
        .id
}

fn source(
    kind: SourceKind,
    object_id: Uuid,
    canonicalizer: &str,
    digest_value: &str,
    schema_version: u32,
    workspace_id: Uuid,
) -> SourceRef {
    SourceRef {
        workspace_id,
        kind,
        // R18.5: present for the kinds whose records carry one (Completion, Policy, Spend), and
        // never minted for the rest.
        identity: matches!(
            kind,
            SourceKind::Completion | SourceKind::Policy | SourceKind::Spend
        )
        .then_some(beacond::ReferenceIdentity {
            object_id,
            schema_version,
        }),
        digest: SourceDigest {
            algorithm: "sha256".to_owned(),
            canonicalizer: canonicalizer.to_owned(),
            value: digest_value.to_owned(),
        },
        pinned_revision: OutcomeState::Absent(
            "this source type pins no content revision.".to_owned(),
        ),
    }
}

/// A manifest frozen on one recorded completion and one recorded classification, plus the verified
/// values its references name.
fn manifest_for(
    fixture: &Fixture,
    completion_digest: &str,
    completion: &MissionCompletionV1,
    classification: &(Uuid, u32, String),
    policy_high_impact: bool,
    appendix_request: AppendixRequest,
) -> (OutcomeManifestV1, VerifiedSources) {
    let manifest = OutcomeManifestV1 {
        workspace_id: fixture.workspace_id,
        // By identity (R18.4): a Mission has no digest to cite; the completion is the anchor.
        mission_id: fixture.mission_id,
        completion: source(
            SourceKind::Completion,
            completion.completion_id,
            COMPLETION_CANONICALIZER,
            completion_digest,
            1,
            fixture.workspace_id,
        ),
        policy_classification: source(
            SourceKind::Policy,
            classification.0,
            MISSION_IMPACT_CANONICALIZER,
            &classification.2,
            classification.1,
            fixture.workspace_id,
        ),
        boundary_event_seq: completion.boundary_event_seq,
        completion_recorded_at_ms: completion.completed_at_ms,
        accountable_resident_id: completion.accountable_resident_id,
        policy_high_impact,
        appendix_request,
        canonicalizer_version: "beacon.outcome.manifest.v2".to_owned(),
        renderer_version: "beacon.outcome.md.v1".to_owned(),
        success_conditions: vec![SuccessCondition {
            ordinal: 0,
            condition_id: CONDITION_ID.to_owned(),
            outcome: OutcomeState::Verified(ConditionResult::Met),
            citations: OutcomeState::Absent(
                "the completion recorded no supporting object for this condition.".to_owned(),
            ),
        }],
        delivered_artifacts: OutcomeState::Absent(
            "the completion delivered no Artifacts.".to_owned(),
        ),
        proposal_refs: OutcomeState::Absent("no Proposal is cited at this boundary.".to_owned()),
        ruling_refs: OutcomeState::Absent("no Ruling is cited at this boundary.".to_owned()),
        evaluation_refs: OutcomeState::NotEvaluated(
            "no Evaluation names this Mission at the boundary event.".to_owned(),
        ),
        receipt_refs: OutcomeState::Absent("no Receipt is cited at this boundary.".to_owned()),
        spend_ref: OutcomeState::NotEvaluated(
            "No spend record exists for this Mission.".to_owned(),
        ),
        unfinished_work: OutcomeState::Absent(
            "the completion recorded no unfinished work.".to_owned(),
        ),
        uncertainty: OutcomeState::Absent("the completion recorded no uncertainty.".to_owned()),
    };
    let sources = VerifiedSources {
        mission_title: "Ship one reviewed Beacon feature.".to_owned(),
        condition_statements: vec![VerifiedText {
            id: CONDITION_ID.to_owned(),
            text: CONDITION_TEXT.to_owned(),
        }],
        artifacts: Vec::new(),
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        verified_source_bytes: 4_096,
    };
    (manifest, sources)
}

/// The default arrangement: one completion, one ordinary classification, `policy_default`.
fn ordinary(label: &str) -> (Fixture, OutcomeManifestV1, VerifiedSources) {
    let fixture = admitted_mission(label);
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    (fixture, manifest, sources)
}

/// Runs `tamper` with an immutability trigger down, then restores it -- leaving the schema
/// signature intact and the store holding a row nothing should have written.
fn with_trigger_down(
    store: &TestStore,
    name: &str,
    table: &str,
    message: &str,
    tamper: impl FnOnce(&Connection),
) {
    let connection = store.connect();
    connection
        .execute_batch(&format!("DROP TRIGGER {name};"))
        .unwrap();
    tamper(&connection);
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER {name}
BEFORE UPDATE ON {table}
BEGIN
    SELECT RAISE(ABORT, '{message}');
END;"
        ))
        .unwrap();
}

// ---------------------------------------------------------------------------------------------
// The migration
// ---------------------------------------------------------------------------------------------

#[test]
fn the_ladder_produces_an_outcome_manifest_store() {
    let store = TestStore::new("ladder");
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let connection = store.connect();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .expect("the outcome manifest store does not exist after the ladder");
    assert_eq!(count, 0);
}

#[test]
fn stored_manifests_are_immutable_under_update_and_delete() {
    let (fixture, manifest, sources) = ordinary("immutable");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let connection = fixture.store.connect();
    for statement in [
        "UPDATE outcome_manifests SET renderer_version = 'beacon.outcome.md.v2'",
        "DELETE FROM outcome_manifests",
    ] {
        let error = connection.execute(statement, []).unwrap_err().to_string();
        assert!(
            error.contains("Outcome manifests are immutable"),
            "{statement} was not refused: {error}"
        );
    }
    // And the record still reads back as itself.
    fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the manifest still reads");
}

/// Migration 0031: the publication relation exists, is STRICT, is append-only, and carries nothing
/// renderable.
///
/// Asserted against a live migrated store rather than against the migration text: a table nobody
/// created and a trigger nobody registered are indistinguishable from a comment. That the triggers
/// FIRE, and that versions stay contiguous, are proved through the real publish path below.
#[test]
fn migration_0031_makes_the_publication_relation_append_only_and_scoped() {
    let fixture = admitted_mission("publication-relation-shape");
    let connection = fixture.store.connect();

    // 0031 HAS BEEN APPLIED -- not "the ladder ends at 31". The two were the same number when
    // this test was written and stopped being the same the moment a later migration landed, which
    // red-gated CF-1 for a reason that had nothing to do with the publication relation. A test
    // named for one migration should fail when THAT migration is broken, so this asserts the floor
    // its subject needs; the seven pins in lib.rs are where the ladder head is pinned on purpose.
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        version >= 31,
        "this store is at schema {version}, before 0031: the publication relation it asserts on \
         was never created"
    );

    let ddl: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'outcome_publications'",
            [],
            |row| row.get(0),
        )
        .expect("the relation exists");
    for column in [
        "workspace_id",
        "mission_id",
        "publication_id",
        "candidate_artifact_digest",
        "evaluation_digest",
        "publication_version",
    ] {
        assert!(ddl.contains(column), "{column} is missing from {ddl}");
    }
    assert!(ddl.contains("STRICT"), "{ddl}");
    // Nothing renderable (R4.3): the row is a relation between identities, and a surface that wants
    // a title, a verdict or a time reads it through the owning record.
    for forbidden in ["title", "verdict", "summary", "rationale", "published_at"] {
        assert!(
            !ddl.contains(forbidden),
            "the relation carries a renderable column `{forbidden}`: {ddl}"
        );
    }
    assert!(ddl.contains("publication_version >= 1"), "{ddl}");

    let triggers: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'trigger' AND tbl_name = 'outcome_publications'",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        triggers.contains(&"outcome_publications_no_update".to_owned()),
        "{triggers:?}"
    );
    assert!(
        triggers.contains(&"outcome_publications_no_delete".to_owned()),
        "{triggers:?}"
    );
}

#[test]
fn migration_0030_rebuilds_outcome_manifests_from_0029s_ddl_with_exactly_one_line_changed() {
    // R22's promise, made executable: 0030 is 0029's table definition with the canonicalizer CHECK
    // repointed and nothing else. A rebuild that quietly dropped a column CHECK, a foreign key or a
    // constraint would pass every behavioural test that does not happen to exercise it; this diff
    // catches it at the DDL.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let m29 = std::fs::read_to_string(dir.join("0029_outcome_manifests.sql")).unwrap();
    let m30 =
        std::fs::read_to_string(dir.join("0030_outcome_manifest_canonicalizer_v2.sql")).unwrap();
    let table = |source: &str, name: &str| -> Vec<String> {
        let start = source
            .find(&format!("CREATE TABLE {name} ("))
            .unwrap_or_else(|| panic!("no CREATE TABLE {name}"));
        let end = source[start..].find(") STRICT;").expect("table end") + start;
        source[start..end]
            .lines()
            .skip(1)
            .map(|line| line.trim_end().to_owned())
            .collect()
    };
    let old = table(&m29, "outcome_manifests");
    let new = table(&m30, "outcome_manifests_v30");
    assert_eq!(
        old.len(),
        new.len(),
        "0030's table has a different number of lines than 0029's"
    );
    let differing: Vec<(&String, &String)> =
        old.iter().zip(new.iter()).filter(|(a, b)| a != b).collect();
    assert_eq!(
        differing,
        vec![(
            &"        CHECK (digest_canonicalizer = 'beacon.outcome.manifest.v1'),".to_owned(),
            &"        CHECK (digest_canonicalizer = 'beacon.outcome.manifest.v2'),".to_owned(),
        )],
        "0030 changed something other than the canonicalizer CHECK"
    );
    // And the objects come back under their 0029 names, so the three hand-rolled rollback helpers
    // and every reader keep working unchanged.
    for name in [
        "CREATE INDEX outcome_manifests_by_mission",
        "CREATE TRIGGER outcome_manifests_no_update",
        "CREATE TRIGGER outcome_manifests_no_delete",
        "ALTER TABLE outcome_manifests_v30 RENAME TO outcome_manifests;",
    ] {
        assert!(m30.contains(name), "0030 is missing: {name}");
    }
}

#[test]
fn a_v2_manifest_row_is_refused_by_a_store_at_0029_and_admitted_after_the_ladder() {
    // R22. Migration 0029 merged with `digest_canonicalizer` pinned to the v1 token, and 0029 is
    // applied history now -- immutable. The v2 grammar (R18.3-5) therefore needs a migration of its
    // own to repoint that CHECK, or every row `prepare` writes is refused by the store's own guard.
    //
    // RED: a store migrated through 0029 ONLY (the files, applied by hand, exactly as
    // `a_store_at_the_previous_version_migrates_forward_and_reconciles` does) refuses a v2 row,
    // and so does the same store after startup while the ladder still ends at 0029. GREEN once the
    // ladder carries 0030 and startup applies it.
    let store = TestStore::new("v2-row-at-0029");
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<String> = std::fs::read_dir(&migrations_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".sql") && &name[..4] <= "0029")
        .collect();
    files.sort();
    assert_eq!(files.last().map(|name| &name[..4]), Some("0029"));
    {
        let connection = store.connect();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        for name in &files {
            connection
                .execute_batch(&std::fs::read_to_string(migrations_dir.join(name)).unwrap())
                .unwrap();
        }
    }
    // Foreign keys stay OFF for the probe: the column under test is the CHECK, not the relations,
    // and a real parent chain would only move the failure somewhere else.
    let insert = |connection: &Connection| {
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection.execute(
            "INSERT INTO outcome_manifests (
                 workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                 canonical_bytes, manifest_id, mission_id, completion_digest,
                 canonicalizer_version, renderer_version, boundary_event_seq,
                 candidate_artifact_digest
             ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v2', ?2, ?3, ?4, ?5, ?6,
                       'beacon.outcome.manifest.v2', 'beacon.outcome.md.v1', 1, ?7)",
            params![
                Uuid::new_v4().to_string(),
                "ab".repeat(32),
                vec![0_u8; 27],
                Uuid::new_v4().to_string(),
                Uuid::new_v4().to_string(),
                "cd".repeat(32),
                "ef".repeat(32),
            ],
        )
    };
    let refused = insert(&store.connect()).expect_err("a store at 0029 accepted a v2 row");
    assert!(
        refused.to_string().contains("CHECK constraint failed"),
        "refused for the wrong reason: {refused}"
    );

    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("a store at 0029 migrates forward");
    insert(&store.connect()).expect(
        "after the ladder a v2 manifest row is still refused: migration 0030 has not repointed \
         the digest_canonicalizer CHECK to the grammar the codec emits",
    );
}

#[test]
fn sol1_a_store_migrated_through_0029_and_0030_refuses_a_v1_manifest_row() {
    let store = TestStore::new("v1-row-at-0030");
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("the migration ladder reaches 0030");

    let connection = store.connect();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    let error = connection
        .execute(
            "INSERT INTO outcome_manifests (
                 workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                 canonical_bytes, manifest_id, mission_id, completion_digest,
                 canonicalizer_version, renderer_version, boundary_event_seq,
                 candidate_artifact_digest
             ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v1', ?2, ?3, ?4, ?5, ?6,
                       'beacon.outcome.manifest.v1', 'beacon.outcome.md.v1', 1, ?7)",
            params![
                Uuid::new_v4().to_string(),
                "ab".repeat(32),
                vec![0_u8; 27],
                Uuid::new_v4().to_string(),
                Uuid::new_v4().to_string(),
                "cd".repeat(32),
                "ef".repeat(32),
            ],
        )
        .expect_err("a store migrated through 0030 accepted a retired v1 row");
    assert!(
        error.to_string().contains(
            "CHECK constraint failed: digest_canonicalizer = 'beacon.outcome.manifest.v2'"
        ),
        "the retired v1 row was refused for the wrong reason: {error}"
    );
}

// ---------------------------------------------------------------------------------------------
// The assembler's text producer (slice 3b item 0b, item 2)
// ---------------------------------------------------------------------------------------------

#[test]
fn verified_sources_text_is_read_through_owners_not_narrative_projections() {
    let fixture = admitted_mission("sources-through-owners");
    // The classification BEFORE the completion: the collector freezes every source at the
    // completion's boundary, so a classification recorded after it is `AfterBoundary` -- correctly.
    // (`prepare` alone never showed this, because it re-checks the classification by citation
    // rather than by position.)
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let summary = "The second probe was deferred to the next Mission.";
    let (completion_digest, _completion) = fixture.complete_with(
        (fixture.resident_id, fixture.configuration_id),
        vec![NarrativeItem {
            ordinal: 0,
            kind: UnfinishedKind::DeferredScope,
            condition_ordinal: Some(0),
            summary: summary.to_owned(),
        }],
    );
    // The honest read through the public collector: the collected narrative carries a kind and an
    // ordinal and NO text -- which is the point: the text below cannot have come from it.
    let connection = fixture.store.connect();
    let collected = Collector::new(&connection, fixture.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(completion_digest),
            classification_digest: Some(classification.2.clone()),
            ..CollectionRequest::default()
        })
        .expect("collects");
    assert_eq!(
        collected.completion.as_ref().unwrap().unfinished_work[0].kind,
        "deferred_scope"
    );
    drop(connection);

    // The seam from collected values to a document is `prepare_for_mission` and nothing else: the
    // assembler and its text producer are crate-private and a `CollectedSources` is the collector's
    // to build (sol1's P0-1). What the producer read is observed where it lands -- the candidate
    // Artifact's bytes.
    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("a completed, classified Mission prepares");
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .expect("the candidate reads")
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(
        document.contains("Ship one reviewed Beacon feature."),
        "{document}"
    );
    assert!(document.contains(CONDITION_TEXT), "{document}");
    assert!(
        document.contains(summary),
        "the narrative text did not come from the completion's own record:\n{document}"
    );

    // The decoy: lie in the Mission row's own title column. The owner's reader proves the row
    // against its admission Event, so the lie is refused by name, never printed -- a producer that
    // read `missions.desired_outcome` off the column would serve it (acceptance mutant 1).
    with_trigger_down(
        &fixture.store,
        "missions_no_update",
        "missions",
        "Missions are immutable at admission",
        |connection| {
            connection
                .execute(
                    "UPDATE missions SET desired_outcome = 'A title nobody admitted' WHERE id = ?1",
                    params![fixture.mission_id.to_string()],
                )
                .unwrap();
        },
    );
    let refusal = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect_err("a tampered Mission title was served as the document's title");
    // THREE owners can now prove the same tampered row, and which one speaks first is not this
    // test's business -- only that the lie is refused by a name and never printed.
    //
    // #74 (the seams fold) made the completion reader authenticate the Mission's admission Event
    // through the Mission's owner BEFORE anything else, so a `missions` row edited out from under
    // its Event is caught as `MissionAdmissionUnreadable` -- earlier, and by a stronger check, than
    // the two names this list used to allow. The list is widened rather than pinned to one name
    // because pinning it says the refusal must come from a particular layer, which is exactly the
    // coupling that made this test fail when a lower layer got better at its job.
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Sources(SourcesRefusal::MissionUnverified)
                | PrepareRefusal::Sources(SourcesRefusal::Completion(
                    beacond::CompletionReadRefusal::MissionProjectDisagrees(_)
                        | beacond::CompletionReadRefusal::MissionAdmissionUnreadable { .. }
                ))
                | PrepareRefusal::Completion(
                    beacond::CompletionReadRefusal::MissionProjectDisagrees(_)
                        | beacond::CompletionReadRefusal::MissionAdmissionUnreadable { .. }
                )
        ),
        "refused for the wrong reason: {refusal}"
    );
    assert!(
        !refusal.to_string().contains("A title nobody admitted"),
        "the lie was printed: {refusal}"
    );
}

/// sol1's third probe, rebuilt on the path that stays public once the assembler seam is sealed: a
/// hand-built manifest -- no `CollectedSources` anywhere -- that says the completion delivered an
/// Artifact the completion's own bytes never cited. The store's reconcile refuses it by name, on
/// the write path; the same function runs in the startup pass.
#[test]
fn sol1_a_hand_built_manifest_cannot_invent_a_delivered_artifact() {
    let (fixture, mut manifest, mut sources) = ordinary("invented-delivery-by-hand");
    let undelivered = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# This Artifact was never delivered by the completion.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the decoy Artifact records");
    let envelope = undelivered.digest().clone();
    manifest.delivered_artifacts = OutcomeState::Verified(vec![SourceRef {
        workspace_id: fixture.workspace_id,
        kind: SourceKind::Artifact,
        identity: None,
        digest: SourceDigest {
            algorithm: envelope.algorithm.clone(),
            canonicalizer: envelope.canonicalizer.clone(),
            value: envelope.value.clone(),
        },
        pinned_revision: OutcomeState::Absent(
            "an Artifact record pins no content revision.".to_owned(),
        ),
    }]);
    sources.artifacts.push(beacond::VerifiedArtifact {
        digest_value: envelope.value.clone(),
        media_type: "text/markdown".to_owned(),
        title: "(no title recorded)".to_owned(),
    });

    let result = fixture.outcomes.prepare(&manifest, &sources);
    assert!(
        matches!(
            result,
            Err(PrepareRefusal::Irreconcilable { ref detail })
                if detail.contains("delivered Artifacts") && detail.contains(&envelope.value)
        ),
        "prepare stored a manifest that says an Artifact was delivered although the verified \
         completion says none: {result:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The real path: prepare_for_mission (slice 3b item 0b, item 3)
// ---------------------------------------------------------------------------------------------

#[test]
fn a_completed_mission_becomes_a_candidate_outcome() {
    // The first time anything outside a hand-built fixture reaches `prepare`: classify, complete,
    // and ask for the Mission's Outcome by identity. Collector, assembler, owner-read text and
    // `prepare` run in one transaction; what comes back is a candidate whose document carries the
    // Mission's own words, read through their owners.
    let fixture = admitted_mission("mission-to-candidate");
    fixture.classify(ImpactLevel::Ordinary);
    let summary = "One probe is deferred to the follow-up Mission.";
    let (completion_digest, completion) = fixture.complete_with(
        (fixture.resident_id, fixture.configuration_id),
        vec![NarrativeItem {
            ordinal: 0,
            kind: UnfinishedKind::DeferredScope,
            condition_ordinal: Some(0),
            summary: summary.to_owned(),
        }],
    );

    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("a completed, classified Mission prepares");

    let stored = fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(stored.manifest().mission_id, fixture.mission_id);
    assert_eq!(stored.manifest().completion.digest.value, completion_digest);
    assert_eq!(
        stored.manifest().boundary_event_seq,
        completion.boundary_event_seq
    );
    assert_eq!(
        stored.manifest().accountable_resident_id,
        completion.accountable_resident_id
    );
    let record = fixture
        .outcomes
        .candidate(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(
        record.payload().accountable_resident_id,
        fixture.resident_id
    );
    let document = String::from_utf8(record.content().to_vec()).unwrap();
    assert!(
        document.contains("Ship one reviewed Beacon feature."),
        "{document}"
    );
    assert!(document.contains(CONDITION_TEXT), "{document}");
    assert!(document.contains(summary), "{document}");
    assert!(
        document.contains("appendix_mode: policy_state_not_high_impact"),
        "{document}"
    );
}

#[test]
fn sol1_a_post_boundary_classification_cannot_change_which_record_is_frozen() {
    let control = admitted_mission("boundary-classification-control");
    control.classify(ImpactLevel::Ordinary);
    control.complete((control.resident_id, control.configuration_id));
    assert!(
        control
            .outcomes
            .prepare_for_mission(control.mission_id, AppendixRequest::PolicyDefault)
            .is_ok(),
        "the no-decoy control must prepare"
    );

    let fixture = admitted_mission("boundary-classification-decoy");
    let first = fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let later = MissionImpactClassificationV1 {
        workspace_id: fixture.workspace_id,
        mission_id: fixture.mission_id,
        classification_id: Uuid::new_v4(),
        version: 2,
        impact: ImpactLevel::High,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project_id,
            policy_revision: fixture.policy_revision,
            charter_revision: fixture.charter_revision,
        },
        rule_id: None,
        reason: "This reclassification happened after completion.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_001,
        supersedes: ClassificationSupersedes::Record(first.2.clone()),
    };
    MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .classify(&later.canonical_bytes().unwrap())
        .expect("the post-boundary revision records");

    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("a later classification must not hide the newest one at the stored boundary");
    let stored = fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(
        stored.manifest().policy_classification.digest.value,
        first.2
    );
    assert!(!stored.manifest().policy_high_impact);
}

#[test]
fn sol1_a_real_path_candidate_reopens_with_its_v2_digest() {
    let fixture = admitted_mission("real-path-restart-v2");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("the real path prepares");
    let before = fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(
        before.manifest().canonicalizer_version,
        "beacon.outcome.manifest.v2"
    );
    assert_eq!(
        data_encoding::HEXLOWER.encode(&sha2_256(before.canonical_bytes())),
        candidate.manifest_digest
    );

    fixture.restart();
    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .expect("the reopened store unlocks");
    let outcomes = OutcomeService::new(accounts, fixture.workspace_id);
    let reopened = outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the startup pass accepts the real-path manifest");
    assert_eq!(reopened.canonical_bytes(), before.canonical_bytes());
    assert_eq!(
        data_encoding::HEXLOWER.encode(&sha2_256(reopened.canonical_bytes())),
        candidate.manifest_digest
    );
    outcomes
        .candidate(&candidate.manifest_digest)
        .expect("the real-path candidate reads after restart");
}

#[test]
fn prepare_for_mission_is_idempotent_through_the_real_path() {
    let fixture = admitted_mission("mission-idempotent");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let rows = |table: &str| -> i64 {
        fixture
            .store
            .connect()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    let first = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .unwrap();
    let after_first = (rows("outcome_manifests"), rows("artifacts"), rows("events"));
    let second = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .unwrap();
    assert_eq!(
        first, second,
        "the same completion assembled to different identities"
    );
    assert_eq!(
        (rows("outcome_manifests"), rows("artifacts"), rows("events")),
        after_first,
        "a second prepare_for_mission wrote something"
    );
    // A different appendix request is a different manifest, by design: the request is inside the
    // canonical bytes.
    let included = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .unwrap();
    assert_ne!(included.manifest_digest, first.manifest_digest);
}

#[test]
fn prepare_for_mission_refuses_by_name_when_the_mission_has_nothing_to_freeze_on() {
    // No completion: nothing to freeze on. No classification: no appendix bit to key on. Neither is
    // guessed, and each names the Mission.
    let fixture = admitted_mission("mission-nothing-to-freeze");
    let refusal = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect_err("an uncompleted Mission prepared");
    assert!(
        matches!(refusal, PrepareRefusal::MissionNotCompleted { mission_id } if mission_id == fixture.mission_id),
        "{refusal:?}"
    );
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let refusal = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect_err("an unclassified Mission prepared");
    assert!(
        matches!(refusal, PrepareRefusal::MissionNotClassified { mission_id } if mission_id == fixture.mission_id),
        "{refusal:?}"
    );
}

/// R23.4: "current classification" is the newest version whose Event is at or before the
/// completion's boundary. A Mission classified at v1, completed, then reclassified at v2 cites v1
/// -- the newest row in the table is a later classification, not the current one.
#[test]
fn prepare_for_mission_cites_the_newest_classification_at_or_before_the_boundary() {
    let fixture = admitted_mission("current-at-boundary");
    let (first_id, _, _) = fixture.classify_version(ImpactLevel::Ordinary, 1);
    let (_, completion) = fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (second_id, _, _) = fixture.classify_version(ImpactLevel::High, 2);
    assert_ne!(first_id, second_id);

    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("the classification current at the boundary is cited");
    let stored = fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .unwrap();
    let cited = stored
        .manifest()
        .policy_classification
        .identity
        .expect("a Policy citation carries its reference identity");
    assert_eq!(
        (cited.object_id, cited.schema_version),
        (first_id, 1),
        "cited the v2 recorded after the boundary (event {})",
        completion.boundary_event_seq
    );
}

/// R23.4, the other side: every classification of the Mission was recorded after the completion's
/// boundary. None was current when the Mission completed, and the refusal says so by name rather
/// than reaching forward in the log.
#[test]
fn prepare_for_mission_refuses_a_mission_whose_only_classifications_follow_the_boundary() {
    let fixture = admitted_mission("classified-after-boundary");
    let (_, completion) = fixture.complete((fixture.resident_id, fixture.configuration_id));
    fixture.classify_version(ImpactLevel::Ordinary, 1);
    fixture.classify_version(ImpactLevel::High, 2);

    let refusal = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect_err("a Mission classified only after its boundary prepared");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::MissionNotClassifiedAtBoundary {
                mission_id,
                boundary_event_seq,
                classifications_after: 2,
            } if mission_id == fixture.mission_id
                && boundary_event_seq == completion.boundary_event_seq
        ),
        "{refusal:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// The verified reader
// ---------------------------------------------------------------------------------------------

#[test]
fn the_manifest_reader_recomputes_the_digest_over_the_stored_bytes_on_every_read() {
    let (fixture, manifest, sources) = ordinary("byte-flip");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let digest = candidate.manifest_digest.clone();

    // The stored bytes, with one byte flipped, filed under the digest they no longer hash to. No
    // write path builds this row, which is exactly why the reader has to be the thing that catches
    // it: a reader that returns the row instead of recomputing would hand back a manifest whose
    // digest is a lie.
    let stored = fixture
        .outcomes
        .manifest(&digest)
        .unwrap()
        .canonical_bytes()
        .to_vec();
    let mut flipped = stored.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    assert_ne!(flipped, stored);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET canonical_bytes = ?1 WHERE manifest_digest = ?2",
                    params![flipped, digest],
                )
                .unwrap();
        },
    );

    let refusal = fixture.outcomes.manifest(&digest).expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::BytesDisagree)
        ),
        "a flipped byte was not refused: {refusal:?}"
    );
}

#[test]
fn a_decoy_projection_column_cannot_change_what_the_reader_returns() {
    let (fixture, manifest, sources) = ordinary("decoy");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let digest = candidate.manifest_digest.clone();

    // Every projection lies; the bytes stay correct. The reader must follow the object, never the
    // column -- and here "follow the object" means refuse and name the column, because a row that
    // disagrees with its own bytes is invalid state rather than a differently-shaped record.
    // The digest-envelope pair is here because 0029 PERSISTS it: a reader that hard-codes the
    // envelope it returns cannot refuse a lie in either column, and hands back a v1 envelope over
    // bytes filed under something else (gpt2's cold read, P1 #1).
    for (column, lie) in [
        (
            "renderer_version",
            Value::from("beacon.outcome.md.v2".to_owned()),
        ),
        (
            // The lie is the RETIRED token: a plausible stale value, and one the v2 codec refuses
            // by name -- the blanket v1->v2 rename of this file briefly made this lie the truth,
            // and the decoy row read back as honest. A lie table has to keep lying.
            "canonicalizer_version",
            Value::from("beacon.outcome.manifest.v1".to_owned()),
        ),
        ("mission_id", Value::from(Uuid::new_v4().to_string())),
        ("manifest_id", Value::from(Uuid::new_v4().to_string())),
        ("completion_digest", Value::from("aa".repeat(32))),
        ("digest_algorithm", Value::from("sha512".to_owned())),
        (
            "digest_canonicalizer",
            Value::from("beacon.outcome.manifest.v1".to_owned()),
        ),
        // The candidate projection: astra's A2. Until it was checked, changing this column pointed
        // every read at an unrelated document. The lie is a REAL Artifact of this Mission by this
        // Resident, so nothing but the manifest-proof binding can tell it from the candidate.
        (
            "candidate_artifact_digest",
            Value::from(
                fixture
                    .artifacts()
                    .record(
                        ArtifactKind::OutcomeDocument,
                        b"# A decoy document for this Mission\n",
                        ArtifactContext::Mission {
                            project_id: fixture.project_id,
                            mission_id: fixture.mission_id,
                        },
                        fixture.resident_id,
                        fixture.configuration_id,
                        None,
                    )
                    .unwrap()
                    .digest()
                    .value
                    .clone(),
            ),
        ),
        // The reader compared this one and nothing witnessed it: dropping it from `StoredRow`, the
        // SELECT and the comparison left all 21 tests green and clippy at exit 0 (gpt2's P2,
        // reproduced here before this row existed). A check with no witness is a check that can be
        // deleted by accident.
        (
            "boundary_event_seq",
            Value::from(i64::try_from(manifest.boundary_event_seq).unwrap() + 1),
        ),
    ] {
        let previous: Value = fixture
            .store
            .connect()
            .query_row(
                &format!("SELECT {column} FROM outcome_manifests WHERE manifest_digest = ?1"),
                params![digest],
                |row| row.get(0),
            )
            .unwrap();
        with_trigger_down(
            &fixture.store,
            "outcome_manifests_no_update",
            "outcome_manifests",
            "Outcome manifests are immutable",
            |connection| {
                connection
                    .pragma_update(None, "foreign_keys", "OFF")
                    .unwrap();
                connection
                    .pragma_update(None, "ignore_check_constraints", "ON")
                    .unwrap();
                connection
                    .execute(
                        &format!(
                            "UPDATE outcome_manifests SET {column} = ?1 WHERE manifest_digest = ?2"
                        ),
                        params![lie, digest],
                    )
                    .unwrap();
            },
        );
        let refusal = fixture.outcomes.manifest(&digest).expect_err("refuses");
        assert!(
            matches!(
                refusal,
                PrepareRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees { column: named })
                    if named == column
            ),
            "a lie in {column} was not refused by name: {refusal:?}"
        );
        with_trigger_down(
            &fixture.store,
            "outcome_manifests_no_update",
            "outcome_manifests",
            "Outcome manifests are immutable",
            |connection| {
                connection
                    .pragma_update(None, "foreign_keys", "OFF")
                    .unwrap();
                connection
                    .pragma_update(None, "ignore_check_constraints", "ON")
                    .unwrap();
                connection
                    .execute(
                        &format!(
                            "UPDATE outcome_manifests SET {column} = ?1 WHERE manifest_digest = ?2"
                        ),
                        params![previous, digest],
                    )
                    .unwrap();
            },
        );
        fixture
            .outcomes
            .manifest(&digest)
            .expect("the restored row reads");
    }
}

#[test]
fn a_manifest_digest_from_another_workspace_is_refused_rather_than_absent() {
    let (fixture, manifest, sources) = ordinary("cross-workspace");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let connection = fixture.store.connect();
    let stranger = OutcomeService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    let refusal = stranger
        .manifest(&candidate.manifest_digest)
        .expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::CrossWorkspace)
        ),
        "{refusal:?}"
    );
    // ...and a digest nothing holds is `Absent`, which is a different sentence.
    let absent = verified_manifest_within(&connection, fixture.workspace_id, &"bb".repeat(32))
        .expect_err("refuses");
    assert_eq!(absent, ManifestStoreRefusal::Absent);
}

// ---------------------------------------------------------------------------------------------
// prepare
// ---------------------------------------------------------------------------------------------

#[test]
fn prepare_is_idempotent_for_same_manifest_and_renderer_version() {
    let (fixture, manifest, sources) = ordinary("idempotent");
    let first = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let rows = |table: &str| -> i64 {
        fixture
            .store
            .connect()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    let manifests_after_first = rows("outcome_manifests");
    let artifacts_after_first = rows("artifacts");
    let events_after_first = rows("events");

    let second = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    assert_eq!(
        first, second,
        "a second prepare produced different identities"
    );
    assert_eq!(rows("outcome_manifests"), manifests_after_first);
    assert_eq!(rows("artifacts"), artifacts_after_first);
    assert_eq!(
        rows("events"),
        events_after_first,
        "a second prepare wrote an Event"
    );
    assert_eq!(manifests_after_first, 1);
}

#[test]
fn the_renderer_version_is_inside_the_bytes_and_an_unknown_one_stores_nothing() {
    let (fixture, manifest, sources) = ordinary("renderer-version");
    let first = fixture.outcomes.prepare(&manifest, &sources).unwrap();

    // "The same manifest bytes AND the same renderer version" is one condition, not two: the
    // renderer version is a manifest field, so it is already inside the bytes the digest covers.
    // The stored manifest proves it by decoding to the version it was prepared under.
    let stored = fixture.outcomes.manifest(&first.manifest_digest).unwrap();
    assert_eq!(stored.manifest().renderer_version, "beacon.outcome.md.v1");

    // And an unknown version is refused by the manifest's OWN codec, before a transaction is
    // opened -- earlier than the renderer, which is where an unknown version is also refused. A
    // caller cannot reach a store with one.
    let unknown = OutcomeManifestV1 {
        renderer_version: "beacon.outcome.md.v2".to_owned(),
        ..manifest
    };
    let refusal = fixture
        .outcomes
        .prepare(&unknown, &sources)
        .expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::Malformed(_))
        ),
        "an unknown renderer version was not refused by the codec: {refusal:?}"
    );
    let manifests: i64 = fixture
        .store
        .connect()
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(manifests, 1, "a refused manifest was stored");
}

#[test]
fn candidate_artifact_digest_covers_exact_markdown_bytes() {
    let (fixture, manifest, sources) = ordinary("exact-bytes");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let expected = render(&manifest, &sources).expect("renders");

    let record = fixture
        .outcomes
        .candidate(&candidate.manifest_digest)
        .expect("the candidate reads");
    assert_eq!(
        record.content(),
        expected.as_bytes(),
        "the candidate's content is not the exact rendered bytes"
    );
    assert_eq!(record.digest().value, candidate.candidate_artifact_digest);
    assert_eq!(
        record.payload().content_digest.value,
        data_encoding::HEXLOWER.encode(&sha2_256(expected.as_bytes())),
        "the content digest does not cover the exact Markdown bytes"
    );
    assert_eq!(record.payload().media_type, "text/markdown; charset=utf-8");
}

#[test]
fn candidate_accountable_author_equals_completion_accountable_resident() {
    let fixture = admitted_mission("author");
    // Deliberately the SECOND roster member, so a candidate authored by "the first Resident" or by
    // "the Operator" cannot pass by coincidence.
    let accountable = (fixture.other_resident_id, fixture.other_configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let record = fixture
        .outcomes
        .candidate(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(
        record.payload().accountable_resident_id,
        completion.accountable_resident_id
    );
    assert_eq!(
        record.payload().accountable_configuration_id,
        completion.accountable_resident_configuration_id
    );
    assert_ne!(
        record.payload().accountable_resident_id,
        fixture.resident_id
    );
}

#[test]
fn the_accountable_author_comes_from_the_verified_record_not_the_column_beside_it() {
    let fixture = admitted_mission("author-column");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );

    // The completion row's accountable-Resident COLUMN now names the other roster member while its
    // bytes still name the first. A `prepare` that read the column would record a candidate
    // authored by the wrong Resident and report success; one that reads through the completion's
    // verified reader refuses, in the reader's own words.
    with_trigger_down(
        &fixture.store,
        "mission_completions_no_update",
        "mission_completions",
        "Mission completions are immutable",
        |connection| {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            connection
                .execute(
                    "UPDATE mission_completions SET accountable_resident_id = ?1
                     WHERE digest_value = ?2",
                    params![fixture.other_resident_id.to_string(), completion_digest],
                )
                .unwrap();
        },
    );

    let refusal = fixture
        .outcomes
        .prepare(&manifest, &sources)
        .expect_err("refuses");
    assert!(
        matches!(refusal, PrepareRefusal::Completion(_)),
        "the tampered column did not reach the completion's reader: {refusal:?}"
    );
    let manifests: i64 = fixture
        .store
        .connect()
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(manifests, 0, "a refused prepare still stored a manifest");
}

#[test]
fn prepare_refuses_a_manifest_that_disagrees_with_its_completion() {
    let fixture = admitted_mission("reconcile");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (base, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );

    let disagreements: Vec<(&str, OutcomeManifestV1)> = vec![
        (
            "accountable Resident",
            OutcomeManifestV1 {
                accountable_resident_id: fixture.other_resident_id,
                ..base.clone()
            },
        ),
        (
            "boundary event",
            OutcomeManifestV1 {
                boundary_event_seq: base.boundary_event_seq + 1,
                ..base.clone()
            },
        ),
        (
            "completion recorded time",
            OutcomeManifestV1 {
                completion_recorded_at_ms: base.completion_recorded_at_ms + 1,
                ..base.clone()
            },
        ),
        (
            "mission",
            OutcomeManifestV1 {
                mission_id: Uuid::new_v4(),
                ..base.clone()
            },
        ),
        (
            "completion reference identity",
            OutcomeManifestV1 {
                completion: source(
                    SourceKind::Completion,
                    Uuid::new_v4(),
                    COMPLETION_CANONICALIZER,
                    &completion_digest,
                    1,
                    fixture.workspace_id,
                ),
                ..base.clone()
            },
        ),
    ];
    for (what, manifest) in disagreements {
        let refusal = fixture
            .outcomes
            .prepare(&manifest, &sources)
            .expect_err("refuses");
        match refusal {
            PrepareRefusal::Irreconcilable { detail } => assert!(
                detail.contains(what),
                "the refusal for {what} did not name it: {detail}"
            ),
            other => panic!("{what} was not refused as irreconcilable: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The appendix policy
// ---------------------------------------------------------------------------------------------

#[test]
fn high_impact_policy_forces_appendix_even_when_caller_requests_default() {
    let fixture = admitted_mission("high-impact");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::High);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        true,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(manifest.appendix_request, AppendixRequest::PolicyDefault);
    assert_eq!(manifest.appendix_mode(), AppendixMode::Included);

    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap()
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(
        document.contains("appendix_mode: included"),
        "a high-impact Mission's document omitted the appendix:\n{document}"
    );
}

#[test]
fn a_manifest_that_understates_the_policy_bit_is_refused_rather_than_rendered() {
    let fixture = admitted_mission("policy-bit");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::High);
    // The caller writes `false` beside a high-impact classification -- the one move that would
    // otherwise buy a caller an omitted appendix, since the appendix mode is derived from this bit.
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(
        manifest.appendix_mode(),
        AppendixMode::PolicyStateNotHighImpact,
        "the arrangement does not exercise the dodge it exists to close"
    );
    let refusal = fixture
        .outcomes
        .prepare(&manifest, &sources)
        .expect_err("refuses");
    assert!(
        matches!(refusal, PrepareRefusal::Classification(_)),
        "the understated bit was not caught against the classification: {refusal:?}"
    );
}

#[test]
fn ordinary_policy_allows_explicit_include_but_api_has_no_force_omit() {
    let fixture = admitted_mission("ordinary-include");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);

    let (included, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::Include,
    );
    assert_eq!(included.appendix_mode(), AppendixMode::Included);
    let candidate = fixture.outcomes.prepare(&included, &sources).unwrap();
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap()
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(document.contains("appendix_mode: included"), "{document}");

    // The request vocabulary is closed at two arms and neither omits. Two guards, because one was
    // not enough: an earlier version of this test counted a hand-written array, and a `force_omit`
    // arm added to the enum passed it -- a guard that cannot fail is a false green, measured here.
    //
    // First, at compile time. This match is exhaustive TODAY, so a third arm makes this test stop
    // compiling: a closed vocabulary is extended by a named change, never in silence.
    for request in [AppendixRequest::PolicyDefault, AppendixRequest::Include] {
        match request {
            AppendixRequest::PolicyDefault | AppendixRequest::Include => {}
        }
        // Second, the property the vocabulary exists to protect: whatever the caller asks for, a
        // high-impact classification still resolves to `Included`. An arm that suppressed the
        // appendix would have to break this to be worth adding.
        let mandatory = OutcomeManifestV1 {
            policy_high_impact: true,
            appendix_request: request,
            ..included.clone()
        };
        assert_eq!(
            mandatory.appendix_mode(),
            AppendixMode::Included,
            "a caller's request suppressed a mandatory appendix"
        );
    }
    let (default, _) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(
        default.appendix_mode(),
        AppendixMode::PolicyStateNotHighImpact
    );
    let ordinary_document = render(&default, &sources).unwrap();
    assert!(
        ordinary_document.contains("appendix_mode: policy_state_not_high_impact"),
        "the ordinary document did not print its appendix policy state:\n{ordinary_document}"
    );
}

// ---------------------------------------------------------------------------------------------
// Retention and restart
// ---------------------------------------------------------------------------------------------

#[test]
fn manifest_bytes_remain_retrievable_for_every_retained_outcome_version() {
    // Two prepared manifests over one Mission -- the shape a correction takes once versions arrive
    // in 3-B. Both sets of canonical bytes must still be retrievable and still recompute their own
    // digests: an appendix lets a reader recompute a digest from the exported Markdown, and a
    // superseded version whose bytes were dropped makes that impossible for the record it names.
    let (fixture, first_manifest, sources) = ordinary("retained");
    let first = fixture.outcomes.prepare(&first_manifest, &sources).unwrap();
    let second_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..first_manifest.clone()
    };
    let second = fixture
        .outcomes
        .prepare(&second_manifest, &sources)
        .unwrap();
    assert_ne!(first.manifest_digest, second.manifest_digest);

    for (candidate, expected) in [(&first, &first_manifest), (&second, &second_manifest)] {
        let stored = fixture
            .outcomes
            .manifest(&candidate.manifest_digest)
            .unwrap();
        assert_eq!(stored.manifest(), expected);
        assert_eq!(
            stored.canonical_bytes(),
            expected.canonical_bytes().unwrap().as_slice()
        );
        assert_eq!(
            data_encoding::HEXLOWER.encode(&sha2_256(stored.canonical_bytes())),
            candidate.manifest_digest,
            "the retained bytes no longer recompute their own digest"
        );
    }
}

#[test]
fn prepare_survives_restart_without_activity_or_trace() {
    let (fixture, manifest, sources) = ordinary("restart");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let expected = render(&manifest, &sources).unwrap();

    fixture.restart();

    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    assert_eq!(
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("the store unlocks")
            .phase,
        OperatorSessionPhase::Unlocked
    );
    let outcomes = OutcomeService::new(Arc::clone(&accounts), fixture.workspace_id);

    let stored = outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the manifest reads back after restart");
    assert_eq!(stored.manifest(), &manifest);
    assert_eq!(stored.manifest_id(), candidate.manifest_id);
    assert_eq!(
        stored.candidate_artifact_digest(),
        candidate.candidate_artifact_digest
    );
    let record = outcomes
        .candidate(&candidate.manifest_digest)
        .expect("the candidate reads back after restart");
    assert_eq!(record.content(), expected.as_bytes());
}

#[test]
fn the_manifests_reference_identity_is_derived_from_its_content_identity() {
    let (fixture, manifest, sources) = ordinary("reference-identity");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    assert_eq!(
        candidate.manifest_id,
        beacond::manifest_reference_id(&candidate.manifest_digest).unwrap(),
        "the stored reference identity is not a function of the content identity"
    );
    // Which is the property that makes it safe to store beside the bytes: no citation can name a
    // UUID and a digest that disagree, because one IS the other's first sixteen bytes.
    let digest_head = data_encoding::HEXLOWER
        .decode(candidate.manifest_digest.as_bytes())
        .unwrap();
    assert_eq!(candidate.manifest_id.as_bytes(), &digest_head[..16]);
}

// ---------------------------------------------------------------------------------------------
// The startup pass (R18.7)
// ---------------------------------------------------------------------------------------------

/// Inserts one manifest row directly, behind the ledger, naming `candidate` as its candidate.
///
/// Every column is self-consistent: the bytes hash to the digest, the projections repeat what the
/// bytes say, and every foreign key is satisfied. No write path produces this row -- which is the
/// point, because a store that only re-proves on READ answers for it as though `prepare` had made
/// it.
fn insert_manifest_behind_the_ledger(
    fixture: &Fixture,
    manifest: &OutcomeManifestV1,
    candidate_artifact_digest: &str,
) -> String {
    let bytes = manifest.canonical_bytes().unwrap();
    let digest = data_encoding::HEXLOWER.encode(&sha2_256(&bytes));
    let manifest_id = beacond::manifest_reference_id(&digest).unwrap();
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO outcome_manifests (
                 workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                 canonical_bytes, manifest_id, mission_id, completion_digest,
                 canonicalizer_version, renderer_version, boundary_event_seq,
                 candidate_artifact_digest
             ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v2', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, ?10)",
            params![
                fixture.workspace_id.to_string(),
                digest,
                bytes,
                manifest_id.to_string(),
                manifest.mission_id.to_string(),
                manifest.completion.digest.value.as_str(),
                manifest.canonicalizer_version.as_str(),
                manifest.renderer_version.as_str(),
                i64::try_from(manifest.boundary_event_seq).unwrap(),
                candidate_artifact_digest,
            ],
        )
        .unwrap();
    digest
}

#[test]
fn an_outcome_manifest_no_artifact_event_reaches_refuses_startup() {
    let (fixture, manifest, sources) = ordinary("orphan");
    let real = fixture.outcomes.prepare(&manifest, &sources).unwrap();

    // A REAL, ledger-accounted Artifact -- recorded through the service, so it has its own
    // `artifact.recorded` Event and satisfies every foreign key -- but Workspace-scoped, and so
    // not the Outcome Document of this manifest's Mission by this manifest's accountable Resident.
    // The manifest row below claims it anyway.
    let unrelated = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Not this Mission's outcome\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .unwrap();
    let orphan_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest.clone()
    };
    let orphan =
        insert_manifest_behind_the_ledger(&fixture, &orphan_manifest, &unrelated.digest().value);
    assert_ne!(orphan, real.manifest_digest);

    // When R18.7 was folded this row read clean, and that was the gap the startup pass closed:
    // re-proving bytes on every read cannot tell a row the ledger accounts for from one inserted
    // behind it. astra's A2 then closed it a second time and earlier -- the reader now proves the
    // candidate binding, so the row is refused on read as well. Both are asserted, because the
    // startup pass must not quietly become dead code behind the reader.
    let refusal = fixture
        .outcomes
        .manifest(&orphan)
        .expect_err("the reader accepted a manifest whose candidate is not its document");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees {
                column: "candidate_artifact_digest"
            })
        ),
        "{refusal:?}"
    );

    // And every foreign key is satisfied, so whatever refuses this row below, it is not the
    // schema. Measured before the fix: `foreign_key_violations: 0, status: Ready`.
    assert_eq!(
        fixture
            .store
            .connect()
            .query_row("PRAGMA foreign_key_check", [], |row| row
                .get::<_, String>(0))
            .optional()
            .unwrap(),
        None,
        "the arrangement leans on a foreign key rather than on the startup pass"
    );

    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted a manifest the ledger does not account for");
    assert!(
        format!("{refusal:?}").contains("OutcomeManifestStateMismatch"),
        "the orphan was not refused by name: {refusal:?}"
    );
}

#[test]
fn a_manifest_cannot_exist_without_its_candidate() {
    // R18.6 rules that a prepared manifest is positioned by its candidate Artifact's
    // `artifact.recorded` Event rather than by an `outcome.prepared` kind of its own, and revisits
    // that only if a manifest can ever exist without a candidate. It cannot, and this is the test
    // that says so: the column is NOT NULL, and the foreign key requires a real Artifact in the
    // same Workspace.
    let (fixture, manifest, sources) = ordinary("no-candidate");
    fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let orphan_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest
    };
    let bytes = orphan_manifest.canonical_bytes().unwrap();
    let digest = data_encoding::HEXLOWER.encode(&sha2_256(&bytes));
    let manifest_id = beacond::manifest_reference_id(&digest).unwrap();

    for (label, candidate) in [
        ("absent", None),
        ("naming no Artifact", Some("cc".repeat(32))),
    ] {
        let connection = fixture.store.connect();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO outcome_manifests (
                     workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                     canonical_bytes, manifest_id, mission_id, completion_digest,
                     canonicalizer_version, renderer_version, boundary_event_seq,
                     candidate_artifact_digest
                 ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v2', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                           ?9, ?10)",
                params![
                    fixture.workspace_id.to_string(),
                    digest,
                    bytes,
                    manifest_id.to_string(),
                    orphan_manifest.mission_id.to_string(),
                    orphan_manifest.completion.digest.value.as_str(),
                    orphan_manifest.canonicalizer_version.as_str(),
                    orphan_manifest.renderer_version.as_str(),
                    i64::try_from(orphan_manifest.boundary_event_seq).unwrap(),
                    candidate,
                ],
            )
            .expect_err("a manifest with a candidate {label} was stored");
        let error = error.to_string();
        assert!(
            error.contains("NOT NULL") || error.contains("FOREIGN KEY"),
            "a candidate {label} was refused for the wrong reason: {error}"
        );
    }
}

#[test]
fn a_prepared_manifest_still_reopens_after_the_startup_pass() {
    let (fixture, manifest, sources) = ordinary("startup-positive");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    // The positive control: the pass must not refuse the row `prepare` actually made, and the
    // record must still read back as itself afterwards.
    fixture.restart();
    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    assert_eq!(
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("the store unlocks")
            .phase,
        OperatorSessionPhase::Unlocked
    );
    let stored = OutcomeService::new(accounts, fixture.workspace_id)
        .manifest(&candidate.manifest_digest)
        .expect("the prepared manifest still reads after the startup pass");
    assert_eq!(stored.manifest(), &manifest);
}

fn sha2_256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}
// Astra O-3a cold-read probes; append to tests/outcome_prepare.rs at 1c328cc.

fn astra_record_document(fixture: &Fixture, content: &[u8]) -> String {
    fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            content,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .unwrap()
        .digest()
        .value
        .clone()
}

fn astra_boot(fixture: &Fixture) -> Result<beacond::StartupReport, beacond::SupervisorError> {
    Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
}

#[test]
fn astra_same_mission_decoy_document_must_not_account_for_a_manifest() {
    let (fixture, manifest, sources) = ordinary("astra-document-decoy");
    let unrelated_bytes = b"# A different document with the same author and Mission\n";
    assert_ne!(
        render(&manifest, &sources).unwrap().as_bytes(),
        unrelated_bytes
    );
    let unrelated = astra_record_document(&fixture, unrelated_bytes);
    let digest = insert_manifest_behind_the_ledger(&fixture, &manifest, &unrelated);
    // astra wrote this as `candidate(&digest).unwrap()` and asserted the decoy's bytes came back,
    // which is what the store did at b8d7455. The A2 fix makes that unreachable: the reader now
    // proves the candidate binding too, so the decoy is refused before startup is ever reached.
    // The finding's own assertion -- that startup must not accept it -- is unchanged below.
    let read = fixture
        .outcomes
        .candidate(&digest)
        .expect_err("the reader served a decoy document as this manifest's candidate");
    assert!(
        matches!(
            read,
            PrepareRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees {
                column: "candidate_artifact_digest"
            })
        ),
        "{read:?}"
    );
    let boot = astra_boot(&fixture);
    eprintln!("ASTRA_DECOY candidate_is_unrelated=true startup={boot:?}");
    assert!(
        boot.is_err(),
        "startup must reject a candidate whose bytes are not this manifest's rendered document"
    );
}

#[test]
fn astra_candidate_projection_must_be_verified_on_read_and_retry() {
    let (fixture, manifest, sources) = ordinary("astra-candidate-projection");
    let prepared = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let unrelated = astra_record_document(&fixture, b"# Unrelated replacement document\n");
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection.execute("UPDATE outcome_manifests SET candidate_artifact_digest = ?1 WHERE manifest_digest = ?2",
                params![unrelated, prepared.manifest_digest]).unwrap();
        },
    );
    let read = fixture.outcomes.manifest(&prepared.manifest_digest);
    let candidate = fixture.outcomes.candidate(&prepared.manifest_digest);
    let retry = fixture.outcomes.prepare(&manifest, &sources);
    eprintln!(
        "ASTRA_PROJECTION manifest_ok={} candidate_ok={} retry={retry:?}",
        read.is_ok(),
        candidate.is_ok()
    );
    assert!(
        read.is_err() && candidate.is_err() && retry.is_err(),
        "all three paths must refuse an unbound candidate projection"
    );
}

#[test]
fn astra_startup_must_reconcile_completion_copies() {
    let (fixture, mut manifest, sources) = ordinary("astra-startup-completion");
    manifest.boundary_event_seq += 1;
    // Correct prepare rejects this cross-record lie; the renderer and manifest codec can encode it.
    assert!(matches!(
        fixture.outcomes.prepare(&manifest, &sources),
        Err(PrepareRefusal::Irreconcilable { .. })
    ));
    let bytes = render(&manifest, &sources).unwrap();
    let candidate = astra_record_document(&fixture, bytes.as_bytes());
    insert_manifest_behind_the_ledger(&fixture, &manifest, &candidate);
    let boot = astra_boot(&fixture);
    eprintln!(
        "ASTRA_COMPLETION wrong_boundary={} startup={boot:?}",
        manifest.boundary_event_seq
    );
    assert!(
        boot.is_err(),
        "startup must recheck the same completion/manifest agreement that prepare requires"
    );
}

#[test]
fn astra_prepare_must_bind_policy_to_the_completed_mission() {
    let fixture = admitted_mission("astra-other-policy");
    fixture.classify(ImpactLevel::High);
    let team_id: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT team_id FROM missions WHERE id = ?1",
            params![fixture.mission_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let other = MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .admit(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: fixture.charter_revision,
            team_id: Uuid::parse_str(&team_id).unwrap(),
            desired_outcome: "An unrelated ordinary Mission".to_owned(),
            success_conditions: vec![CONDITION_TEXT.to_owned()],
            constraints: vec!["Keep changes reversible".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: vec![
                MissionMemberDraft {
                    resident_id: fixture.resident_id,
                    expected_configuration_id: fixture.configuration_id,
                },
                MissionMemberDraft {
                    resident_id: fixture.other_resident_id,
                    expected_configuration_id: fixture.other_configuration_id,
                },
            ],
        })
        .unwrap();
    let ordinary_classification = MissionImpactClassificationV1 {
        workspace_id: fixture.workspace_id,
        mission_id: other.id,
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project_id,
            policy_revision: fixture.policy_revision,
            charter_revision: fixture.charter_revision,
        },
        rule_id: None,
        reason: "This other Mission is ordinary".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::None,
    };
    let record = MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .classify(&ordinary_classification.canonical_bytes().unwrap())
        .unwrap();
    let classification = (
        ordinary_classification.classification_id,
        1,
        record.digest().value.clone(),
    );
    let (completion_digest, completion) =
        fixture.complete((fixture.resident_id, fixture.configuration_id));
    let mut accepted = Vec::new();
    for request in [AppendixRequest::PolicyDefault, AppendixRequest::Include] {
        let (manifest, sources) = manifest_for(
            &fixture,
            &completion_digest,
            &completion,
            &classification,
            false,
            request,
        );
        let result = fixture.outcomes.prepare(&manifest, &sources);
        if let Ok(ref candidate) = result {
            let content = fixture
                .outcomes
                .candidate(&candidate.manifest_digest)
                .unwrap();
            let markdown = std::str::from_utf8(content.content()).unwrap();
            eprintln!(
                "ASTRA_POLICY request={request:?} accepted=true omitted={}",
                markdown.contains("appendix_mode: policy_state_not_high_impact")
            );
        }
        if result.is_ok() {
            accepted.push(request);
        }
    }
    assert!(
        accepted.is_empty(),
        "a different Mission's valid classification was accepted under: {accepted:?}"
    );
}

#[test]
fn astra_completion_citation_envelope_and_version_must_match_its_owner() {
    let (fixture, base, sources) = ordinary("astra-completion-envelope");
    let mut wrong_canonicalizer = base.clone();
    wrong_canonicalizer.completion.digest.canonicalizer =
        "beacon.mission_completion.v999".to_owned();
    let mut wrong_version = base;
    wrong_version
        .completion
        .identity
        .as_mut()
        .unwrap()
        .schema_version = 999;
    let mut accepted = Vec::new();
    for (name, manifest) in [
        ("canonicalizer", wrong_canonicalizer),
        ("schema_version", wrong_version),
    ] {
        let result = fixture.outcomes.prepare(&manifest, &sources);
        eprintln!("ASTRA_CITATION {name} result={result:?}");
        if result.is_ok() {
            accepted.push(name);
        }
    }
    assert!(
        accepted.is_empty(),
        "accepted false completion citation fields: {accepted:?}"
    );
}

#[test]
fn astra_control_both_insert_faults_roll_back_every_row_and_event() {
    for table in ["artifacts", "outcome_manifests"] {
        let (fixture, manifest, sources) = ordinary(&format!("astra-fault-{table}"));
        let counts = || {
            fixture.store.connect().query_row(
            "SELECT (SELECT count(*) FROM outcome_manifests), (SELECT count(*) FROM artifacts), (SELECT count(*) FROM events)",
            [], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))).unwrap()
        };
        let before = counts();
        fixture.store.connect().execute_batch(&format!(
            "CREATE TRIGGER astra_insert_fault BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT, 'astra injected insert fault'); END;"
        )).unwrap();
        let result = fixture.outcomes.prepare(&manifest, &sources).unwrap_err();
        assert!(format!("{result:?}").contains("astra injected insert fault"));
        assert_eq!(
            counts(),
            before,
            "partial prepare persisted after a fault at {table}"
        );
        fixture
            .store
            .connect()
            .execute_batch("DROP TRIGGER astra_insert_fault")
            .unwrap();
        fixture.outcomes.prepare(&manifest, &sources).unwrap();
        assert_eq!(counts(), (before.0 + 1, before.1 + 1, before.2 + 1));
        eprintln!(
            "ASTRA_ROLLBACK table={table} before={before:?} fault_unchanged=true retry={:?}",
            counts()
        );
    }
}

#[test]
fn astra_control_a_candidate_event_in_another_workspace_refuses_startup() {
    let (fixture, manifest, sources) = ordinary("astra-foreign-event");
    let prepared = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let elsewhere = Uuid::new_v4();
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
            params![elsewhere.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO events (
                 workspace_id, kind, payload_json, occurred_at_ms,
                 predecessor_kind, predecessor_digest, link_digest
             ) VALUES (?1, 'workspace.created', ?2, 1, 'unchained', NULL, NULL)",
            params![
                elsewhere.to_string(),
                serde_json::json!({"workspace_id": elsewhere, "name": "Elsewhere"}).to_string()
            ],
        )
        .unwrap();
    // R58: a second Workspace planted around the append path, in a test whose CONTROL half must
    // reopen Ready. The store says what a fixture made it -- chained from here on -- rather than
    // carrying a row the chain would refuse.
    connection
        .execute_batch(
            "UPDATE events
                SET predecessor_kind = 'unchained', predecessor_digest = NULL, link_digest = NULL;
             UPDATE event_chain_state
                SET genesis_kind = 'migrated',
                    migrated_through_sequence = (SELECT max(sequence) FROM events),
                    head_digest = NULL,
                    head_sequence = (SELECT max(sequence) FROM events)
              WHERE id = 1;",
        )
        .unwrap();
    astra_boot(&fixture).expect("the two-Workspace control is a legitimate store");
    assert_eq!(connection.execute("UPDATE events SET workspace_id = ?1 WHERE kind = 'artifact.recorded' AND json_extract(payload_json, '$.artifact.digest.value') = ?2",
        params![elsewhere.to_string(), prepared.candidate_artifact_digest]).unwrap(), 1);
    let error = astra_boot(&fixture).expect_err("a foreign Event cannot account for the candidate");
    eprintln!("ASTRA_FOREIGN_EVENT refusal={error:?}");
    assert!(matches!(
        error,
        beacond::SupervisorError::ArtifactEventMismatch(_)
    ));
}

#[test]
fn astra_control_reference_identity_is_exactly_the_first_sixteen_digest_bytes() {
    let digest = "0123456789abcdeffedcba987654321000112233445566778899aabbccddeeff";
    let id = beacond::manifest_reference_id(digest).unwrap();
    assert_eq!(id.to_string(), "01234567-89ab-cdef-fedc-ba9876543210");
    assert_eq!(beacond::manifest_reference_id(digest).unwrap(), id);
    assert_ne!(
        beacond::manifest_reference_id(&format!("11{}", &digest[2..])).unwrap(),
        id
    );
}

// kimi's O-3a read probes (D-kimi-read-o3a.md): the two startup-only arms' missing witnesses,
// the duplicate-Event case, and the derived-identity collision behaviour.

#[test]
fn kimi_startup_refuses_a_candidate_scoped_to_another_mission() {
    let fixture = admitted_mission("kimi-scope-mission");

    // A second, real Mission in the same Project, admitted BEFORE the completion so team
    // eligibility is current, with the same Residents as members.
    let team_id: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT team_id FROM missions WHERE id = ?1",
            params![fixture.mission_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let other = MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .admit(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: fixture.charter_revision,
            team_id: Uuid::parse_str(&team_id).unwrap(),
            desired_outcome: "The decoy document's Mission".to_owned(),
            success_conditions: vec![CONDITION_TEXT.to_owned()],
            constraints: vec!["Keep changes reversible".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: vec![
                MissionMemberDraft {
                    resident_id: fixture.resident_id,
                    expected_configuration_id: fixture.configuration_id,
                },
                MissionMemberDraft {
                    resident_id: fixture.other_resident_id,
                    expected_configuration_id: fixture.other_configuration_id,
                },
            ],
        })
        .unwrap();

    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    fixture.outcomes.prepare(&manifest, &sources).unwrap();

    // The planted manifest: same everything, appendix flipped, so its bytes/digest are its own.
    let planted = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest.clone()
    };
    let planted_bytes = planted.canonical_bytes().unwrap();

    // A REAL, ledger-accounted Outcome Document whose content DOES embed the planted manifest's
    // exact bytes (so the reader's content binding passes) but whose own bytes scope it to the
    // OTHER Mission -- the one thing only the startup pass checks.
    let decoy_content = format!(
        "# A decoy that embeds the manifest hex\n{}\n",
        data_encoding::HEXLOWER.encode(&planted_bytes)
    );
    let decoy = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            decoy_content.as_bytes(),
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: other.id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .unwrap();
    let planted_digest =
        insert_manifest_behind_the_ledger(&fixture, &planted, &decoy.digest().value);

    // The reader cannot see the scope mismatch: the content binding is all it has, and it holds.
    fixture
        .outcomes
        .manifest(&planted_digest)
        .expect("the reader's content binding passes; scope is the startup pass's job");

    let boot = astra_boot(&fixture);
    assert!(
        matches!(
            boot,
            Err(beacond::SupervisorError::OutcomeManifestStateMismatch { .. })
        ),
        "startup accepted a candidate scoped to another Mission: {boot:?}"
    );
}

#[test]
fn kimi_startup_refuses_a_candidate_authored_by_another_resident() {
    let (fixture, manifest, sources) = ordinary("kimi-scope-resident");
    fixture.outcomes.prepare(&manifest, &sources).unwrap();

    let planted = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest.clone()
    };
    let planted_bytes = planted.canonical_bytes().unwrap();
    // Same Mission, real ledger-accounted document, content embeds the planted manifest's hex --
    // but the accountable Resident in the Artifact's own bytes is the OTHER team member.
    let decoy_content = format!(
        "# A decoy with the wrong author\n{}\n",
        data_encoding::HEXLOWER.encode(&planted_bytes)
    );
    let decoy = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            decoy_content.as_bytes(),
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.other_resident_id,
            fixture.other_configuration_id,
            None,
        )
        .unwrap();
    let planted_digest =
        insert_manifest_behind_the_ledger(&fixture, &planted, &decoy.digest().value);
    fixture
        .outcomes
        .manifest(&planted_digest)
        .expect("the reader's content binding passes; authorship is the startup pass's job");

    let boot = astra_boot(&fixture);
    assert!(
        matches!(
            boot,
            Err(beacond::SupervisorError::OutcomeManifestStateMismatch { .. })
        ),
        "startup accepted a candidate authored by another Resident: {boot:?}"
    );
}

#[test]
fn kimi_startup_refuses_a_candidate_with_two_recording_events() {
    let (fixture, manifest, sources) = ordinary("kimi-dup-event");
    let prepared = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    fixture.restart();

    // A second `artifact.recorded` Event naming the candidate: two answers to "where did this
    // enter the ledger".
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO events (
                 workspace_id, kind, payload_json, occurred_at_ms,
                 predecessor_kind, predecessor_digest, link_digest
             )
             SELECT workspace_id, kind, payload_json, occurred_at_ms + 1, 'unchained', NULL, NULL
             FROM events
             WHERE kind = 'artifact.recorded'
               AND json_extract(payload_json, '$.artifact.digest.value') = ?1",
            params![prepared.candidate_artifact_digest],
        )
        .unwrap();
    let boot = astra_boot(&fixture);
    eprintln!("KIMI_DUP_EVENT boot={boot:?}");
    assert!(
        boot.is_err(),
        "startup accepted a doubly-recorded candidate"
    );
}

#[test]
fn kimi_a_manifest_id_collision_is_a_loud_refusal_not_a_merge() {
    let (fixture, manifest, sources) = ordinary("kimi-id-collision");
    let prepared = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let connection = fixture.store.connect();
    let row = |digest: &str| {
        connection
            .query_row(
                "SELECT canonical_bytes, manifest_id, mission_id, completion_digest,
                    canonicalizer_version, renderer_version, boundary_event_seq,
                    candidate_artifact_digest
             FROM outcome_manifests WHERE manifest_digest = ?1",
                params![digest],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .unwrap()
    };
    let real = row(&prepared.manifest_digest);
    // A second digest agreeing in its first sixteen bytes -- the derived manifest_id collides.
    let colliding = format!("{}{}", &prepared.manifest_digest[..32], "f".repeat(32));
    assert_ne!(colliding, prepared.manifest_digest);
    assert_eq!(
        beacond::manifest_reference_id(&colliding)
            .unwrap()
            .to_string(),
        real.1,
        "the probe's premise: the derived identity collides"
    );
    let result = connection.execute(
        "INSERT INTO outcome_manifests (
             workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
             canonical_bytes, manifest_id, mission_id, completion_digest,
             canonicalizer_version, renderer_version, boundary_event_seq,
             candidate_artifact_digest
         ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v2', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            fixture.workspace_id.to_string(),
            colliding,
            real.0,
            real.1,
            real.2,
            real.3,
            real.4,
            real.5,
            real.6,
            real.7,
        ],
    );
    let error = result.expect_err("a manifest_id collision was stored");
    eprintln!("KIMI_COLLISION error={error}");
    assert!(
        error.to_string().contains("UNIQUE"),
        "the collision was refused for a reason other than uniqueness: {error}"
    );
}

// ---------------------------------------------------------------------------------------------
// publish: the publication relation (slice 3-B, items 2 and 3)
// ---------------------------------------------------------------------------------------------

/// The Evaluation must be about the EXACT candidate bytes being published.
///
/// The mirror-image mutant gpt2's gate names is "accept Evaluation by ID while its subject digest
/// differs", so the positive control matters as much as the refusal: a real Evaluation of a real
/// sibling Artifact, by the right evaluator, with a `meets` verdict, refused only because its
/// subject is different bytes.
#[test]
fn publish_requires_evaluation_of_exact_candidate_artifact_digest() {
    let fixture = admitted_mission("publish-exact-subject");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    assert!(
        fixture
            .outcomes
            .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
            .is_ok(),
        "the control pairing must publish"
    );

    // A second Mission-scoped Artifact, evaluated properly -- and it is not the candidate.
    let sibling = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Different bytes entirely.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the sibling records");
    let sibling_evaluation = fixture.evaluate(&sibling.digest().value, Verdict::Meets);

    let refusal = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            &candidate_digest,
            sibling_evaluation.digest(),
        )
        .expect_err("an Evaluation of different bytes published the candidate");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate)
        ),
        "{refusal:?}"
    );
}

/// Non-authorship, keyed on the Resident and never on the (Resident, Configuration) pair.
///
/// MEASURED, and it changes what this test can claim: the Evaluation module refuses a
/// self-evaluation at BOTH ends -- `record` refuses the draft, and `verified_evaluation_within`
/// re-derives the same fact from the canonical bytes and refuses the stored row. So a
/// self-evaluation cannot be recorded, and a planted one cannot be read back, which means
/// `publish` can never be handed one and the seam's `EvaluatorAuthoredTheCandidate` arm is
/// unreachable-by-construction from this layer.
///
/// That is defence in depth working, not a gap: the seam re-proves non-authorship from both
/// records so `publish` verifies rather than trusts the Evaluation store's guard, and it stays
/// correct if that guard is ever relaxed. What this test can honestly pin is the wall that DOES
/// fire, and the part of it a check keyed on the pair would let through: the same Resident under a
/// different Configuration is still the author.
#[test]
fn publish_refuses_evaluator_equal_to_accountable_author() {
    let fixture = admitted_mission("publish-non-authorship");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("the Mission prepares");

    // The author, under the exact Configuration the completion names.
    let refusal = fixture
        .evaluations()
        .record(fixture.artifact_evaluation_draft(
            &candidate.candidate_artifact_digest,
            Verdict::Meets,
            (fixture.resident_id, fixture.configuration_id),
        ))
        .expect_err("the candidate's own author evaluated it");
    assert!(
        matches!(
            refusal,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheArtifact)
        ),
        "{refusal:?}"
    );

    // The same Resident under a DIFFERENT Configuration: two evaluators for configuration
    // purposes, ONE author for non-authorship purposes. A check keyed on the pair passes this.
    let second_configuration = fixture.second_configuration_for_author();
    let refusal = fixture
        .evaluations()
        .record(fixture.artifact_evaluation_draft(
            &candidate.candidate_artifact_digest,
            Verdict::Meets,
            (fixture.resident_id, second_configuration),
        ))
        .expect_err("the author under a second Configuration evaluated their own candidate");
    assert!(
        matches!(
            refusal,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheArtifact)
        ),
        "keyed on the (Resident, Configuration) pair rather than the Resident: {refusal:?}"
    );

    // And the independent evaluator publishes, so the refusals above are about authorship and not
    // about the pairing being unpublishable.
    let (_, candidate_digest, evaluation_digest) = fixture.publishable_from(candidate);
    assert!(
        fixture
            .outcomes
            .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
            .is_ok()
    );
}

/// Every verdict that is not `meets` refuses -- including the ones a summariser would round up.
///
/// `uncertain` and `revision_requested` are first-class outcomes, not soft yeses, and there is no
/// "absent", "broken" or "not_evaluated" verdict to launder into one: the grammar has four arms and
/// the verified reader is what makes a missing Evaluation a refusal rather than a zero.
#[test]
fn publish_refuses_absent_broken_not_evaluated_or_non_meets_evaluation() {
    for verdict in [
        Verdict::RevisionRequested,
        Verdict::DoesNotMeet,
        Verdict::Uncertain,
    ] {
        let fixture = admitted_mission("publish-non-meets");
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .expect("the Mission prepares");
        let candidate_digest = fixture
            .artifacts()
            .get(&candidate.candidate_artifact_digest)
            .unwrap()
            .digest()
            .clone();
        let evaluation = fixture.evaluate(&candidate.candidate_artifact_digest, verdict);
        let refusal = fixture
            .outcomes
            .publish(fixture.mission_id, &candidate_digest, evaluation.digest())
            .expect_err("a non-meets verdict published");
        assert!(
            matches!(
                refusal,
                PublishRefusal::Seam(PublicationSeamRefusal::VerdictIsNotMeets)
            ),
            "{verdict:?}: {refusal:?}"
        );
    }

    // An Evaluation nobody recorded is a refusal, never an absence read as approval.
    let fixture = admitted_mission("publish-absent-evaluation");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("the Mission prepares");
    let candidate_digest = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap()
        .digest()
        .clone();
    let refusal = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            &candidate_digest,
            &EvaluationDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.evaluation.v1".to_owned(),
                value: "e".repeat(64),
            },
        )
        .expect_err("an Evaluation nobody recorded published the candidate");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Seam(PublicationSeamRefusal::EvaluationAbsent)
        ),
        "{refusal:?}"
    );
}

/// The relation can only cite records both stores already refused to fabricate.
///
/// The cycle the external relation exists to break: an Evaluation stored INSIDE the candidate would
/// change the candidate's bytes and so change the digest the Evaluation is about. Here the two
/// records stay separate, and the proof is that the candidate's stored bytes do not contain the
/// Evaluation's digest while the relation cites both.
#[test]
fn publication_relation_breaks_self_evaluation_digest_cycle() {
    let fixture = admitted_mission("publication-no-cycle");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    let published = fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");

    let bytes = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap();
    let document = String::from_utf8(bytes.content().to_vec()).unwrap();
    assert!(
        !document.contains(&evaluation_digest.value),
        "the candidate's own bytes name the Evaluation that qualified them, which would change the \
         digest that Evaluation is about"
    );
    // And the relation holds both, outside either record.
    assert_eq!(
        published.cited_evaluation_digest().value,
        evaluation_digest.value
    );
    assert_eq!(
        published.cited_candidate_artifact_digest().value,
        candidate_digest.value
    );
}

/// `publish` completes nothing and grants nothing.
///
/// Asserted on the store, not on the type: the completion record's bytes and the Mission row are
/// unchanged, no Event was appended beyond the ones publication's own reads did not make, and
/// `PublishedOutcome` exposes no predicate a caller could branch on for success or permission.
#[test]
fn publish_does_not_change_mission_completion_or_create_authority() {
    let fixture = admitted_mission("publish-grants-nothing");
    fixture.classify(ImpactLevel::Ordinary);
    let (completion_digest, completion) =
        fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();

    let before_completion: Vec<u8> = fixture
        .store
        .connect()
        .query_row(
            "SELECT canonical_bytes FROM mission_completions WHERE digest_value = ?1",
            params![completion_digest],
            |row| row.get(0),
        )
        .unwrap();
    let before_events = fixture.latest_event_seq();

    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");

    let after_completion: Vec<u8> = fixture
        .store
        .connect()
        .query_row(
            "SELECT canonical_bytes FROM mission_completions WHERE digest_value = ?1",
            params![completion_digest],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        before_completion, after_completion,
        "publication rewrote the completion it cites"
    );
    assert_eq!(
        fixture.latest_event_seq(),
        before_events,
        "publication appended an Event: the relation append IS the publication event, so there is \
         no second record of it"
    );
    // And the bytes still decode to the same record: unchanged, not merely the same length.
    let reread = MissionCompletionV1::from_canonical_bytes(&after_completion)
        .expect("the completion still decodes");
    assert_eq!(
        reread.condition_results.len(),
        completion.condition_results.len()
    );
    assert_eq!(reread.disposition, completion.disposition);
    assert_eq!(reread.mission_id, completion.mission_id);
}

/// A candidate no manifest names is Markdown, not an Outcome.
///
/// The check `publish` adds beyond the seam: the seam proves a pairing of two records, and this
/// proves the document being published is one this module rendered from a frozen manifest. Without
/// it, any Mission-scoped Artifact with a `meets` Evaluation could be published as the Mission's
/// Outcome Document -- prose with no manifest to verify it against.
#[test]
fn publish_refuses_a_candidate_no_manifest_names() {
    let fixture = admitted_mission("publish-no-manifest");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));

    // A well-formed, Mission-scoped, independently evaluated Artifact that `prepare` never made.
    let impostor = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nEverything went well.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the impostor records");
    let evaluation = fixture.evaluate(&impostor.digest().value, Verdict::Meets);

    let refusal = fixture
        .outcomes
        .publish(fixture.mission_id, impostor.digest(), evaluation.digest())
        .expect_err("prose with no manifest published as an Outcome Document");
    assert!(
        matches!(refusal, PublishRefusal::NoManifestForCandidate),
        "{refusal:?}"
    );
}

/// Mission mismatch is a field comparison inside the subject arm's copied envelope, never a join.
///
/// The Evaluation's Artifact subject carries the Mission the Artifact declared, copied into the
/// Evaluation's own canonical bytes at record time. Publishing under a different Mission is refused
/// by comparing that copied field -- no query reaches the `missions` table to decide it.
#[test]
fn publish_refuses_mission_mismatch_from_the_envelope_not_a_join() {
    let fixture = admitted_mission("publish-mission-mismatch");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();

    let other_mission = Uuid::new_v4();
    let refusal = fixture
        .outcomes
        .publish(other_mission, &candidate_digest, &evaluation_digest)
        .expect_err("a candidate published under another Mission");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Seam(PublicationSeamRefusal::MissionDiffers)
        ),
        "{refusal:?}"
    );
}

/// A correction is version n+1 BESIDE version n, never an edit of it (R12.11).
///
/// The corrected Outcome goes through the real path end to end: a second `prepare` with a different
/// appendix request renders different Markdown, so it is a different manifest and a different
/// candidate, and a second independent Evaluation qualifies it. Publishing it leaves version 1
/// exactly where it was -- same publication id, same cited digests. `get` follows the newest;
/// `get_version` still answers for the old one, which is what "preserves" has to mean.
#[test]
fn correction_creates_new_version_and_preserves_superseded_outcome() {
    let fixture = admitted_mission("outcome-correction");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, first_digest, first_evaluation) = fixture.publishable();
    let first = fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("the first publication");
    assert_eq!(first.publication_version(), 1);

    // The correction, through `prepare`: a different appendix request is a different manifest and
    // therefore a different candidate. No planted row anywhere.
    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    assert_ne!(
        corrected.candidate_artifact_digest, first_digest.value,
        "the correction rendered the same bytes, so it is not a second candidate"
    );
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);

    let second = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("the correction publishes");
    assert_eq!(second.publication_version(), 2);
    assert_ne!(second.publication_id(), first.publication_id());

    // Version 1 is untouched: same identity, same cited digests, still readable at its own number.
    let preserved = fixture
        .outcomes
        .get_version(fixture.mission_id, 1)
        .expect("version 1 still reads");
    assert_eq!(preserved.publication_id(), first.publication_id());
    assert_eq!(
        preserved.cited_candidate_artifact_digest().value,
        first_digest.value
    );
    assert_eq!(
        preserved.cited_evaluation_digest().value,
        first_evaluation.value
    );
    assert_eq!(
        fixture
            .outcomes
            .get(fixture.mission_id)
            .expect("the Mission has a current Outcome")
            .publication_version(),
        2
    );
}

/// Versions are contiguous from 1, and a gap is refused by name.
///
/// A "version 4" whose version 2 nobody can produce is a claim about a history that does not exist.
/// The store enforces uniqueness and the floor; contiguity needs the current maximum read inside the
/// same transaction, so `publish` enforces it. The gap is planted directly because `publish` is
/// exactly what cannot create one -- the shape has to come from somewhere else to be refused.
#[test]
fn publication_version_gap_is_refused() {
    let fixture = admitted_mission("publication-version-gap");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("version 1 publishes");
    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome (planted)\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the planted candidate records");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO outcome_publications (
                 workspace_id, mission_id, publication_id, candidate_artifact_digest,
                 evaluation_digest, publication_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, 3)",
            params![
                fixture.workspace_id.to_string(),
                fixture.mission_id.to_string(),
                Uuid::new_v4().to_string(),
                planted.digest().value,
                planted_evaluation.digest().value,
            ],
        )
        .expect("the planted row inserts");

    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    let refusal = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect_err("a version was written over a gap");
    assert!(
        matches!(refusal, PublishRefusal::VersionGap { .. }),
        "{refusal:?}"
    );
}

/// A publication is a historical fact: the relation refuses an in-place edit and a delete.
#[test]
fn a_published_outcome_is_never_edited_or_deleted_in_place() {
    let fixture = admitted_mission("publication-immutable");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");

    let connection = fixture.store.connect();
    let edited = connection.execute(
        "UPDATE outcome_publications SET publication_version = 9 WHERE workspace_id = ?1",
        params![fixture.workspace_id.to_string()],
    );
    assert!(edited.is_err(), "a publication was edited in place");
    let deleted = connection.execute(
        "DELETE FROM outcome_publications WHERE workspace_id = ?1",
        params![fixture.workspace_id.to_string()],
    );
    assert!(deleted.is_err(), "a publication was deleted");
}

#[test]
fn sol1_get_refuses_a_relation_forged_under_another_mission() {
    let fixture = admitted_mission("sol1-get-cross-mission");
    let other_mission = sol1_admit_other_mission(&fixture, "A distinct Mission");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();

    let control = fixture
        .outcomes
        .publish(other_mission, &candidate, &evaluation)
        .expect_err("the public path accepted another Mission's records");
    assert!(matches!(
        control,
        PublishRefusal::Seam(PublicationSeamRefusal::MissionDiffers)
    ));

    let connection = fixture.store.connect();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO outcome_publications (
                 workspace_id, mission_id, publication_id, candidate_artifact_digest,
                 evaluation_digest, publication_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                fixture.workspace_id.to_string(),
                other_mission.to_string(),
                Uuid::new_v4().to_string(),
                candidate.value,
                evaluation.value,
            ],
        )
        .expect("the schema admits the cross-Mission relation");

    let read = fixture.outcomes.get(other_mission);
    eprintln!("SOL1_CROSS_MISSION_GET result={read:?}");
    assert!(
        read.is_err(),
        "get accepted a relation whose candidate, Evaluation, and manifest all belong to another Mission"
    );
}

#[test]
fn sol1_startup_refuses_a_relation_forged_under_another_mission() {
    let fixture = admitted_mission("sol1-startup-cross-mission");
    let other_mission = sol1_admit_other_mission(&fixture, "A distinct Mission");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();
    let connection = fixture.store.connect();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO outcome_publications (
                 workspace_id, mission_id, publication_id, candidate_artifact_digest,
                 evaluation_digest, publication_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![
                fixture.workspace_id.to_string(),
                other_mission.to_string(),
                Uuid::new_v4().to_string(),
                candidate.value,
                evaluation.value,
            ],
        )
        .expect("the schema admits the cross-Mission relation");
    drop(connection);

    let restart = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    });
    eprintln!("SOL1_CROSS_MISSION_STARTUP result={restart:?}");
    assert!(
        restart.is_err(),
        "startup accepted a publication relation that publish itself refuses"
    );
}

#[test]
fn sol1_get_refuses_a_valid_evaluation_swapped_into_the_relation() {
    let fixture = admitted_mission("sol1-get-evaluation-swap");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate, &evaluation)
        .expect("the control publishes");

    let sibling = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# A different candidate\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the sibling records");
    let sibling_evaluation = fixture.evaluate(&sibling.digest().value, Verdict::Meets);
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER outcome_publications_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE outcome_publications SET evaluation_digest = ?1 WHERE workspace_id = ?2",
            params![
                sibling_evaluation.digest().value,
                fixture.workspace_id.to_string()
            ],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER outcome_publications_no_update
             BEFORE UPDATE ON outcome_publications
             BEGIN
                 SELECT RAISE(ABORT, 'Outcome publications are immutable');
             END;",
        )
        .unwrap();

    let refusal = fixture
        .outcomes
        .get(fixture.mission_id)
        .expect_err("get trusted the relation's swapped Evaluation digest");
    assert!(matches!(
        refusal,
        PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate)
    ));
}

/// R28.1 pass 2 of 5: a manifest whose projection disagrees with its own bytes is refused at
/// startup, and the refusal names WHICH manifest and carries the reader's own fault.
///
/// R39.3's two-row shape, made deliberate rather than lucky: two honest manifests are prepared and
/// the tamper lands on whichever sorts LAST in the pass's own `ORDER BY`, so a mutant naming
/// `rows[0]` always names an honest row. Manifest order is by digest, which no fixture can choose,
/// so the fixture reads the order back and picks — it does not hope.
#[test]
fn startup_refuses_a_manifest_whose_column_disagrees_and_names_the_row() {
    // R44: BOTH ENDS. Tampering only the last-sorting row makes "names the
    // failing row" and "names the last row" the same assertion, and a Row arm
    // naming `rows.last()` satisfies it. One run per end, so `rows[0]` and
    // `rows.last()` are each wrong in exactly one of them.
    for end in ["last", "first"] {
        let (fixture, manifest, sources) = ordinary(&format!("r28-manifest-startup-{end}"));
        fixture.outcomes.prepare(&manifest, &sources).unwrap();
        let second = OutcomeManifestV1 {
            appendix_request: AppendixRequest::Include,
            ..manifest.clone()
        };
        fixture
            .outcomes
            .prepare(&second, &sources)
            .expect("a second honest manifest, so rows[0] is not the tampered row");

        let connection = fixture.store.connect();
        // R44.6: CALL the pass's own enumeration rather than copying its query -- a copy agrees
        // with itself, so flipping the pass's ORDER BY would leave this read decorative.
        let ordered: Vec<String> = beacond::enumerate_manifests(&connection)
            .expect("the pass's enumeration reads")
            .into_iter()
            .map(|row| row.1)
            .collect();
        assert_eq!(
            ordered.len(),
            2,
            "the fixture must hold two manifests: {ordered:?}"
        );
        let target = if end == "last" {
            ordered.last()
        } else {
            ordered.first()
        }
        .unwrap()
        .clone();

        connection
            .execute_batch("DROP TRIGGER outcome_manifests_no_update;")
            .unwrap();
        connection
            .execute(
                "UPDATE outcome_manifests SET renderer_version = ?1 WHERE manifest_digest = ?2",
                params!["r28-not-what-the-bytes-say", target],
            )
            .unwrap();
        // Byte-exact, as migration 0030 wrote it (R39.3): SQLite keeps the literal CREATE text, and
        // startup compares the schema before it reaches any verification pass.
        connection
            .execute_batch(
                "CREATE TRIGGER outcome_manifests_no_update
BEFORE UPDATE ON outcome_manifests
BEGIN
    SELECT RAISE(ABORT, 'Outcome manifests are immutable');
END;",
            )
            .unwrap();

        // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
        let read = fixture
            .outcomes
            .manifest(&target)
            .expect_err("the reader trusted a projection its bytes do not support");
        assert!(
            matches!(
                read,
                PrepareRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees {
                    column: "renderer_version"
                })
            ),
            "the planted fault is not the one this test is about: {read:?}"
        );
        drop(connection);

        let refusal = astra_boot(&fixture)
            .expect_err("a store holding a manifest that disagrees with its bytes reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::OutcomeManifestStateMismatch {
                    workspace_id,
                    manifest_digest,
                    cause: ManifestStoreRefusal::ColumnDisagrees {
                        column: "renderer_version"
                    },
                } if *workspace_id == fixture.workspace_id && manifest_digest == &target
            ),
            "refused for the wrong reason, with the cause flattened, or naming the wrong row: \
         {refusal:?}"
        );

        // R28.4: the key and the fault's NAME, and not the carried refusal's own text.
        let line = refusal.to_string();
        assert!(
            line.contains(&target) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("ColumnDisagrees"),
            "the refusal does not name the fault: {line}"
        );
        let carried = ManifestStoreRefusal::ColumnDisagrees {
            column: "renderer_version",
        }
        .to_string();
        assert!(
            !line.contains(&carried),
            "the carried refusal's own text reached the one-line message: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("renderer_version"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28.1 pass 3 of 5: a Mission whose success-condition projection disagrees with its own bytes is
/// refused at startup, and the refusal names WHICH Mission.
///
/// WHY THIS PLANT AND NOT A COLUMN TAMPER. Every column of `missions` is compared against the
/// admission event's payload by `verify_mission_event`, which runs EARLIER, so a tampered column
/// reopens as `MissionEventMismatch` and never reaches this pass -- measured, not assumed: the
/// first version of this bar tampered `success_conditions_json` and got
/// `MissionEventMismatch(12)`. The success-condition projection is the one thing this pass owns
/// alone (`verify_mission_success_conditions`, called only from here).
///
/// R39.3's two-row shape, deliberate: two Missions are admitted, the pass's own `ORDER BY` is read
/// back, and the tamper lands on whichever sorts LAST.
#[test]
fn startup_refuses_a_mission_whose_condition_projection_disagrees_and_names_the_row() {
    // R44: BOTH ENDS. Tampering only the last-sorting row makes "names the
    // failing row" and "names the last row" the same assertion, and a Row arm
    // naming `rows.last()` satisfies it. One run per end, so `rows[0]` and
    // `rows.last()` are each wrong in exactly one of them.
    for end in ["last", "first"] {
        let fixture = admitted_mission(&format!("r28-mission-startup-{end}"));
        let other = sol1_admit_other_mission(&fixture, "A second admitted Mission");
        assert_ne!(other, fixture.mission_id);

        let connection = fixture.store.connect();
        // R44.6: CALL the pass's own enumeration rather than copying its query -- a copy agrees
        // with itself, so flipping the pass's ORDER BY would leave this read decorative.
        let ordered: Vec<String> = beacond::enumerate_missions(&connection)
            .expect("the pass's enumeration reads")
            .into_iter()
            .map(|row| row.1)
            .collect();
        assert_eq!(
            ordered.len(),
            2,
            "the fixture must hold two Missions: {ordered:?}"
        );
        let target = Uuid::parse_str(
            if end == "last" {
                ordered.last()
            } else {
                ordered.first()
            }
            .unwrap(),
        )
        .unwrap();

        connection
            .execute_batch("DROP TRIGGER mission_success_conditions_no_delete;")
            .unwrap();
        let removed = connection
            .execute(
                "DELETE FROM mission_success_conditions WHERE mission_id = ?1",
                params![target.to_string()],
            )
            .unwrap();
        assert!(
            removed > 0,
            "the plant removed nothing, so it plants nothing"
        );
        // Byte-exact, as migration 0024 wrote it (R39.3).
        connection
            .execute_batch(
                "CREATE TRIGGER mission_success_conditions_no_delete
BEFORE DELETE ON mission_success_conditions
BEGIN
    SELECT RAISE(ABORT, 'Mission success conditions are immutable');
END;",
            )
            .unwrap();
        drop(connection);

        // The direct reader still answers: this fault lives in a projection `get` never consults, which
        // is exactly why the startup pass exists and why the pass owns the refusal.
        MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .get(target)
            .expect("the Mission's own record is untouched");

        let refusal = astra_boot(&fixture)
            .expect_err("a store whose condition projection disagrees reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::MissionStateMismatch {
                    workspace_id,
                    mission_id,
                    cause: MissionError::InvalidStore,
                } if *workspace_id == fixture.workspace_id && *mission_id == target
            ),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );

        // R28.4: the key and the fault's NAME.
        let line = refusal.to_string();
        assert!(
            line.contains(&target.to_string()) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("InvalidStore"),
            "the refusal does not name the fault: {line}"
        );
        // NOTE, and it is why this pass has no "cause flattened" mutant: `InvalidStore` is the ONLY
        // cause this pass can reach for a row. Every column of `missions` is compared against the
        // admission event by an earlier pass, so a richer fault never gets here. The row identity is
        // the whole of what widening buys for this pass today.
        assert!(
            format!("{refusal:?}").contains("InvalidStore"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28.1 pass 4 of 5: a Project whose charter history disagrees with its current revision is
/// refused at startup, and the refusal names WHICH Project.
///
/// R39.3's two-row shape, deliberate: a second Project is created and the tamper lands on
/// whichever sorts LAST in the pass's own `ORDER BY`.
#[test]
fn startup_refuses_a_project_whose_charter_history_disagrees_and_names_the_row() {
    // R44: BOTH ENDS. Tampering only the last-sorting row makes "names the
    // failing row" and "names the last row" the same assertion, and a Row arm
    // naming `rows.last()` satisfies it. One run per end, so `rows[0]` and
    // `rows.last()` are each wrong in exactly one of them.
    for end in ["last", "first"] {
        let fixture = admitted_mission(&format!("r28-project-startup-{end}"));
        let second = ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .create(ProjectDraft {
                name: "Beacon Beta".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "A second Project, so rows[0] is not the tampered row.".to_owned(),
                    desired_outcomes: vec!["Exist".to_owned()],
                    constraints: vec!["Keep authority explicit".to_owned()],
                    prohibitions: vec!["Do not deploy".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .expect("the second Project is created");
        assert_ne!(second.id, fixture.project_id);

        let connection = fixture.store.connect();
        // R44.6: CALL the pass's own enumeration rather than copying its query -- a copy agrees
        // with itself, so flipping the pass's ORDER BY would leave this read decorative.
        let ordered: Vec<String> = beacond::enumerate_projects(&connection)
            .expect("the pass's enumeration reads")
            .into_iter()
            .map(|row| row.1)
            .collect();
        assert_eq!(
            ordered.len(),
            2,
            "the fixture must hold two Projects: {ordered:?}"
        );
        let target = Uuid::parse_str(
            if end == "last" {
                ordered.last()
            } else {
                ordered.first()
            }
            .unwrap(),
        )
        .unwrap();

        // `project_charter_revisions` carries no immutability trigger, so this needs no drop-and-restore
        // -- and that absence is itself worth knowing: the history a Project's terminal revision is
        // checked against is not protected the way the records it summarises are.
        // ADDED, not removed: the Project row references its own charter revision, so deleting the
        // history trips a foreign key before the pass ever sees it (measured -- the first version of
        // this bar got `FOREIGN KEY constraint failed`). A revision the Project does not point at
        // makes the terminal revision disagree with the current one, which is this pass's own check.
        let added = connection
            .execute(
                "INSERT INTO project_charter_revisions (
                 workspace_id, project_id, revision, purpose, desired_outcomes_json,
                 constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                 external_effects, author_kind, authored_at_ms
             )
             SELECT workspace_id, project_id, revision + 1, purpose, desired_outcomes_json,
                    constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                    external_effects, author_kind, authored_at_ms + 1
             FROM project_charter_revisions WHERE project_id = ?1",
                params![target.to_string()],
            )
            .unwrap();
        assert!(added > 0, "the plant added nothing, so it plants nothing");
        drop(connection);

        let refusal = astra_boot(&fixture)
            .expect_err("a store whose charter history disagrees reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::ProjectStateMismatch {
                    workspace_id,
                    project_id,
                    cause: ProjectError::InvalidStore,
                } if *workspace_id == fixture.workspace_id && *project_id == target
            ),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );

        // R28.4: the key and the fault's NAME.
        let line = refusal.to_string();
        assert!(
            line.contains(&target.to_string()) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("InvalidStore"),
            "the refusal does not name the fault: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("InvalidStore"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28.1 pass 5 of 5: a completion whose projection disagrees with its own bytes is refused at
/// startup, and the refusal names WHICH completion and carries the reader's own fault.
///
/// R39.3's two-row shape, deliberate: a superseding completion is recorded, the pass's own
/// `ORDER BY digest_value` is read back, and the tamper lands on whichever sorts LAST.
#[test]
fn startup_refuses_a_completion_whose_column_disagrees_and_names_the_row() {
    // R44: BOTH ENDS. Tampering only the last-sorting row makes "names the
    // failing row" and "names the last row" the same assertion, and a Row arm
    // naming `rows.last()` satisfies it. One run per end, so `rows[0]` and
    // `rows.last()` are each wrong in exactly one of them.
    for end in ["last", "first"] {
        let fixture = admitted_mission(&format!("r28-completion-startup-{end}"));
        fixture.classify(ImpactLevel::Ordinary);
        let (first_digest, first) =
            fixture.complete((fixture.resident_id, fixture.configuration_id));

        // A SECOND completion needs a second Mission: `mission_completions` is UNIQUE on
        // (workspace_id, mission_id), so a superseding completion cannot sit beside its predecessor
        // (measured -- the first version of this bar got that constraint back). Two Missions, one
        // completion each.
        let other = sol1_admit_other_mission(&fixture, "A second Mission to complete");
        let second = MissionCompletionV1 {
            mission_id: other,
            // `completion_id` is an R15 reference identity carried INSIDE the bytes, so it has to be
            // its own -- copying the predecessor's collides on (workspace_id, completion_id).
            completion_id: Uuid::new_v4(),
            // The second Mission's admission moved the log, so the predecessor's boundary is no longer
            // a cut this completion can claim (`BoundaryOutOfRange`, measured).
            boundary_event_seq: fixture.latest_event_seq(),
            summary: "The second Mission's own completion.".to_owned(),
            ..first.clone()
        };
        let mut connection = fixture.store.connect();
        let second_digest =
            record_completion(&mut connection, &second).expect("the second completion records");
        assert_ne!(second_digest, first_digest);
        drop(connection);

        let connection = fixture.store.connect();
        // R44.6: CALL the pass's own enumeration rather than copying its query -- a copy agrees
        // with itself, so flipping the pass's ORDER BY would leave this read decorative.
        let ordered: Vec<String> = beacond::enumerate_completions(&connection)
            .expect("the pass's enumeration reads")
            .into_iter()
            .map(|row| row.1)
            .collect();
        assert_eq!(
            ordered.len(),
            2,
            "the fixture must hold two completions: {ordered:?}"
        );
        let target = if end == "last" {
            ordered.last()
        } else {
            ordered.first()
        }
        .unwrap()
        .clone();

        connection
            .execute_batch("DROP TRIGGER mission_completions_no_update;")
            .unwrap();
        connection
            .execute(
                "UPDATE mission_completions SET completed_at_ms = completed_at_ms + 1
             WHERE digest_value = ?1",
                params![target],
            )
            .unwrap();
        // Byte-exact, as migration 0026 wrote it (R39.3).
        connection
            .execute_batch(
                "CREATE TRIGGER mission_completions_no_update
BEFORE UPDATE ON mission_completions
BEGIN
    SELECT RAISE(ABORT, 'Mission completions are immutable');
END;",
            )
            .unwrap();
        drop(connection);

        // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
        let read_connection = fixture.store.connect();
        let read =
            beacond::verified_completion_within(&read_connection, fixture.workspace_id, &target)
                .expect_err("the reader trusted a projection its bytes do not support");
        assert!(
            matches!(
                read,
                CompletionReadRefusal::ColumnDisagrees("completed_at_ms")
            ),
            "the planted fault is not the one this test is about: {read:?}"
        );
        drop(read_connection);

        let refusal = astra_boot(&fixture).expect_err(
            "a store holding a completion that disagrees with its bytes reopened Ready",
        );
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::MissionCompletionStateMismatch {
                    workspace_id,
                    digest_value,
                    cause: CompletionReadRefusal::ColumnDisagrees("completed_at_ms"),
                } if *workspace_id == fixture.workspace_id && digest_value == &target
            ),
            "refused for the wrong reason, with the cause flattened, or naming the wrong row: \
         {refusal:?}"
        );

        // R28.4: the key and the fault's NAME, and not the carried refusal's own text.
        let line = refusal.to_string();
        assert!(
            line.contains(&target) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("ColumnDisagrees"),
            "the refusal does not name the fault: {line}"
        );
        let carried = CompletionReadRefusal::ColumnDisagrees("completed_at_ms").to_string();
        assert!(
            !line.contains(&carried),
            "the carried refusal's own text reached the one-line message: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("completed_at_ms"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28.4 at the innermost point (Fable's addendum 2, from opus2's R28.4-D lane): the driver's own
/// text never reaches a printed line, through ANY mapping onto `SupervisorError::Store`.
///
/// Every pass's `Store` fault maps onto that one arm, so fixing it there makes each outer mapping
/// compliant for free -- and makes this one bar cover all five passes' enumeration failures rather
/// than five near-identical ones.
#[test]
fn store_failures_do_not_print_the_drivers_text_in_the_one_line_message() {
    let fixture = admitted_mission("r28-store-text-channel");
    let connection = fixture.store.connect();
    // A REAL driver error, not a hand-built one: whatever rusqlite says about this is exactly what
    // a failing enumeration inside a pass would carry.
    let driver = connection
        .prepare("SELECT no_such_column_r28 FROM outcome_publications")
        .expect_err("the query names a column that does not exist");
    let text = driver.to_string();
    assert!(
        text.contains("no_such_column_r28"),
        "the driver error does not carry the text this bar is about: {text}"
    );

    let refusal = SupervisorError::Store(driver);
    let line = refusal.to_string();
    assert!(
        !line.contains("no_such_column_r28"),
        "the driver's text reached the one-line message: {line}"
    );
    assert!(
        line.contains("store operation failed"),
        "the refusal does not name the fault: {line}"
    );
    // ...and it is not thrown away: Debug is the channel R28.4 leaves open.
    assert!(
        format!("{refusal:?}").contains("no_such_column_r28"),
        "the driver's text is not reachable through Debug: {refusal:?}"
    );
}

/// kimi's boundary note on the same lane: `PredecessorFault::Unverifiable` carries a decoder's free
/// text, and it is interpolated into a startup refusal through `ClassificationStateFault::Row`.
/// The name is the line; the text is in `Debug`.
#[test]
fn an_unverifiable_predecessors_reason_stays_out_of_the_one_line_message() {
    let fault = PredecessorFault::Unverifiable("SENTINEL_DECODER_TEXT_R28".to_owned());
    let line = fault.to_string();
    assert!(
        !line.contains("SENTINEL_DECODER_TEXT_R28"),
        "the predecessor reader's free text reached the one-line message: {line}"
    );
    assert_eq!(line, "unverifiable", "the line is the fault's name: {line}");
    assert!(
        format!("{fault:?}").contains("SENTINEL_DECODER_TEXT_R28"),
        "the reason is not reachable through Debug: {fault:?}"
    );
}

/// R28 tier 2 pass 2 of 14: an Artifact row the log does not account for is refused at startup,
/// and the refusal names WHICH Artifact and carries the reader's own fault.
///
/// Same shape as pass 1, and for the same measured reason: a column tamper is caught by the event
/// replay first, so the plant is a row with no Event of its own. The planted digest is all `f`, so
/// it sorts last in the pass's own `ORDER BY` by construction (R39.3).
#[test]
fn startup_refuses_an_unaccounted_artifact_row_and_names_the_row() {
    // R44: BOTH ENDS. A planted digest of all `f` sorts LAST in this pass's own
    // ORDER BY and all `0` sorts FIRST -- the extremes of the hex space, so the
    // placement holds against any honest digest. Tampering only one end makes
    // "names the failing row" and "names that end" the same assertion.
    for (end, fill) in [("last", "f"), ("first", "0")] {
        let fixture = admitted_mission(&format!("r28t2-artifact-startup-{end}"));
        // A bare admitted Mission records no Artifact, so the fixture makes one to copy.
        fixture
            .artifacts()
            .record(
                ArtifactKind::OutcomeDocument,
                b"# An Artifact for the plant to copy\n",
                ArtifactContext::Mission {
                    project_id: fixture.project_id,
                    mission_id: fixture.mission_id,
                },
                fixture.resident_id,
                fixture.configuration_id,
                None,
            )
            .expect("the Artifact records");
        let connection = fixture.store.connect();
        let existing: String = connection
            .query_row(
                "SELECT digest_value FROM artifacts ORDER BY digest_value LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the fixture recorded an Artifact");

        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('artifacts')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let planted = fill.repeat(64);
        let names = columns.join(", ");
        let values = columns
            .iter()
            .map(|column| {
                if column == "digest_value" {
                    "?1".to_owned()
                } else {
                    column.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute(
                &format!(
                    "INSERT INTO artifacts ({names}) SELECT {values} FROM artifacts
                 WHERE digest_value = ?2"
                ),
                params![planted, existing],
            )
            .expect("the planted row inserts");
        // R44.4 (astra): the endpoint is CONSTRUCTED, not read back, so assert it -- one ordered
        // read per iteration, the same query the pass runs. Fixed-width lowercase hex makes all
        // `f`/all `0` the extremes today; a migration that changed the digest encoding or the
        // column collation would silently stop bracketing and this bar would decay to one end
        // without failing.
        {
            // R44.6: CALL the pass's own enumeration, do not copy its query. A copied SELECT
            // agrees with itself, so flipping the pass's ORDER BY left this assertion decorative.
            let enumerated =
                beacond::enumerate_artifacts(&connection).expect("the pass's enumeration reads");
            let placed = if end == "last" {
                enumerated.last()
            } else {
                enumerated.first()
            }
            .expect("the plant is in the table");
            assert_eq!(
                placed.1, planted,
                "{end}: the planted row is not at the {end} end of the pass's OWN enumeration"
            );
        }

        let read = fixture
            .artifacts()
            .get(&planted)
            .expect_err("the reader trusted a row whose bytes do not hash to its digest");
        // MEASURED, and it bounds what this pass can prove: the Artifact reader answers a
        // digest-mismatched row with the FLAT `InvalidStore`, not with an `Inconsistent(..)` of its
        // own. `digest_value` hashes `canonical_bytes`, so a plant that both carries a fresh digest and
        // reaches a domain fault would have to mint new canonical bytes through the artifact codec --
        // a fixture that builds records rather than copying one. So this pass ships row-identity-only
        // (R41.1), exactly as tier 1's `mission` and `project` did, and its "cause flattened" mutant is
        // not killable.
        assert!(
            matches!(read, ArtifactError::InvalidStore),
            "the planted fault is not the one this test is about: {read:?}"
        );
        drop(connection);

        let refusal = astra_boot(&fixture)
            .expect_err("a store holding an unaccounted Artifact row reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::ArtifactStateMismatch {
                    workspace_id,
                    digest_value,
                    cause: ArtifactError::InvalidStore,
                } if *workspace_id == fixture.workspace_id && digest_value == &planted
            ),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );

        // R28.4: the key and the fault's NAME.
        let line = refusal.to_string();
        assert!(
            line.contains(&planted) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("InvalidStore"),
            "the refusal does not name the fault: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("InvalidStore"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28 tier 2 pass 1 of 14: an Evaluation row the log does not account for is refused at startup,
/// and the refusal names WHICH Evaluation and carries the reader's own fault.
///
/// WHY A PLANTED ROW AND NOT A COLUMN TAMPER, measured before it was designed. The global event
/// replay runs `verify_evaluation_event` for every `evaluation.recorded` Event EARLIER in
/// `inspect_store`, and that function loads the row and runs the same verified reader -- so a
/// tampered projection reopens as `EvaluationEventMismatch`, not as this pass. The first version of
/// this bar tampered `rationale` and got `EvaluationEventMismatch(15)`.
///
/// The replay walks EVENTS; this pass walks ROWS. A row with no Event of its own is exactly the gap
/// only this pass can see, and it is where the pass's own refusal lives.
///
/// R39.3's "tamper whichever sorts last" is satisfied BY CONSTRUCTION here rather than by reading
/// the order back: the planted digest is all `f`, which sorts after every real sha256.
#[test]
fn startup_refuses_an_unaccounted_evaluation_row_and_names_the_row() {
    // R44: BOTH ENDS. A planted digest of all `f` sorts LAST in this pass's own
    // ORDER BY and all `0` sorts FIRST -- the extremes of the hex space, so the
    // placement holds against any honest digest. Tampering only one end makes
    // "names the failing row" and "names that end" the same assertion.
    for (end, fill) in [("last", "f"), ("first", "0")] {
        let fixture = admitted_mission(&format!("r28t2-evaluation-startup-{end}"));
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        fixture.publishable();

        let connection = fixture.store.connect();
        let existing: String = connection
            .query_row(
                "SELECT digest_value FROM evaluations ORDER BY digest_value LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the fixture recorded an Evaluation");

        // A copy of an honest row under a digest its bytes do not hash to, and with no Event naming it.
        // Column list read from the schema, so this plant does not go stale when a column is added.
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('evaluations')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let planted = fill.repeat(64);
        let names = columns.join(", ");
        let values = columns
            .iter()
            .map(|column| {
                if column == "digest_value" {
                    "?1".to_owned()
                } else {
                    column.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute(
                &format!(
                    "INSERT INTO evaluations ({names}) SELECT {values} FROM evaluations
                 WHERE digest_value = ?2"
                ),
                params![planted, existing],
            )
            .expect("the planted row inserts");
        // R44.4 (astra): the endpoint is CONSTRUCTED, not read back, so assert it -- one ordered
        // read per iteration, the same query the pass runs. Fixed-width lowercase hex makes all
        // `f`/all `0` the extremes today; a migration that changed the digest encoding or the
        // column collation would silently stop bracketing and this bar would decay to one end
        // without failing.
        {
            // R44.6: CALL the pass's own enumeration, do not copy its query -- a copied SELECT
            // agrees with itself, so flipping the pass's ORDER BY left this assertion decorative.
            let enumerated = beacond::enumerate_evaluation_keys(&connection)
                .expect("the pass's enumeration reads");
            let placed = if end == "last" {
                enumerated.last()
            } else {
                enumerated.first()
            }
            .expect("the plant is in the table");
            assert_eq!(
                placed.1, planted,
                "{end}: the planted row is not at the {end} end of the pass's OWN enumeration"
            );
        }

        // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
        let read = fixture
            .evaluations()
            .get(&planted)
            .expect_err("the reader trusted a row whose bytes do not hash to its digest");
        assert!(
            matches!(
                read,
                EvaluationError::Inconsistent(InconsistentEvaluation::DigestMismatch)
            ),
            "the planted fault is not the one this test is about: {read:?}"
        );
        drop(connection);

        let refusal = astra_boot(&fixture)
            .expect_err("a store holding an unaccounted Evaluation row reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::EvaluationStateMismatch {
                    workspace_id,
                    digest_value,
                    cause: EvaluationError::Inconsistent(InconsistentEvaluation::DigestMismatch),
                } if *workspace_id == fixture.workspace_id && digest_value == &planted
            ),
            "refused for the wrong reason, with the cause flattened, or naming the wrong row: \
         {refusal:?}"
        );

        // R28.4: the key and the fault's NAME, and not the carried error's own text.
        let line = refusal.to_string();
        assert!(
            line.contains(&planted) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("Inconsistent"),
            "the refusal does not name the fault: {line}"
        );
        let carried =
            EvaluationError::Inconsistent(InconsistentEvaluation::DigestMismatch).to_string();
        assert!(
            !line.contains(&carried),
            "the carried error's own text reached the one-line message: {line}"
        );
        assert!(
            format!("{refusal:?}").contains("DigestMismatch"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R28.1 pass 1 of 5: a publication that does not re-prove is refused at startup, and the refusal
/// names WHICH publication and carries the reader's own fault.
///
/// Mirrors `sol1_get_refuses_a_valid_evaluation_swapped_into_the_relation` for the plant (the
/// relation's `evaluation_digest` swapped for a valid Evaluation of a different candidate, with
/// the immutability trigger dropped and recreated around the write) and
/// `an_unreadable_classification_is_refused_at_startup_with_the_readers_fault` for the reopen.
#[test]
fn startup_refuses_a_swapped_publication_evaluation_and_names_the_row() {
    // R44: BOTH ENDS. Tampering only the last-sorting publication made "names the failing
    // row" and "names the last row" one assertion. Version 2 sorts last under the pass's
    // `ORDER BY workspace_id, mission_id, publication_version`; version 1 sorts first.
    for (end, target) in [("last", 2_i64), ("first", 1_i64)] {
        let fixture = admitted_mission(&format!("r28-publication-startup-swap-{end}"));
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let (_, candidate, evaluation) = fixture.publishable();
        fixture
            .outcomes
            .publish(fixture.mission_id, &candidate, &evaluation)
            .expect("the control publishes");

        // A SECOND version, through the real correction path, and the tamper lands on THAT one. With a
        // single row the pass cannot tell "the row that failed" from "the first row": a mutant naming
        // `rows[0]` SURVIVED the one-row fixture. Found by running the mutant R28.1 asks for, not by
        // reading the code. Version 1 stays honest and is the positive control inside this test.
        let corrected = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
            .expect("the correction prepares");
        let (_, corrected_candidate, corrected_evaluation) = fixture.publishable_from(corrected);
        let second = fixture
            .outcomes
            .publish(
                fixture.mission_id,
                &corrected_candidate,
                &corrected_evaluation,
            )
            .expect("the correction publishes");
        assert_eq!(
            second.publication_version(),
            2,
            "the tamper has to land on a row that is not rows[0]"
        );

        let sibling = fixture
            .artifacts()
            .record(
                ArtifactKind::OutcomeDocument,
                b"# A different candidate\n",
                ArtifactContext::Mission {
                    project_id: fixture.project_id,
                    mission_id: fixture.mission_id,
                },
                fixture.resident_id,
                fixture.configuration_id,
                None,
            )
            .expect("the sibling records");
        let sibling_evaluation = fixture.evaluate(&sibling.digest().value, Verdict::Meets);

        let connection = fixture.store.connect();
        connection
            .execute_batch("DROP TRIGGER outcome_publications_no_update;")
            .unwrap();
        connection
            .execute(
                &format!(
                    "UPDATE outcome_publications SET evaluation_digest = ?1
                 WHERE workspace_id = ?2 AND publication_version = {target}"
                ),
                params![
                    sibling_evaluation.digest().value,
                    fixture.workspace_id.to_string()
                ],
            )
            .unwrap();
        // Recreated BYTE-EXACTLY as migration 0031 wrote it. SQLite keeps the literal CREATE text in
        // `sqlite_master`, and startup compares the schema before it ever reaches a verification pass,
        // so the sibling `sol1_get_` tests' indented re-CREATE reopens as `SchemaMismatch` and never
        // reaches this pass at all. (Those tests never bootstrap, so it is invisible there.)
        connection
            .execute_batch(
                "CREATE TRIGGER outcome_publications_no_update
BEFORE UPDATE ON outcome_publications
BEGIN
    SELECT RAISE(ABORT, 'Outcome publications are immutable');
END;",
            )
            .unwrap();

        // R44.4 (astra): the endpoint is CONSTRUCTED (versions 1 and 2), not read back, so assert
        // it -- one ordered read, the same query the pass runs.
        {
            // R44.6: CALL the pass's own enumeration rather than restating its ORDER BY.
            let enumerated =
                beacond::enumerate_publications(&connection).expect("the pass's enumeration reads");
            let placed = if end == "last" {
                enumerated.last()
            } else {
                enumerated.first()
            }
            .expect("the fixture published twice");
            assert_eq!(
                placed.2, target,
                "{end}: the tampered version is not at the {end} end of the pass's OWN enumeration"
            );
        }

        // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
        // Only in the `last` run: `get` answers with the LATEST publication, so when the tamper
        // lands on version 1 the reader is asked about an honest row and rightly says nothing.
        // The startup pass below walks EVERY version and is the check that matters in both runs.
        if end == "last" {
            let read = fixture
                .outcomes
                .get(fixture.mission_id)
                .expect_err("get trusted the relation's swapped Evaluation digest");
            assert!(
                matches!(
                    read,
                    PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate)
                ),
                "the planted fault is not the one this test is about: {read:?}"
            );
        }
        drop(connection);

        // And startup refuses it BY NAME, naming the row. Asserting only the variant would pass just
        // as well if startup flattened every cause onto one name (R28.1).
        let refusal = Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .expect_err("a store holding a swapped publication Evaluation reopened Ready");
        assert!(
            matches!(
                &refusal,
                SupervisorError::OutcomePublicationStateMismatch {
                    workspace_id,
                    mission_id,
                    version,
                    cause: PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate),
                } if *workspace_id == fixture.workspace_id
                    && *mission_id == fixture.mission_id
                    && *version == target
            ),
            "refused for the wrong reason, with the cause flattened, or naming the wrong row: \
         {refusal:?}"
        );

        // R28.4: the one-line message is the KEY and the fault's NAME. The carried refusal's own text
        // is reachable through Debug and source(), and must not be pasted into this line -- without
        // this assertion R28.4 is a rule the suite cannot enforce (kimi's pre-read).
        let line = refusal.to_string();
        assert!(
            line.contains(&fixture.mission_id.to_string())
                && line.contains(&fixture.workspace_id.to_string())
                && line.contains(&format!("version {target}")),
            "the refusal does not name the row it refused: {line}"
        );
        assert!(
            line.contains("Seam"),
            "the refusal does not name the fault: {line}"
        );
        let carried =
            PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate).to_string();
        assert!(
            !line.contains(&carried),
            "the carried refusal's own text reached the one-line message: {line}"
        );
        // ...and IS reachable, so nothing was thrown away.
        assert!(
            format!("{refusal:?}").contains("SubjectIsNotTheCandidate"),
            "the carried fault is not reachable through Debug: {refusal:?}"
        );
    }
}

#[test]
fn sol1_get_refuses_a_non_meets_evaluation_of_the_same_candidate_swapped_into_the_relation() {
    let fixture = admitted_mission("sol1-get-same-candidate-evaluation-swap");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, first_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate, &first_evaluation)
        .expect("the control publishes");

    // Independently valid owner record, same exact candidate and verdict, distinct identity/digest.
    let mut replacement = fixture.artifact_evaluation_draft(
        &candidate.value,
        Verdict::DoesNotMeet,
        (fixture.other_resident_id, fixture.other_configuration_id),
    );
    replacement.rationale = "A second reading says the exact candidate does not meet.".to_owned();
    replacement.supersedes = EvaluationSupersedes::Evaluation(first_evaluation.value.clone());
    let replacement_evaluation = fixture
        .evaluations()
        .record(replacement)
        .expect("the replacement Evaluation records through its owner");
    assert_ne!(
        replacement_evaluation.digest().value,
        first_evaluation.value
    );
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER outcome_publications_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE outcome_publications SET evaluation_digest = ?1 WHERE workspace_id = ?2",
            params![
                replacement_evaluation.digest().value,
                fixture.workspace_id.to_string()
            ],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER outcome_publications_no_update
             BEFORE UPDATE ON outcome_publications
             BEGIN
                 SELECT RAISE(ABORT, 'Outcome publications are immutable');
             END;",
        )
        .unwrap();

    let read = fixture.outcomes.get(fixture.mission_id);
    eprintln!("SOL1_NON_MEETS_EVALUATION_SWAP result={read:?}");
    assert!(
        read.is_err(),
        "get accepted a valid does-not-meet Evaluation as qualifying the publication"
    );
}

#[test]
fn sol1_get_refuses_a_noncontiguous_relation_history() {
    let fixture = admitted_mission("sol1-get-version-gap");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();
    let connection = fixture.store.connect();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO outcome_publications (
                 workspace_id, mission_id, publication_id, candidate_artifact_digest,
                 evaluation_digest, publication_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, 2)",
            params![
                fixture.workspace_id.to_string(),
                fixture.mission_id.to_string(),
                Uuid::new_v4().to_string(),
                candidate.value,
                evaluation.value,
            ],
        )
        .expect("the schema admits a history beginning at version 2");

    let read = fixture.outcomes.get(fixture.mission_id);
    eprintln!("SOL1_GAPPED_GET result={read:?}");
    assert!(
        read.is_err(),
        "get accepted a noncontiguous publication history"
    );
}

#[test]
fn sol1_same_candidate_is_not_a_correction() {
    let fixture = admitted_mission("sol1-same-candidate-correction");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate, &evaluation)
        .expect("version 1 publishes");
    let refusal = fixture
        .outcomes
        .publish(fixture.mission_id, &candidate, &evaluation)
        .expect_err("the same candidate was accepted as a correction");
    assert!(matches!(refusal, PublishRefusal::AlreadyPublished));
    let count: i64 = fixture
        .store
        .connect()
        .query_row("SELECT count(*) FROM outcome_publications", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn sol1_concurrent_publish_cannot_create_two_version_one_rows() {
    let fixture = admitted_mission("sol1-concurrent-publish");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate, evaluation) = fixture.publishable();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let results = thread::scope(|scope| {
        let run = |barrier: Arc<std::sync::Barrier>| {
            let accounts = Arc::clone(&fixture.accounts);
            let candidate = candidate.clone();
            let evaluation = evaluation.clone();
            move || {
                barrier.wait();
                OutcomeService::new(accounts, fixture.workspace_id).publish(
                    fixture.mission_id,
                    &candidate,
                    &evaluation,
                )
            }
        };
        let first = scope.spawn(run(Arc::clone(&barrier)));
        let second = scope.spawn(run(Arc::clone(&barrier)));
        [first.join().unwrap(), second.join().unwrap()]
    });
    eprintln!("SOL1_CONCURRENT_PUBLISH results={results:?}");
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let versions: Vec<i64> = fixture
        .store
        .connect()
        .prepare(
            "SELECT publication_version FROM outcome_publications ORDER BY publication_version",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(versions, vec![1]);
}

/// R24.1: the seam's non-authorship arm, exercised directly against the row the Evaluation module
/// would have refused.
///
/// `publication_candidacy` is called with a stored Evaluation whose evaluator IS the candidate's
/// accountable Resident -- planted, because no path records one. The row is made SELF-CONSISTENT
/// (canonical bytes rebuilt through the real codec, digest recomputed over them), so the reader's
/// byte and digest checks pass and the read reaches the checks that judge the record's content.
///
/// What this measures, rather than what it assumes: which refusal comes back, and therefore which
/// layer is load-bearing.
#[test]
fn the_seam_refuses_a_candidacy_whose_evaluator_authored_the_candidate() {
    let fixture = admitted_mission("seam-non-authorship-direct");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, honest_digest) = fixture.publishable();

    // The control: the honest pairing is a candidacy.
    {
        let connection = fixture.store.connect();
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            &honest_digest,
            &candidate_digest,
            fixture.mission_id,
        )
        .expect("the honest pairing must be a candidacy");
    }

    // The planted row: the same Evaluation, re-authored so its evaluator is the candidate's
    // accountable Resident. Bytes and digest are rebuilt together, so the row is exactly what a
    // store would hold if the Evaluation module's guard had not been there.
    let honest = fixture.evaluations().get(&honest_digest.value).unwrap();
    let mut payload = honest.payload().clone();
    payload.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        configuration_id: fixture.configuration_id,
    };
    let planted_bytes = payload
        .canonical_bytes()
        .expect("the re-authored record encodes");
    let planted_value = data_encoding::HEXLOWER.encode(sha2_256(&planted_bytes).as_slice());

    // Inserted as a SIBLING row rather than an edit: the digest is the primary key the findings
    // reference, so re-stamping it in place would orphan them. The column list is read from the
    // live schema so this keeps working when `evaluations` gains a column.
    //
    // The insert trigger has to come down for it: non-authorship is enforced at THREE layers --
    // `record`'s draft check, this trigger, and the verified reader -- and the row a seam-level
    // test needs is precisely the one all three exist to prevent.
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER evaluations_evaluator_is_not_the_artifact_author;")
        .unwrap();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('evaluations')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let projected = columns
        .iter()
        .map(|column| match column.as_str() {
            "digest_value" => "?1".to_owned(),
            "canonical_bytes" => "?2".to_owned(),
            "evaluator_resident_id" => "?3".to_owned(),
            "evaluator_configuration_id" => "?6".to_owned(),
            other => other.to_owned(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    connection
        .execute(
            &format!(
                "INSERT INTO evaluations ({}) SELECT {projected} FROM evaluations
                 WHERE workspace_id = ?4 AND digest_value = ?5",
                columns.join(", ")
            ),
            params![
                planted_value,
                planted_bytes,
                fixture.resident_id.to_string(),
                fixture.workspace_id.to_string(),
                honest_digest.value,
                fixture.configuration_id.to_string()
            ],
        )
        .expect("the planted row inserts");
    connection
        .execute(
            "INSERT INTO evaluation_findings (
                 workspace_id, evaluation_digest, ordinal, severity, summary
             )
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE workspace_id = ?2 AND evaluation_digest = ?3",
            params![
                planted_value,
                fixture.workspace_id.to_string(),
                honest_digest.value
            ],
        )
        .expect("its findings come with it");
    // Restored exactly as migration 0028 wrote it, so the store's schema signature is unchanged.
    connection
        .execute_batch(
            "CREATE TRIGGER evaluations_evaluator_is_not_the_artifact_author
BEFORE INSERT ON evaluations
WHEN NEW.subject_kind = 'artifact'
  AND EXISTS (
      SELECT 1 FROM artifacts
      WHERE artifacts.workspace_id = NEW.workspace_id
        AND artifacts.digest_value = NEW.artifact_digest
        AND artifacts.accountable_resident_id = NEW.evaluator_resident_id
  )
BEGIN
    SELECT RAISE(ABORT, 'The evaluator is the accountable Resident of the subject Artifact');
END;",
        )
        .unwrap();
    drop(connection);

    let planted_digest = EvaluationDigest {
        algorithm: honest_digest.algorithm.clone(),
        canonicalizer: honest_digest.canonicalizer.clone(),
        value: planted_value,
    };
    let connection = fixture.store.connect();
    let refusal = publication_candidacy(
        &connection,
        fixture.workspace_id,
        &planted_digest,
        &candidate_digest,
        fixture.mission_id,
    )
    .expect_err("the candidate's own author qualified it");

    // The seam refuses it, as `Unverified` -- the Evaluation module's refusal, surfaced opaquely.
    //
    // That name is the whole point of this test. The seam calls `verified_evaluation_within` FIRST,
    // and `verified_evaluation` re-derives non-authorship from the canonical bytes and refuses the
    // read, so no self-evaluation ever reaches a check further down. The seam used to carry its own
    // non-authorship arm for this case; R24.1 had it removed (R9) once this test showed the arm
    // could not fire. If this assertion ever fails with a different name, a wall moved: find out
    // which before changing the expectation.
    assert_eq!(refusal, PublicationSeamRefusal::Unverified, "{refusal:?}");
    // And publishing that pairing refuses too, so nothing downstream is more permissive.
    assert!(
        fixture
            .outcomes
            .publish(fixture.mission_id, &candidate_digest, &planted_digest)
            .is_err()
    );
    let _ = candidate;
}

/// R24.2: a published Outcome says which version it is AND how many there are.
///
/// "Version 2" alone cannot tell a reader whether they are holding the current Outcome or a
/// superseded one. After one correction, `get` reports version 2 of 2 and `get_version(.., 1)`
/// reports version 1 of 2 -- the same count from both, because the count is a fact about the
/// Mission's history and not about which version you asked for.
#[test]
fn a_corrected_outcome_reports_which_version_it_is_and_how_many_there_are() {
    let fixture = admitted_mission("outcome-version-count");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, first_digest, first_evaluation) = fixture.publishable();
    let first = fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("the first publication");
    assert_eq!(
        (
            first.publication_version(),
            first.publication_version_count()
        ),
        (1, 1),
        "the only version is version 1 of 1"
    );
    assert!(first.is_current_version());

    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("the correction publishes");

    // The current Outcome: version 2 of 2.
    let current = fixture.outcomes.get(fixture.mission_id).unwrap();
    assert_eq!(
        (
            current.publication_version(),
            current.publication_version_count()
        ),
        (2, 2)
    );
    assert!(current.is_current_version());

    // The superseded one, read at its own number: version 1 OF 2, and it says it is not current.
    let superseded = fixture.outcomes.get_version(fixture.mission_id, 1).unwrap();
    assert_eq!(
        (
            superseded.publication_version(),
            superseded.publication_version_count()
        ),
        (1, 2),
        "a superseded Outcome that reports 1 of 1 reads as the only one there is"
    );
    assert!(!superseded.is_current_version());
    // And it is still the same publication, not a re-derived one.
    assert_eq!(superseded.publication_id(), first.publication_id());
}

// ---------------------------------------------------------------------------------------------
// get: retrieval that re-proves, and labels what it did not (slice 3-B, item 4)
// ---------------------------------------------------------------------------------------------

/// `get` re-verifies all four: the manifest's bytes, the candidate's bytes, the Evaluation, and the
/// relation row itself.
///
/// Each is proved by tampering with it and reading back: a stored record that no longer matches its
/// own bytes is refused, not returned with a warning. The immutability trigger is dropped and
/// restored around each edit, because the store must hold a row nothing should have written for the
/// read path's checks to be reachable at all.
#[test]
fn published_outcome_get_verifies_manifest_artifact_evaluation_and_publication_relation() {
    let fixture = admitted_mission("get-reverifies");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    let published = fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");
    assert!(
        fixture.outcomes.get(fixture.mission_id).is_ok(),
        "the control must read back"
    );

    // 1. A manifest projection column that disagrees with the stored bytes.
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET boundary_event_seq = boundary_event_seq + 1
                     WHERE workspace_id = ?1",
                    params![fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    let refusal = fixture
        .outcomes
        .get(fixture.mission_id)
        .expect_err("a manifest column that disagrees with its bytes was served");
    assert!(
        matches!(refusal, PublishRefusal::Manifest(_)),
        "{refusal:?}"
    );
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET boundary_event_seq = boundary_event_seq - 1
                     WHERE workspace_id = ?1",
                    params![fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    assert!(
        fixture.outcomes.get(fixture.mission_id).is_ok(),
        "the restore must bring the control back"
    );
    assert_eq!(
        published.manifest().digest().value,
        fixture
            .outcomes
            .get(fixture.mission_id)
            .unwrap()
            .manifest()
            .digest()
            .value
    );

    // 2. The candidate Artifact's bytes.
    with_trigger_down(
        &fixture.store,
        "artifacts_no_update",
        "artifacts",
        "Artifacts are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE artifacts SET content_inline = ?1
                     WHERE workspace_id = ?2 AND digest_value = ?3",
                    params![
                        b"# Not the document that was evaluated.\n".to_vec(),
                        fixture.workspace_id.to_string(),
                        candidate.candidate_artifact_digest
                    ],
                )
                .unwrap();
        },
    );
    let refusal = fixture
        .outcomes
        .get(fixture.mission_id)
        .expect_err("a candidate whose stored bytes changed was served");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Unverified(_) | PublishRefusal::Manifest(_)
        ),
        "{refusal:?}"
    );

    // 3. The relation row: a publication naming a candidate the Evaluation is not about. Read from
    //    the Evaluation's own bytes on every read, never trusted from the row.
    let other = admitted_mission("get-reverifies-relation");
    other.classify(ImpactLevel::Ordinary);
    other.complete((other.resident_id, other.configuration_id));
    let (_, other_candidate, other_evaluation) = other.publishable();
    other
        .outcomes
        .publish(other.mission_id, &other_candidate, &other_evaluation)
        .expect("the second Mission publishes");
    let unrelated = other
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# An Artifact no Evaluation is about.\n",
            ArtifactContext::Mission {
                project_id: other.project_id,
                mission_id: other.mission_id,
            },
            other.resident_id,
            other.configuration_id,
            None,
        )
        .expect("the unrelated Artifact records");
    with_trigger_down(
        &other.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![unrelated.digest().value, other.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    let refusal = other
        .outcomes
        .get(other.mission_id)
        .expect_err("a relation row naming bytes the Evaluation is not about was served");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Seam(PublicationSeamRefusal::SubjectIsNotTheCandidate)
                | PublishRefusal::NoManifestForCandidate
        ),
        "{refusal:?}"
    );
}

/// R12.10: both links, and the cited one is never replaced by the current one.
///
/// After the published candidate is superseded, `get` still reports the digest the relation cites --
/// the bytes that were actually evaluated -- and reports the successor BESIDE it as read-time
/// state. A reader that swapped one for the other would be saying an Evaluation approved a document
/// it never saw.
#[test]
fn published_outcome_get_reports_the_cited_digest_and_the_current_tail_separately() {
    let fixture = admitted_mission("get-both-links");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");

    // Nothing has superseded it yet: the chain is the cited digest alone.
    let before = fixture.outcomes.get(fixture.mission_id).unwrap();
    assert_eq!(
        before.supersession_at_read_time().candidate_chain(),
        std::slice::from_ref(&candidate_digest)
    );
    assert_eq!(
        before.supersession_at_read_time().candidate_tail().value,
        candidate_digest.value
    );

    let successor = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome (revised)\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_bytes(&candidate.candidate_artifact_digest)),
        )
        .expect("the successor records");

    let after = fixture.outcomes.get(fixture.mission_id).unwrap();
    assert_eq!(
        after.cited_candidate_artifact_digest().value,
        candidate_digest.value,
        "the tail was served in place of the digest the Evaluation is about"
    );
    assert_eq!(
        after.supersession_at_read_time().candidate_tail().value,
        successor.digest().value,
        "the current tail was not reported"
    );
    assert_eq!(
        after.supersession_at_read_time().candidate_chain().len(),
        2,
        "the chain must carry the cited digest first and the successor after it"
    );
    assert_eq!(
        after.supersession_at_read_time().candidate_chain()[0].value,
        candidate_digest.value
    );
}

/// Boundary facts stay boundary facts. `get` never relabels them as re-verified now.
#[test]
fn published_outcome_get_does_not_present_boundary_citations_as_read_time_reverification() {
    let fixture = admitted_mission("get-boundary-labels");
    fixture.classify(ImpactLevel::Ordinary);
    let (_, completion) = fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");

    let published = fixture.outcomes.get(fixture.mission_id).unwrap();

    // What the manifest cites was proved at the completion's boundary, and the label says so with
    // the exact event -- not "verified", not "current".
    assert_eq!(
        published.citation_provenance(),
        Provenance::VerifiedAtBoundary {
            boundary_event_seq: completion.boundary_event_seq
        }
    );
    assert_eq!(
        published.citation_provenance().to_string(),
        format!(
            "verified at boundary event {}",
            completion.boundary_event_seq
        )
    );
    // And the links derived on this read are labelled read-time, never as boundary facts.
    assert_eq!(
        published.supersession_at_read_time().provenance(),
        Provenance::DerivedAtReadTime
    );
    assert_ne!(
        published.citation_provenance(),
        published.supersession_at_read_time().provenance(),
        "one label for two different questions"
    );
}

/// A published Outcome survives a restart and reads back without Activity or Trace.
#[test]
fn publication_survives_restart_without_activity_or_trace() {
    let fixture = admitted_mission("publication-restart");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    let before = fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes");
    let document = String::from_utf8(before.candidate().content().to_vec()).unwrap();

    fixture.restart();
    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .expect("the reopened store unlocks");
    let outcomes = OutcomeService::new(accounts, fixture.workspace_id);

    let after = outcomes
        .get(fixture.mission_id)
        .expect("the published Outcome reads after restart");
    assert_eq!(after.publication_id(), before.publication_id());
    assert_eq!(after.publication_version(), 1);
    assert_eq!(
        after.cited_candidate_artifact_digest().value,
        candidate_digest.value
    );
    assert_eq!(
        after.cited_evaluation_digest().value,
        evaluation_digest.value
    );
    assert_eq!(
        String::from_utf8(after.candidate().content().to_vec()).unwrap(),
        document,
        "the exact published Markdown did not survive the restart"
    );
}

/// Publication is not success, not completion, and not permission.
///
/// The Mission here was ABANDONED with its one declared condition NOT met, and its Outcome Document
/// publishes anyway -- because the document reports what happened, and what happened was failure. A
/// surface that read `PublishedOutcome` existence as a success verdict would be reporting the
/// opposite of the record.
#[test]
fn published_outcome_existence_is_never_permission_completion_or_success_authority() {
    let fixture = admitted_mission("publication-no-authority");
    fixture.classify(ImpactLevel::Ordinary);
    let (_, completion) =
        fixture.complete_unsuccessfully((fixture.resident_id, fixture.configuration_id));
    assert_eq!(completion.disposition, CompletionDisposition::Abandoned);
    assert_eq!(
        completion.condition_results[0].outcome,
        ConditionOutcome::NotMet
    );

    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    let published = fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("an unsuccessful Mission still publishes its Outcome");

    // The published document reports the failure rather than concealing it.
    let document = String::from_utf8(published.candidate().content().to_vec()).unwrap();
    assert!(
        document.contains("not_met") || document.contains("not met"),
        "the Outcome of a failed Mission does not say so: {document}"
    );
    // And the completion still says abandoned: publishing changed no Work state.
    let reread: Vec<u8> = fixture
        .store
        .connect()
        .query_row(
            "SELECT canonical_bytes FROM mission_completions WHERE mission_id = ?1",
            params![fixture.mission_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let reread = MissionCompletionV1::from_canonical_bytes(&reread).unwrap();
    assert_eq!(reread.disposition, CompletionDisposition::Abandoned);
}

/// R23.2 end to end: a Mission whose condition cites real evidence produces a document that lists
/// it.
///
/// Every layer is the real one -- a recorded Evaluation and a recorded Ruling, cited by the
/// completion's own bytes, read by the collector through their owning verified readers at the
/// boundary, mapped to citations by the assembler, and rendered. The assertion is on the candidate
/// Artifact's Markdown, because that is what a reader actually gets.
#[test]
fn a_condition_with_support_becomes_a_document_that_cites_it() {
    let fixture = admitted_mission("condition-support-cited");
    fixture.classify(ImpactLevel::Ordinary);

    // A recorded Artifact, independently evaluated, plus a Ruling: two citeable kinds.
    let evidence = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# The gate run that proves the condition.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the evidence Artifact records");
    let evaluation = fixture.evaluate(&evidence.digest().value, Verdict::Meets);

    let (_, completion) = fixture.complete_with_support(
        (fixture.resident_id, fixture.configuration_id),
        vec![SupportRef::Evaluation(digest_bytes(
            &evaluation.digest().value,
        ))],
    );
    assert_eq!(completion.condition_results[0].support.len(), 1);

    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("a Mission whose condition cites evidence prepares");

    // The manifest cites it, with the owner's whole envelope.
    let stored = fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the manifest reads");
    let citations = stored.manifest().success_conditions[0]
        .citations
        .verified()
        .expect("the condition's citations are Verified, not Absent");
    assert_eq!(citations.len(), 1);
    assert_eq!(citations[0].kind, SourceKind::Evaluation);
    assert_eq!(citations[0].digest.value, evaluation.digest().value);
    assert_eq!(
        citations[0].digest.canonicalizer,
        evaluation.digest().canonicalizer
    );
    assert_eq!(citations[0].workspace_id, fixture.workspace_id);

    // And the document a reader gets lists it under the condition, not merely in the manifest.
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .expect("the candidate reads")
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(document.contains("- citations:"), "{document}");
    assert!(
        document.contains(&evaluation.digest().value),
        "the cited Evaluation digest is not in the document:\n{document}"
    );
    // The "no supporting object" sentence belongs to a condition that has none, and this one does.
    assert!(
        !document.contains("the completion recorded no supporting object for this condition"),
        "a condition WITH support printed the absent sentence:\n{document}"
    );
}
// ---------------------------------------------------------------------------------------------
// verify_classification_state: the classification startup pass (R26.1)
// ---------------------------------------------------------------------------------------------

/// RED bar: a superseded classification whose bytes cite a predecessor this store does not hold
/// reopens `Ready` before this slice, and must not after.
///
/// The forged row is planted PAST the Command with foreign keys off, and it is version 3 of a chain
/// whose newest version is 3 -- no. Read that again, because the placement is the whole point: the
/// forgery is version 1, and versions 2 and 3 sit above it. The Outcome path reads only the newest
/// classification at or before the completion's boundary, and `history()` is called from tests
/// only, so NOTHING in the product reads version 1 again. The belt runs on read; for this row there
/// is no read. That is the gap the pass closes.
#[test]
fn a_superseded_classification_citing_a_predecessor_the_store_does_not_hold_fails_startup() {
    // R44.5 (kimi): RE-ROLL UNTIL THE PLANT SORTS INTERIOR. The forged record's
    // `classification_id` is a fresh `Uuid::new_v4()` INSIDE the hashed bytes, so the planted
    // digest -- and its position in this pass's `ORDER BY workspace_id, digest_value` -- is drawn
    // again on every run. With three rows the plant lands at an end 2/3 of the time, and when it
    // does, "names the failing row" and "names that end" coincide and a fixed-end mutant survives.
    // Re-drawing until it is interior makes both M8 forms die on EVERY run rather than on two runs
    // in three. Expected draws: 3.
    let mut attempt = 0_u32;
    let (fixture, mut forged, planted) = loop {
        attempt += 1;
        assert!(
            attempt <= 40,
            "40 draws without an interior digest: the re-roll is not reaching the hashed bytes"
        );
        let fixture =
            admitted_mission(&format!("classification-pass-forged-predecessor-{attempt}"));

        // Version 1, planted, citing a predecessor nothing stored.
        let forged = MissionImpactClassificationV1 {
            workspace_id: fixture.workspace_id,
            mission_id: fixture.mission_id,
            classification_id: Uuid::new_v4(),
            version: 1,
            impact: ImpactLevel::Ordinary,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: fixture.project_id,
                policy_revision: fixture.policy_revision,
                charter_revision: fixture.charter_revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes: ClassificationSupersedes::Record("ab".repeat(32)),
        };
        let planted = fixture.plant_classification(&forged);

        // Versions 2 and 3, honest, recorded through the Command. From here nothing reads version 1.
        // Their digests depend on the planted one, so the ordering cannot be settled before the draw.
        let second =
            fixture.classify_superseding(2, ClassificationSupersedes::Record(planted.clone()));
        fixture.classify_superseding(3, ClassificationSupersedes::Record(second));

        // R44.4: the placement is READ BACK from the pass's own query, not assumed.
        let connection = fixture.store.connect();
        // R44.6: CALL the pass's own enumeration rather than copying its query.
        let enumerated =
            beacond::enumerate_classifications(&connection).expect("the pass's enumeration reads");
        let ends: Vec<String> = [enumerated.first(), enumerated.last()]
            .into_iter()
            .map(|row| {
                row.expect("the fixture holds three classifications")
                    .1
                    .clone()
            })
            .collect();
        drop(connection);
        if !ends.contains(&planted) {
            break (fixture, forged, planted);
        }
    };

    // The belt still refuses version 1 ON READ, so the fault is real and has a name.
    let connection = fixture.store.connect();
    let read = beacond::verified_classification_within(&connection, fixture.workspace_id, &planted)
        .expect_err("a classification citing a predecessor nothing stored must not read");
    assert!(
        matches!(
            read,
            ClassificationStoreError::Predecessor {
                fault: PredecessorFault::NotStored,
                ..
            }
        ),
        "the planted fault is not the one this test is about: {read:?}"
    );
    drop(connection);
    forged.version = 1;

    // The plant is INTERIOR: neither `rows[0]` nor `rows.last()` of this pass's own ORDER BY,
    // asserted above by the re-roll's exit condition rather than assumed.

    // And startup refuses it BY NAME. Before this slice, this reopened Ready.
    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("a store holding a forged supersession link reopened Ready");
    // R27.1: the refusal names the ROW and carries the owning reader's own fault. Asserting only
    // the variant would pass just as well if startup flattened every cause onto one name.
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch {
                workspace_id,
                digest,
                cause: ClassificationStoreError::Predecessor {
                    fault: PredecessorFault::NotStored,
                    ..
                },
            } if *workspace_id == fixture.workspace_id && digest == &planted
        ),
        "refused for the wrong reason, or with the cause flattened: {refusal:?}"
    );
}

/// Positive control: an honest chain of three versions reopens `Ready`.
#[test]
fn an_honest_classification_chain_reopens_ready() {
    let fixture = admitted_mission("classification-pass-honest-chain");
    let first = fixture.classify_superseding(1, ClassificationSupersedes::None);
    let second = fixture.classify_superseding(2, ClassificationSupersedes::Record(first));
    fixture.classify_superseding(3, ClassificationSupersedes::Record(second));

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("an honest chain reopens");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
}

/// Positive control (R26.5): the pass runs for a store that never held a classification, and for a
/// freshly bootstrapped one. A pass that only runs when the table is non-empty is a pass whose
/// absence nobody would notice.
#[test]
fn a_store_that_never_held_a_classification_reopens_ready() {
    let store = TestStore::new("classification-pass-empty");
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("an empty store reopens");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);

    let fixture = admitted_mission("classification-pass-none-stored");
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("a store with a Mission but no classification reopens");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
}

fn sol1_classification_record(
    fixture: &Fixture,
    mission_id: Uuid,
    version: u32,
    supersedes: ClassificationSupersedes,
    reason: &str,
) -> MissionImpactClassificationV1 {
    MissionImpactClassificationV1 {
        workspace_id: fixture.workspace_id,
        mission_id,
        classification_id: Uuid::new_v4(),
        version,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project_id,
            policy_revision: fixture.policy_revision,
            charter_revision: fixture.charter_revision,
        },
        rule_id: None,
        reason: reason.to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000 + u64::from(version),
        supersedes,
    }
}

fn sol1_classification_snapshot(fixture: &Fixture) -> (Vec<(String, Vec<u8>)>, i64, i64) {
    let connection = fixture.store.connect();
    let rows = connection
        .prepare(
            "SELECT digest_value, canonical_bytes FROM mission_impact_classifications
             ORDER BY digest_value",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let events = connection
        .query_row("SELECT count(*) FROM events", [], |row| row.get(0))
        .unwrap();
    let schema = connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    (rows, events, schema)
}

#[test]
fn sol1_startup_visits_the_hidden_first_version_of_the_second_mission() {
    let fixture = admitted_mission("sol1-classification-two-missions");
    let first_a = fixture.plant_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        1,
        ClassificationSupersedes::None,
        "Mission A version one",
    ));
    fixture.plant_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        2,
        ClassificationSupersedes::Record(first_a),
        "Mission A version two",
    ));
    let mission_b = sol1_admit_other_mission(&fixture, "The second classified Mission");
    let forged_b = fixture.plant_classification(&sol1_classification_record(
        &fixture,
        mission_b,
        1,
        ClassificationSupersedes::Record("ab".repeat(32)),
        "Mission B hidden forged version one",
    ));
    fixture.plant_classification(&sol1_classification_record(
        &fixture,
        mission_b,
        2,
        ClassificationSupersedes::Record(forged_b),
        "Mission B version two",
    ));
    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("the hidden forged row in the second Mission reopened Ready");
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch {
                cause: ClassificationStoreError::Predecessor {
                    fault: PredecessorFault::NotStored,
                    ..
                },
                ..
            }
        ),
        "refused for the wrong reason, or with the cause flattened: {refusal:?}"
    );
}

#[test]
fn sol1_fk_orphan_is_refused_before_the_classification_pass() {
    let fixture = admitted_mission("sol1-classification-orphan-workspace");
    let workspace_id = Uuid::new_v4();
    let record = MissionImpactClassificationV1 {
        workspace_id,
        mission_id: Uuid::new_v4(),
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: Uuid::new_v4(),
            policy_revision: 1,
            charter_revision: 1,
        },
        rule_id: None,
        reason: "No Mission or Policy exists in this Workspace.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::None,
    };
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    let connection = fixture.store.connect();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    connection
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'No policy rows', 1)",
            params![workspace_id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1,
                       'ordinary', 'operator_declared', 1, 1, ?7)",
            params![
                workspace_id.to_string(),
                digest,
                bytes,
                record.classification_id.to_string(),
                record.mission_id.to_string(),
                record.policy.project_id.to_string(),
                i64::try_from(record.classified_at_ms).unwrap(),
            ],
        )
        .unwrap();
    drop(connection);
    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("an orphaned classification reopened Ready");
    assert!(matches!(refusal, SupervisorError::ForeignKeyViolations(_)));
}

#[test]
fn sol1_an_old_honest_classification_survives_a_later_policy_revision() {
    let fixture = admitted_mission("sol1-classification-old-policy");
    fixture.classify_superseding(1, ClassificationSupersedes::None);
    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .publish(
            fixture.project_id,
            fixture.charter_revision,
            Some(fixture.policy_revision),
            ProjectPolicyDraft {
                team_lead_selection: TeamLeadSelectionPolicy {
                    required_capabilities: vec![
                        BeaconCapability::WorkCoordinateMission,
                        BeaconCapability::WorkCreateMission,
                    ],
                    availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                    track_record_requirements: vec![TrackRecordRequirement {
                        evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                        minimum_count: 1,
                    }],
                    conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                    ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                },
            },
        )
        .expect("the later Policy revision records");
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("a later Policy revision must not invalidate an honest old classification");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
}

#[test]
fn sol1_unreadable_classification_startup_error_carries_the_reader_fault() {
    let fixture = admitted_mission("sol1-classification-unreadable-name");
    let record = sol1_classification_record(
        &fixture,
        fixture.mission_id,
        1,
        ClassificationSupersedes::None,
        "The projections are valid but the bytes are unreadable",
    );
    let bytes = vec![0xff; 25];
    let digest = data_encoding::HEXLOWER.encode(&sha2_256(&bytes));
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1,
                       'ordinary', 'operator_declared', ?7, ?8, ?9)",
            params![
                fixture.workspace_id.to_string(),
                digest,
                bytes,
                record.classification_id.to_string(),
                fixture.mission_id.to_string(),
                fixture.project_id.to_string(),
                fixture.policy_revision,
                fixture.charter_revision,
                i64::try_from(record.classified_at_ms).unwrap(),
            ],
        )
        .unwrap();
    let direct = beacond::verified_classification_within(
        &fixture.store.connect(),
        fixture.workspace_id,
        &digest,
    )
    .expect_err("the unreadable row read successfully");
    assert!(matches!(direct, ClassificationStoreError::Malformed(_)));
    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("the unreadable row reopened Ready");
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch {
                cause: ClassificationStoreError::Malformed(_),
                ..
            }
        ),
        "startup flattened the owner's fault: {refusal:?}"
    );
    assert!(
        format!("{refusal:?}").contains("Malformed"),
        "startup flattened the owner's fault: {refusal:?}"
    );
}

#[test]
fn sol1_cross_mission_predecessor_startup_error_carries_the_fault_and_writes_nothing() {
    let fixture = admitted_mission("sol1-classification-cross-mission-name");
    let other_mission = sol1_admit_other_mission(&fixture, "Own the predecessor");
    let predecessor = fixture.plant_classification(&sol1_classification_record(
        &fixture,
        other_mission,
        1,
        ClassificationSupersedes::None,
        "A valid classification of the other Mission",
    ));
    let successor = fixture.plant_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        1,
        ClassificationSupersedes::Record(predecessor),
        "A forged link across Missions in one Project",
    ));
    let connection = fixture.store.connect();
    let violations: i64 = connection
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(violations, 0, "0032, not the belt, refused the fixture");
    let direct =
        beacond::verified_classification_within(&connection, fixture.workspace_id, &successor)
            .expect_err("the cross-Mission predecessor read successfully");
    assert!(matches!(
        direct,
        ClassificationStoreError::Predecessor {
            fault: PredecessorFault::ForAnotherMission,
            ..
        }
    ));
    drop(connection);
    let before = sol1_classification_snapshot(&fixture);
    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("the cross-Mission predecessor reopened Ready");
    let after = sol1_classification_snapshot(&fixture);
    assert_eq!(after, before, "the refusing startup changed stored state");
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch {
                cause: ClassificationStoreError::Predecessor {
                    fault: PredecessorFault::ForAnotherMission,
                    ..
                },
                ..
            }
        ),
        "startup flattened PredecessorFault::ForAnotherMission: {refusal:?}"
    );
    assert!(
        format!("{refusal:?}").contains("ForAnotherMission"),
        "startup flattened PredecessorFault::ForAnotherMission: {refusal:?}"
    );
}

#[test]
fn sol1_concurrent_supervisors_are_not_refused_while_the_classification_pass_runs() {
    const SUPERVISORS: usize = 8;
    let fixture = admitted_mission("sol1-classification-concurrent-startup");
    for version in 1..=128 {
        fixture.plant_supported_classification(&sol1_classification_record(
            &fixture,
            fixture.mission_id,
            version,
            ClassificationSupersedes::None,
            &format!("Independent classification row {version}"),
        ));
    }
    let barrier = Arc::new(std::sync::Barrier::new(SUPERVISORS));
    let handles = (0..SUPERVISORS)
        .map(|_| {
            let path = fixture.store.path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                Supervisor::bootstrap(StartupConfig {
                    store_path: path,
                    initial_workspace_name: "Personal".to_owned(),
                })
                .map_err(|error| format!("{error:?}"))
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("a supervisor thread panicked"))
        .collect::<Vec<_>>();
    assert!(
        results.iter().all(Result::is_ok),
        "concurrent supervisors were refused: {results:?}"
    );
}

#[test]
fn sol1_a_forked_classification_chain_reopens_ready_by_ruling() {
    let fixture = admitted_mission("sol1-classification-fork-known-nonfinding");
    let first = fixture.plant_supported_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        1,
        ClassificationSupersedes::None,
        "The common root",
    ));
    fixture.plant_supported_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        2,
        ClassificationSupersedes::Record(first.clone()),
        "The first child",
    ));
    fixture.plant_supported_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        3,
        ClassificationSupersedes::Record(first),
        "The second child of the same root",
    ));
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("R26.7 leaves fork/continuity to the later structural slice");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
}

/// The pass walks EVERY Workspace, and this is the test that can tell.
///
/// Fable's mutant restricted the enumeration to one Workspace
/// (`WHERE workspace_id = (SELECT MAX(workspace_id) FROM mission_impact_classifications)`) and the
/// whole suite stayed green: every other test in this file uses a single-Workspace store, so "for
/// every Workspace" was a claim nothing could see.
///
/// WHICH Workspace holds the forgery is chosen, not drawn. `workspace_id` is stored as TEXT, so
/// `MAX` is lexicographic over the hyphenated UUID string, and the second Workspace is given the
/// id `ffffffff-ffff-ffff-ffff-ffffffffffff` -- the largest such string there is, since every
/// character is the maximum hex digit. So the bootstrapped Workspace, whatever random id it drew,
/// always sorts below it, and the forged row goes THERE: in the Workspace `MAX` skips. The test
/// asserts that ordering rather than trusting it.
#[test]
fn a_forged_link_in_a_second_workspace_that_max_would_skip_fails_startup() {
    let fixture = admitted_mission("classification-pass-two-workspaces");

    // The second Workspace, with the largest possible id, and its own Project/Policy/Mission so its
    // classifications verify on their own merits.
    let highest = Uuid::from_u128(u128::MAX);
    assert_eq!(highest.to_string(), "ffffffff-ffff-ffff-ffff-ffffffffffff");
    assert!(
        fixture.workspace_id.to_string() < highest.to_string(),
        "the bootstrapped Workspace must sort below the planted one, or this test proves nothing"
    );
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Second', 7)",
            params![highest.to_string()],
        )
        .expect("the second Workspace row inserts");
    let second = admit_mission_in(&fixture.accounts, highest);

    // An honest chain in the HIGHER Workspace -- the one the mutant keeps. Under the mutant these
    // are the only rows walked, and they all verify, so it reports a healthy store.
    let second_first = classify_in(
        &fixture.accounts,
        highest,
        &second,
        1,
        ClassificationSupersedes::None,
    );
    classify_in(
        &fixture.accounts,
        highest,
        &second,
        2,
        ClassificationSupersedes::Record(second_first),
    );

    // An honest chain in the LOWER Workspace, with a forged v1 underneath it: v1 cites a
    // predecessor nothing stored, and v2 supersedes v1, so nothing reads v1 again.
    let forged = MissionImpactClassificationV1 {
        workspace_id: fixture.workspace_id,
        mission_id: fixture.mission_id,
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project_id,
            policy_revision: fixture.policy_revision,
            charter_revision: fixture.charter_revision,
        },
        rule_id: None,
        reason: "The Mission writes to the Operator's own repository.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::Record("ab".repeat(32)),
    };
    let planted = fixture.plant_classification(&forged);
    fixture.classify_superseding(2, ClassificationSupersedes::Record(planted.clone()));

    // The forgery is in the Workspace `MAX(workspace_id)` does NOT name -- asserted against the
    // store, so the test fails loudly if the ids ever stop ordering this way.
    let skipped: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT MAX(workspace_id) FROM mission_impact_classifications",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        skipped,
        highest.to_string(),
        "the mutant's MAX must name the OTHER Workspace"
    );
    assert_ne!(skipped, fixture.workspace_id.to_string());

    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect_err("a forged link in a second Workspace reopened Ready");
    // The row named is the one in the SKIPPED Workspace, which is the whole point of this test:
    // asserting the variant alone would pass even if the pass had refused the other Workspace's.
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch {
                workspace_id,
                digest,
                cause: ClassificationStoreError::Predecessor {
                    fault: PredecessorFault::NotStored,
                    ..
                },
            } if *workspace_id == fixture.workspace_id && digest == &planted
        ),
        "refused for the wrong reason, or named the wrong Workspace: {refusal:?}"
    );
}

/// One classification recorded through the Command in an arbitrary Workspace.
fn classify_in(
    accounts: &Arc<OperatorAccountService>,
    workspace_id: Uuid,
    admitted: &AdmittedMission,
    version: u32,
    supersedes: ClassificationSupersedes,
) -> String {
    let record = MissionImpactClassificationV1 {
        workspace_id,
        mission_id: admitted.mission_id,
        classification_id: Uuid::new_v4(),
        version,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: admitted.project_id,
            policy_revision: admitted.policy_revision,
            charter_revision: admitted.charter_revision,
        },
        rule_id: None,
        reason: "The Mission writes to the Operator's own repository.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes,
    };
    MissionImpactService::new(Arc::clone(accounts), workspace_id)
        .classify(&record.canonical_bytes().unwrap())
        .expect("the classification records")
        .digest()
        .value
        .clone()
}

// ---------------------------------------------------------------------------------------------
// R32 — the two closure-walk defects astra found (OV1-opus2-report.md). astra's probes enter this
// suite verbatim, names kept, because this is the suite whose fixture chain they were built on.
// ---------------------------------------------------------------------------------------------

/// A bound repository for this fixture's Project, through the bind path rather than a seeded row.
///
/// Only the two R34 spend bars need it: `CheckPreflightService::preflight` refuses
/// `RepositoryNotBound` before writing anything, and what those bars need is a REAL spend-bearing
/// Event. The bound path does not exist, so the preflight refuses -- a real outcome, with a real
/// Receipt and a real `run.preflighted` Event, which is the whole point.
fn bind_repository(f: &Fixture) {
    ProjectRepositoryService::new(Arc::clone(&f.accounts), f.workspace_id)
        .bind(
            f.project_id,
            f.charter_revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: "/home/operator/projects/does-not-exist".to_owned(),
                base_revision: "0".repeat(40),
                declared_checks: vec![DeclaredCheck {
                    name: "cargo-test".to_owned(),
                    argv: vec!["cargo".to_owned(), "test".to_owned()],
                    working_directory: ".".to_owned(),
                }],
            },
        )
        .expect("the repository binds");
}

/// One real spend-bearing Event: the preflight service's own run, receipt and `run.preflighted`,
/// written by the path that writes them in production rather than planted.
fn spend_bearing_run(f: &Fixture) {
    bind_repository(f);
    CheckPreflightService::new(Arc::clone(&f.accounts), f.workspace_id)
        .preflight(f.project_id)
        .expect("the preflight writes a run, a receipt and its Event");
}

/// astra's completion builder, verbatim from its C11-D probe.
fn c11d_completion(
    f: &Fixture,
    spend: SpendReference,
    delivered: Vec<[u8; 32]>,
) -> MissionCompletionV1 {
    MissionCompletionV1 {
        workspace_id: f.workspace_id,
        project_id: f.project_id,
        mission_id: f.mission_id,
        completion_id: Uuid::new_v4(),
        actor: CompletionActor::Operator,
        accountable_resident_id: f.resident_id,
        accountable_resident_configuration_id: f.configuration_id,
        boundary_event_seq: f.latest_event_seq(),
        disposition: CompletionDisposition::Concluded,
        declared_conditions_sha256: declared_conditions_digest(&[CONDITION_TEXT.to_owned()]),
        condition_results: vec![ConditionResultRecord {
            ordinal: 0,
            outcome: ConditionOutcome::Met,
            support: vec![],
            note: None,
        }],
        delivered_artifacts: delivered,
        unfinished_work: vec![],
        uncertainty: vec![],
        spend,
        supersedes: CompletionSupersedes::None,
        summary: "C11-D scratch completion".to_owned(),
        completed_at_ms: 1_788_400_000_000,
    }
}

/// astra's C11-D probe, verbatim (OV1-opus2-report.md, R32.1). It was RED on main because
/// `record_completion` refused every delivery unconditionally; the refusal is gone, and a cited
/// delivery is now verified where the rule already lived — in the collector, at the boundary.
#[test]
fn c11d_recorded_delivery_can_complete_through_its_owner() {
    let f = admitted_mission("c11d-delivery");
    f.classify(ImpactLevel::High);
    let artifact = f
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Recorded support\n",
            ArtifactContext::Mission {
                project_id: f.project_id,
                mission_id: f.mission_id,
            },
            f.resident_id,
            f.configuration_id,
            None,
        )
        .expect("C11D.SETUP.ARTIFACT_RECORDED");
    let record = c11d_completion(
        &f,
        SpendReference::NotMeasured("not measured".into()),
        vec![digest_bytes(&artifact.digest().value)],
    );
    record_completion(&mut f.store.connect(), &record).expect("C11.04.DELIVERED_ARTIFACT_ACCEPTED");
}

/// astra's C11-D probe, verbatim (OV1-opus2-report.md, R32.1/R32.2). It was RED on main: the
/// collector read the record's own tag and never compared the window's end to the cut.
#[test]
fn c11d_short_spend_window_is_partial_in_document() {
    use beacond::{
        MeasuredSpendService, MeasuredSpendV1, SpendLine, SpendMeasurement, SpendMeasurementKind,
        SpendOrigin, SpendSupersedes, SpendTotals,
    };
    let f = admitted_mission("c11d-spend-window");
    f.classify(ImpactLevel::High);
    let spend = MeasuredSpendV1 {
        era: SpendEra::V2,
        workspace_id: f.workspace_id,
        mission_id: f.mission_id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: f.latest_event_seq(),
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                attribution: LineAttribution::unattributed("fixture"),
                unit: "USD".into(),
                amount_minor: 1234,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    };
    let stored = MeasuredSpendService::new(Arc::clone(&f.accounts), f.workspace_id)
        .record(&spend.canonical_bytes().unwrap())
        .expect("C11D.SETUP.SPEND_RECORDED");
    // R34.4: a Run in the gap is what makes this window short in the sense that matters -- not the
    // arithmetic distance to the cut, which R32.2 and R33 each got wrong in turn.
    spend_bearing_run(&f);
    let record = c11d_completion(
        &f,
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: digest_bytes(&stored.digest().value),
        },
        vec![],
    );
    assert!(
        spend.coverage_to_event_seq < record.boundary_event_seq,
        "C11D.SETUP.SHORT_WINDOW"
    );
    record_completion(&mut f.store.connect(), &record).expect("C11D.SETUP.COMPLETION_RECORDED");
    let candidate = f
        .outcomes
        .prepare_for_mission(f.mission_id, AppendixRequest::PolicyDefault)
        .expect("C11D.SETUP.PREPARED");
    let manifest = f.outcomes.manifest(&candidate.manifest_digest).unwrap();
    assert_eq!(
        manifest
            .manifest()
            .spend_ref
            .verified()
            .unwrap()
            .measurement,
        SpendMeasurementKind::PartiallyMeasured,
        "C11.05.SPEND_WINDOW_IS_PARTIAL"
    );
}

/// R34's positive control: `Measured` on the real path -- measure AFTER the last Run, then complete.
///
/// The gap then holds only the spend's own `spend.measured` Event and the completion's, and neither
/// carries spend. Without this, "always partial" passes astra's bar above and R21.3's last sentence
/// ("Absent / partial / complete stay distinct") is gone -- which is what R32.2's and R33's
/// arithmetic each did in turn.
///
/// It deliberately keeps `coverage_to < boundary`: under R34 that is not what makes a window
/// partial, so a control that also closed the arithmetic gap would prove nothing about the rule.
#[test]
fn a_spend_measured_after_the_last_run_reaches_the_manifest_as_measured() {
    use beacond::{
        MeasuredSpendService, MeasuredSpendV1, SpendLine, SpendMeasurement, SpendMeasurementKind,
        SpendOrigin, SpendSupersedes, SpendTotals,
    };
    let f = admitted_mission("r34-measured-after-run");
    f.classify(ImpactLevel::High);
    // The Run comes FIRST: this is the ordering an honest ledger has -- measure what has happened.
    spend_bearing_run(&f);
    let spend = MeasuredSpendV1 {
        era: SpendEra::V2,
        workspace_id: f.workspace_id,
        mission_id: f.mission_id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: f.latest_event_seq(),
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                attribution: LineAttribution::unattributed("fixture"),
                unit: "USD".to_owned(),
                amount_minor: 1234,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    };
    let stored = MeasuredSpendService::new(Arc::clone(&f.accounts), f.workspace_id)
        .record(&spend.canonical_bytes().unwrap())
        .expect("the spend records through its owner");
    let record = c11d_completion(
        &f,
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: digest_bytes(&stored.digest().value),
        },
        vec![],
    );
    assert!(
        spend.coverage_to_event_seq < record.boundary_event_seq,
        "the window must still end before the cut, or this control proves nothing about R34"
    );
    record_completion(&mut f.store.connect(), &record).expect("the completion records");
    let candidate = f
        .outcomes
        .prepare_for_mission(f.mission_id, AppendixRequest::PolicyDefault)
        .expect("the candidate prepares");
    let manifest = f.outcomes.manifest(&candidate.manifest_digest).unwrap();
    assert_eq!(
        manifest
            .manifest()
            .spend_ref
            .verified()
            .unwrap()
            .measurement,
        SpendMeasurementKind::Measured,
        "nothing spend-bearing is in the gap, so the measurement covers the Mission"
    );
}

/// Every kind in the closed list is witnessed, not just the one astra's bar happens to plant.
///
/// Fable's mutant on `8a60df2`: remove `"run.executed"` from `SPEND_BEARING_EVENT_KINDS` and the
/// whole suite stays green. Two of the three kinds were claims nothing could see -- the same shape
/// as the single-Workspace survivor on R21.5a, where a list was right and only one entry was
/// exercised.
///
/// It iterates the REAL constant rather than a copy. That is the mechanism, not a detail: a fourth
/// kind added to the list arrives here with no fixture and fails loudly, where a copied list would
/// have kept passing and said nothing.
///
/// The Events are planted rather than produced. `run.preflighted` has a cheap production path and
/// astra's bar uses it; `run.executed` and `proposal.dispatched` need a real worktree, a real
/// Ruling and a real dispatch, and what is under test here is the collector's interval query --
/// which kinds it counts -- not the producers, which the constant's doc comment cites by file.
#[test]
fn every_spend_bearing_kind_makes_a_window_partial() {
    use beacond::{
        MeasuredSpendService, MeasuredSpendV1, SpendCompleteness, SpendLine, SpendMeasurement,
        SpendMeasurementKind, SpendOrigin, SpendSupersedes, SpendTotals,
    };
    // Iterating the real constant makes an ADDITION fail loudly -- a new kind arrives here with no
    // fixture. It cannot make a REMOVAL fail, because the table shrinks with the list: Fable's
    // mutant deleted `run.executed` and this test simply stopped testing it. So the membership is
    // pinned too. This is not a tautology restating the constant: it is the gate that makes any
    // change to a closed, spend-relevant vocabulary deliberate, and it names where each kind is
    // produced so the next person edits both.
    assert_eq!(
        beacond::SPEND_BEARING_EVENT_KINDS,
        [
            "run.preflighted",     // check_preflight.rs, beside its INSERT INTO receipts
            "run.executed",        // declared_checks.rs, likewise
            "proposal.dispatched", // dispatch.rs, likewise
        ],
        "the spend-bearing kinds changed. Every kind written beside an `INSERT INTO receipts` \
         belongs here and nothing else does; adding or removing one changes which documents \
         report complete coverage, so it is a ruling, not an edit."
    );
    for kind in beacond::SPEND_BEARING_EVENT_KINDS {
        let f = admitted_mission(&format!("r34-kind-{}", kind.replace('.', "-")));
        f.classify(ImpactLevel::High);
        let spend = MeasuredSpendV1 {
            era: SpendEra::V2,
            workspace_id: f.workspace_id,
            mission_id: f.mission_id,
            spend_id: Uuid::new_v4(),
            version: 1,
            origin: SpendOrigin::DaemonLedger,
            measured_at_ms: 1_788_400_000_000,
            coverage_from_event_seq: 1,
            coverage_to_event_seq: f.latest_event_seq(),
            measurement: SpendMeasurement::Measured(SpendTotals {
                counted_units: 1,
                lines: vec![SpendLine {
                    attribution: LineAttribution::unattributed("fixture"),
                    unit: "USD".to_owned(),
                    amount_minor: 1234,
                    minor_units_exponent: 2,
                }],
            }),
            supersedes: SpendSupersedes::None,
        };
        let stored = MeasuredSpendService::new(Arc::clone(&f.accounts), f.workspace_id)
            .record(&spend.canonical_bytes().unwrap())
            .expect("the spend records through its owner");

        // Into the gap, after the spend's own Event and before the cut.
        f.store
            .connect()
            .execute(
                "INSERT INTO events (
                 workspace_id, kind, payload_json, occurred_at_ms,
                 predecessor_kind, predecessor_digest, link_digest
             )
                 VALUES (?1, ?2, '{}', 9000, 'unchained', NULL, NULL)",
                params![f.workspace_id.to_string(), kind],
            )
            .expect("the Event plants");
        let planted = f.latest_event_seq();

        let record = c11d_completion(
            &f,
            SpendReference::Recorded {
                spend_id: spend.spend_id,
                spend_version: spend.version,
                spend_sha256: digest_bytes(&stored.digest().value),
            },
            vec![],
        );
        assert!(
            spend.coverage_to_event_seq < planted && planted <= record.boundary_event_seq,
            "{kind} did not land in (coverage_to, boundary], so this case proves nothing"
        );
        let digest =
            record_completion(&mut f.store.connect(), &record).expect("the completion records");

        // The collector names it: kind and sequence, not just "the window was short".
        let connection = f.store.connect();
        let collected = Collector::new(&connection, f.workspace_id)
            .collect(&CollectionRequest {
                completion_digest: Some(digest),
                ..CollectionRequest::default()
            })
            .expect("the completion collects");
        let completeness = collected
            .spend
            .expect("the completion cited spend")
            .completeness;
        match &completeness {
            SpendCompleteness::Partial(gaps) => {
                assert_eq!(gaps.len(), 1, "{kind}: {gaps:?}");
                assert!(
                    gaps[0].detail.contains(*kind) && gaps[0].detail.contains(&planted.to_string()),
                    "{kind}: the gap does not name the Event that caused it: {:?}",
                    gaps[0]
                );
            }
            other => panic!("{kind} did not make the window partial: {other:?}"),
        }
        drop(connection);

        // ...and the document says so.
        let candidate = f
            .outcomes
            .prepare_for_mission(f.mission_id, AppendixRequest::PolicyDefault)
            .expect("the candidate prepares");
        let manifest = f.outcomes.manifest(&candidate.manifest_digest).unwrap();
        assert_eq!(
            manifest
                .manifest()
                .spend_ref
                .verified()
                .unwrap()
                .measurement,
            SpendMeasurementKind::PartiallyMeasured,
            "{kind} is in the closed list but does not reach the document"
        );
    }
}

// Export: the stored bytes, copied (slice 4a, R29.1/R29.3)
// ---------------------------------------------------------------------------------------------

/// MEASUREMENT for R29.3, run before the bar was named: does the read path accept a candidate whose
/// stored bytes are not what the current assembler would produce?
///
/// It does, under one condition the store already enforces: `verified_manifest_within` requires the
/// candidate's content to CONTAIN the manifest's canonical bytes as hex
/// (`candidate_proves_manifest`), and that check is on the `get` path, not only at startup. So a
/// candidate of arbitrary bytes is refused, and a candidate that keeps the appendix and differs
/// everywhere else is accepted. The plant below is the second kind, which is enough: the assembler
/// renders one exact document for a manifest and would never emit this one.
#[test]
fn exported_bytes_are_the_stored_candidate_bytes_byte_for_byte() {
    let fixture = admitted_mission("export-planted-candidate");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    let rendered = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap()
        .content()
        .to_vec();

    // The planted document: the rendered bytes with a line the renderer would never emit, in front.
    // The verification appendix rides along untouched, so the manifest binding still holds.
    let mut planted_content = b"<!-- planted: not what the assembler renders -->\n".to_vec();
    planted_content.extend_from_slice(&rendered);
    assert_ne!(planted_content, rendered);
    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &planted_content,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the planted candidate records");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);

    // Repoint the manifest and the publication at the planted candidate, past the Command, with the
    // immutability triggers down -- the shape a restore or an older binary produces.
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![planted.digest().value, fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications
                     SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                     WHERE workspace_id = ?3",
                    params![
                        planted.digest().value,
                        planted_evaluation.digest().value,
                        fixture.workspace_id.to_string()
                    ],
                )
                .unwrap();
        },
    );

    let exported = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect("the planted publication reads back, so the plant idiom holds here");
    assert_eq!(
        exported.bytes(),
        planted_content.as_slice(),
        "export did not return the STORED bytes"
    );
    assert!(
        exported
            .bytes()
            .starts_with(b"<!-- planted: not what the assembler renders -->"),
        "export returned bytes the assembler would produce, not the ones the store holds"
    );
    assert_eq!(exported.digest().value, planted.digest().value);
    assert_eq!(exported.media_type(), "text/markdown; charset=utf-8");
}

/// A superseded version exports at its own number, and says a newer one exists (R24.2).
#[test]
fn a_superseded_version_exports_at_its_own_number_and_says_a_newer_exists() {
    let fixture = admitted_mission("export-superseded-version");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (first_candidate, first_digest, first_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("version 1 publishes");
    let first_bytes = fixture
        .artifacts()
        .get(&first_candidate.candidate_artifact_digest)
        .unwrap()
        .content()
        .to_vec();

    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("the correction publishes");

    // Version 1, at its own number, with the count saying it is not the only one.
    let superseded = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect("a superseded version still exports");
    assert_eq!(superseded.publication_version(), 1);
    assert_eq!(
        superseded.publication_version_count(),
        2,
        "a superseded export that reports 1 of 1 reads as the only document there is"
    );
    assert_eq!(superseded.bytes(), first_bytes.as_slice());
    assert_eq!(superseded.digest().value, first_digest.value);

    // And the current export is version 2 of 2, with different bytes.
    let current = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect("the current version exports");
    assert_eq!(
        (
            current.publication_version(),
            current.publication_version_count()
        ),
        (2, 2)
    );
    assert_ne!(current.bytes(), superseded.bytes());
}

/// R29.6: a Mission in another Workspace exports as `Absent`, deliberately indistinguishable from
/// "this Mission has no publication" -- so a caller who may not read this Workspace is not told
/// whether the record exists.
#[test]
fn a_mission_in_another_workspace_exports_as_absent_so_existence_is_not_confirmed() {
    let fixture = admitted_mission("export-cross-workspace");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the Mission publishes in its own Workspace");

    // A service scoped to a Workspace that does not hold this Mission.
    let elsewhere = OutcomeService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    let refusal = elsewhere
        .export_markdown(fixture.mission_id)
        .expect_err("another Workspace exported this Mission's document");
    assert!(
        matches!(refusal, PublishRefusal::Absent),
        "R42S1.OPACITY.EXPORT.FOREIGN_VARIANT: {refusal:?}"
    );

    // The SAME answer a Mission with no publication gives, which is the point: the two are not
    // distinguishable, so the refusal confirms nothing.
    let unpublished = admitted_mission("export-cross-workspace-unpublished");
    let never = fixture_outcomes(&unpublished)
        .export_markdown(unpublished.mission_id)
        .expect_err("an unpublished Mission exported");
    assert!(
        matches!(never, PublishRefusal::Absent),
        "R42S1.OPACITY.EXPORT.UNPUBLISHED_VARIANT: {never:?}"
    );

    // R42: each line against the independently written expected text BEFORE they are compared to
    // each other. Two equally wrong strings are equal too, and equality alone would pass.
    assert_eq!(
        refusal.to_string(),
        "this Mission has no published Outcome",
        "R42S1.OPACITY.EXPORT.FOREIGN_LINE"
    );
    assert_eq!(
        never.to_string(),
        "this Mission has no published Outcome",
        "R42S1.OPACITY.EXPORT.UNPUBLISHED_LINE"
    );
    assert_eq!(
        format!("{refusal}"),
        format!("{never}"),
        "R42S1.OPACITY.EXPORT.SAME_LINE"
    );

    // And the history is empty rather than a refusal, for the same reason.
    assert!(
        elsewhere
            .publication_history(fixture.mission_id)
            .expect("a cross-Workspace history is empty, not a refusal")
            .is_empty()
    );
}

/// A Mission with no publication exports as `Absent`.
#[test]
fn a_mission_with_no_publication_exports_as_absent() {
    let fixture = admitted_mission("export-absent");
    let refusal = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect_err("a Mission with no publication exported");
    assert!(matches!(refusal, PublishRefusal::Absent), "{refusal:?}");
    let versioned = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect_err("version 1 of nothing exported");
    assert!(matches!(versioned, PublishRefusal::Absent), "{versioned:?}");
}

/// The one condition the plant must respect, measured rather than assumed.
///
/// kimi's read lists what `read_publication_within` proves and concludes a self-consistent plant
/// passes. It does -- with one more constraint than that list carries: `verified_manifest_within`
/// requires the candidate's content to CONTAIN the manifest's canonical bytes as hex
/// (`candidate_proves_manifest`), and that check is on the `get` path, not only at startup. A
/// candidate of arbitrary bytes is therefore refused BY NAME, and the plant above works because it
/// keeps the appendix and differs everywhere else. Recorded here so the next person building a
/// plant against this path knows where the wall is.
#[test]
fn a_planted_candidate_that_drops_the_verification_appendix_is_refused_by_name() {
    let fixture = admitted_mission("export-plant-without-appendix");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    // Arbitrary bytes: a real Artifact, really evaluated, that simply is not this manifest's
    // document.
    let unrelated = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Not this manifest's document.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the unrelated Artifact records");
    let unrelated_evaluation = fixture.evaluate(&unrelated.digest().value, Verdict::Meets);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![unrelated.digest().value, fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications
                     SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                     WHERE workspace_id = ?3",
                    params![
                        unrelated.digest().value,
                        unrelated_evaluation.digest().value,
                        fixture.workspace_id.to_string()
                    ],
                )
                .unwrap();
        },
    );

    let refusal = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect_err("a candidate that does not carry its manifest's appendix exported");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees {
                column: "candidate_artifact_digest"
            })
        ),
        "{refusal:?}"
    );
}

/// blue's access bar: export without an authenticated session is refused, and NOT as `Absent`.
///
/// The two refusals must stay distinguishable in this direction: a locked store that answered
/// `Absent` would tell an unauthenticated caller that the Mission has no publication, which is a
/// claim about the store's contents. `Absent` is the right answer for a Workspace the caller may
/// not read (R29.6); it is the wrong answer for a caller who has not authenticated at all.
#[test]
fn export_without_an_authenticated_session_is_refused_and_not_as_absent() {
    let fixture = admitted_mission("export-locked");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes while unlocked");
    assert!(fixture.outcomes.export_markdown(fixture.mission_id).is_ok());

    fixture.accounts.lock().unwrap();

    // And it is the ACCOUNT's refusal, carried typed rather than flattened, so the transport can
    // map it to `NotAuthenticated` instead of a protocol fault.
    assert!(
        matches!(
            fixture
                .outcomes
                .export_markdown(fixture.mission_id)
                .expect_err("a locked store exported"),
            PublishRefusal::Account(OperatorAccountError::NotAuthenticated)
        ),
        "the account refusal did not survive as itself"
    );
    for refusal in [
        fixture
            .outcomes
            .export_markdown(fixture.mission_id)
            .expect_err("a locked store exported"),
        fixture
            .outcomes
            .export_markdown_version(fixture.mission_id, 1)
            .expect_err("a locked store exported a version"),
    ] {
        assert!(
            !matches!(refusal, PublishRefusal::Absent),
            "a locked store answered `Absent`, which is a claim about its contents: {refusal:?}"
        );
    }
    let history = fixture
        .outcomes
        .publication_history(fixture.mission_id)
        .expect_err("a locked store listed the history");
    assert!(!matches!(history, PublishRefusal::Absent), "{history:?}");
}

fn fixture_outcomes(fixture: &Fixture) -> OutcomeService {
    OutcomeService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
}

// ---------------------------------------------------------------------------------------------
// Publication history: a reader over the relation's own key (slice 4a, R29.5)
// ---------------------------------------------------------------------------------------------

/// The history lists every version, newest first, with the current one marked.
#[test]
fn publication_history_lists_every_version_newest_first_with_the_current_one_marked() {
    let fixture = admitted_mission("history-lists-versions");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, first_digest, first_evaluation) = fixture.publishable();
    let first = fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("version 1 publishes");

    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    let second = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("the correction publishes");

    let history = fixture
        .outcomes
        .publication_history(fixture.mission_id)
        .expect("the history reads");
    assert_eq!(history.len(), 2);
    // Newest first.
    assert_eq!(history[0].publication_version, 2);
    assert_eq!(history[1].publication_version, 1);
    assert!(history[0].is_current);
    assert!(
        !history[1].is_current,
        "a superseded version marked current reads as the only document there is"
    );
    // Each row carries what the RELATION holds, and it agrees with what `get_version` proves.
    assert_eq!(history[0].publication_id, second.publication_id());
    assert_eq!(history[1].publication_id, first.publication_id());
    assert_eq!(
        history[1].candidate_artifact_digest,
        first_digest.value.clone()
    );
    assert_eq!(history[1].evaluation_digest, first_evaluation.value.clone());
    assert_eq!(
        history[0].candidate_artifact_digest,
        corrected_record.digest().value
    );

    // A Mission with no publication has an empty history, not a refusal.
    let none = admitted_mission("history-empty");
    assert!(
        none.outcomes
            .publication_history(none.mission_id)
            .expect("an unpublished Mission has an empty history")
            .is_empty()
    );
}

/// R29.5, as behaviour rather than as a stopwatch: the history is a reader over the RELATION, so a
/// version whose document no longer verifies is still a version.
///
/// This is the bar that kills a `get_version` loop, and it kills it for the right reason. A loop
/// re-proves every document, so one unreadable version makes the whole list refuse -- the operator
/// who most needs to see that a version exists is exactly the one who cannot. Counting statements
/// would have needed rusqlite's `trace` feature, which is a dependency change (R29.1), and a
/// wall-clock ratio would be a flake waiting to happen. This asserts the difference that matters.
#[test]
fn the_history_lists_a_version_whose_document_no_longer_verifies() {
    let fixture = admitted_mission("history-unverifiable-version");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, first_digest, first_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("version 1 publishes");

    // Break version 1's document: repoint the manifest at an Artifact that does not carry its
    // appendix, which the read path refuses by name (proved by the export bar above).
    let unrelated = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Not this manifest's document.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the unrelated Artifact records");
    let unrelated_evaluation = fixture.evaluate(&unrelated.digest().value, Verdict::Meets);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![unrelated.digest().value, fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications
                     SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                     WHERE workspace_id = ?3",
                    params![
                        unrelated.digest().value,
                        unrelated_evaluation.digest().value,
                        fixture.workspace_id.to_string()
                    ],
                )
                .unwrap();
        },
    );

    // The document does NOT read -- that is the premise, asserted so this test cannot pass because
    // the break failed to take.
    assert!(
        fixture
            .outcomes
            .export_markdown_version(fixture.mission_id, 1)
            .is_err(),
        "the version was supposed to be unreadable"
    );

    // The history still lists it, because the relation still holds it.
    let history = fixture
        .outcomes
        .publication_history(fixture.mission_id)
        .expect("a version whose document is broken is still a version");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].publication_version, 1);
    assert!(history[0].is_current);
    assert_eq!(
        history[0].candidate_artifact_digest,
        unrelated.digest().value
    );
}

#[test]
fn sol1_history_refuses_a_publication_version_outside_the_exportable_domain() {
    let fixture = admitted_mission("history-version-out-of-range");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("version 1 publishes");

    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            let changed = connection
                .execute(
                    "UPDATE outcome_publications SET publication_version = 4294967296
                     WHERE workspace_id = ?1 AND mission_id = ?2",
                    params![
                        fixture.workspace_id.to_string(),
                        fixture.mission_id.to_string()
                    ],
                )
                .unwrap();
            assert_eq!(changed, 1, "the out-of-domain plant did not take");
        },
    );

    let result = fixture.outcomes.publication_history(fixture.mission_id);
    assert!(
        matches!(result, Err(PublishRefusal::Unverified(_))),
        "an out-of-domain relation version was relabelled as an exportable version: {result:?}"
    );
}

#[test]
fn sol1_transport_preserves_nul_and_lone_cr_and_guards_the_session_and_input_type() {
    let fixture = admitted_mission("transport-byte-identity");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("version 1 publishes");
    let other_mission = sol1_admit_other_mission(&fixture, "Another Mission");
    let original = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap();
    let mut planted_bytes = vec![0, b'\r'];
    planted_bytes.extend_from_slice(original.content());
    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &planted_bytes,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the byte probe records");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            assert_eq!(
                connection
                    .execute(
                        "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                         WHERE workspace_id = ?2 AND mission_id = ?3",
                        params![
                            planted.digest().value,
                            fixture.workspace_id.to_string(),
                            fixture.mission_id.to_string()
                        ],
                    )
                    .unwrap(),
                1
            );
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            assert_eq!(
                connection
                    .execute(
                        "UPDATE outcome_publications
                         SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                         WHERE workspace_id = ?3 AND mission_id = ?4",
                        params![
                            planted.digest().value,
                            planted_evaluation.digest().value,
                            fixture.workspace_id.to_string(),
                            fixture.mission_id.to_string()
                        ],
                    )
                    .unwrap(),
                1
            );
        },
    );

    let root = std::env::temp_dir().join(format!("beacond-sol1-s4a-{}", Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("beacond.sock");
    let token = root.join("beacond.token");
    let lock = root.join("beacond.lock");
    initialize_local_auth_token(&token).unwrap();
    let server = LocalSupervisorServer::bind(LocalServerConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
        socket_path: socket.clone(),
        auth_token_path: token.clone(),
        lock_path: lock,
        local_model_endpoint: LocalModelEndpoint::ollama_default(),
    })
    .unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&socket, &token).unwrap();

    assert!(matches!(
        client.outcome_document(fixture.mission_id, None),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    let current = client
        .outcome_document(fixture.mission_id, None)
        .expect("current export crosses the real socket");
    let version = client
        .outcome_document(fixture.mission_id, Some(1))
        .expect("versioned export crosses the real socket");
    assert_eq!(current.bytes(), planted_bytes.as_slice());
    assert_eq!(version.bytes(), planted_bytes.as_slice());
    assert_eq!(current.digest(), planted.digest());
    let history = client.mission_publications(fixture.mission_id).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].candidate_artifact_digest, planted.digest().value);
    // ADOPTED PROBE, ONE ASSERTION CHANGED, deliberately: sol1 wrote this against my item-3
    // mapping, which reused `MissionNotFound` for `Absent`. Fold 3 (blue's P1) gave the Outcome
    // refusals their own codes, so `Absent` is now `OutcomeAbsent` -- and that is the point of the
    // change: `MissionNotFound` says the Mission is gone, while `Absent` means it may well exist
    // and have no publication. What sol1's probe actually pins is unchanged and still holds: one
    // code covers a version that does not exist AND another Mission's version, so nothing here
    // tells a caller which of the two it hit (R29.6).
    for (mission_id, version) in [
        (fixture.mission_id, 0),
        (fixture.mission_id, 2),
        (other_mission, 1),
    ] {
        assert!(matches!(
            client.outcome_document(mission_id, Some(version)),
            Err(LocalTransportError::Remote {
                code: LocalErrorCode::OutcomeAbsent,
                ..
            })
        ));
    }

    let encoded_token = fs::read_to_string(&token).unwrap();
    let malformed = format!(
        "{{\"protocol\":1,\"token\":\"{encoded_token}\",\"command\":{{\"name\":\"outcome_document\",\"mission_id\":\"{}\",\"publication_version\":-1}}}}",
        fixture.mission_id
    );
    let mut raw = UnixStream::connect(&socket).unwrap();
    raw.write_all(malformed.as_bytes()).unwrap();
    raw.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    raw.read_to_string(&mut response).unwrap();
    assert!(
        response.contains("\"code\":\"protocol_error\""),
        "{response}"
    );
    assert_eq!(
        client.status().unwrap().phase,
        OperatorSessionPhase::Unlocked
    );

    client.lock().unwrap();
    assert!(matches!(
        client.mission_publications(fixture.mission_id),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));
    running.store(false, Ordering::Release);
    server_thread.join().unwrap().unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sol1_invalid_utf8_outcome_artifact_is_refused_before_export() {
    let fixture = admitted_mission("export-invalid-utf8");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .unwrap();
    let original = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap();
    let mut invalid = vec![0xff];
    invalid.extend_from_slice(original.content());
    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &invalid,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the Artifact store accepts opaque bytes");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            assert_eq!(
                connection
                    .execute(
                        "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                         WHERE workspace_id = ?2 AND mission_id = ?3",
                        params![
                            planted.digest().value,
                            fixture.workspace_id.to_string(),
                            fixture.mission_id.to_string()
                        ],
                    )
                    .unwrap(),
                1
            );
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            assert_eq!(
                connection
                    .execute(
                        "UPDATE outcome_publications
                         SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                         WHERE workspace_id = ?3 AND mission_id = ?4",
                        params![
                            planted.digest().value,
                            planted_evaluation.digest().value,
                            fixture.workspace_id.to_string(),
                            fixture.mission_id.to_string()
                        ],
                    )
                    .unwrap(),
                1
            );
        },
    );

    let refusal = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect_err("invalid UTF-8 crossed the Markdown export boundary");
    assert!(matches!(
        refusal,
        PublishRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees {
            column: "candidate_artifact_digest"
        })
    ));
}

/// The history and the export are readers over ONE Mission's rows, and this is the test that can
/// tell.
///
/// Fable's mutant relaxed `publication_history_within`'s filter to
/// `(mission_id = ?2 OR 1 = 1)` and the whole suite stayed green: every other fixture publishes one
/// Mission per Workspace, so "a reader over the relation's own key" was a claim nothing could see. A
/// store with two published Missions would have listed the other Mission's versions under this one,
/// with digests and an `is_current` belonging to someone else.
///
/// Two Missions in ONE Workspace, with DIFFERENT version counts (a: 1, b: 2) so a leak cannot hide
/// behind matching shapes.
#[test]
fn the_history_and_the_export_read_one_missions_rows_and_not_another_missions() {
    let fixture = admitted_mission("two-mission-history");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, a_digest, a_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &a_digest, &a_evaluation)
        .expect("Mission a publishes version 1");
    let a_bytes = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect("Mission a exports")
        .bytes()
        .to_vec();

    // A SECOND Mission in the SAME Workspace, published twice.
    let b_fixture = fixture.admit_second_mission("Ship a second reviewed Beacon feature.");
    b_fixture.classify(ImpactLevel::Ordinary);
    b_fixture.complete((b_fixture.resident_id, b_fixture.configuration_id));
    let (_, b_first_digest, b_first_evaluation) = b_fixture.publishable();
    b_fixture
        .outcomes
        .publish(b_fixture.mission_id, &b_first_digest, &b_first_evaluation)
        .expect("Mission b publishes version 1");
    let b_corrected = b_fixture
        .outcomes
        .prepare_for_mission(b_fixture.mission_id, AppendixRequest::Include)
        .expect("Mission b's correction prepares");
    let b_corrected_record = b_fixture
        .artifacts()
        .get(&b_corrected.candidate_artifact_digest)
        .unwrap();
    let b_corrected_evaluation =
        b_fixture.evaluate(&b_corrected.candidate_artifact_digest, Verdict::Meets);
    b_fixture
        .outcomes
        .publish(
            b_fixture.mission_id,
            b_corrected_record.digest(),
            b_corrected_evaluation.digest(),
        )
        .expect("Mission b publishes version 2");

    // Mission a: ONE version, a's digests, a's count.
    let a_history = fixture
        .outcomes
        .publication_history(fixture.mission_id)
        .expect("Mission a's history reads");
    assert_eq!(
        a_history.len(),
        1,
        "Mission a's history carried another Mission's versions"
    );
    assert_eq!(a_history[0].publication_version, 1);
    assert!(a_history[0].is_current);
    assert_eq!(a_history[0].candidate_artifact_digest, a_digest.value);
    assert_eq!(a_history[0].evaluation_digest, a_evaluation.value);

    // Mission b: TWO versions, b's digests.
    let b_history = b_fixture
        .outcomes
        .publication_history(b_fixture.mission_id)
        .expect("Mission b's history reads");
    assert_eq!(b_history.len(), 2);
    assert_eq!(
        (
            b_history[0].publication_version,
            b_history[1].publication_version
        ),
        (2, 1)
    );
    assert_eq!(
        b_history[0].candidate_artifact_digest,
        b_corrected_record.digest().value
    );
    assert_eq!(b_history[1].candidate_artifact_digest, b_first_digest.value);
    // No row of one Mission's history belongs to the other.
    for row in &b_history {
        assert_ne!(row.candidate_artifact_digest, a_digest.value);
    }

    // And export: Mission a has no version 2, even though the Workspace does.
    let missing = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 2)
        .expect_err("Mission a exported a version only Mission b has");
    assert!(matches!(missing, PublishRefusal::Absent), "{missing:?}");
    // Mission a's current export is still a's own bytes, and the count is a's.
    let a_current = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect("Mission a still exports");
    assert_eq!(a_current.bytes(), a_bytes.as_slice());
    assert_eq!(
        (
            a_current.publication_version(),
            a_current.publication_version_count()
        ),
        (1, 1),
        "Mission a's count counted another Mission's publications"
    );
}

// ---------------------------------------------------------------------------------------------
// Slice 4b item 7 — the PDF reaches the service (R30.3)
// ---------------------------------------------------------------------------------------------

/// The derivative bar (R30.3): the export is the source, the PDF is a function of it, and the PDF
/// is stored nowhere.
///
/// "Delete the derivative and regenerate" is the property, and it is asserted the only way it can
/// be: the returned buffer is dropped, the export is read again and is unchanged, and a second
/// render is byte-identical. There is nothing on disk to delete, which is the point -- the service
/// stores no PDF, so a cache cannot go stale against a profile change.
#[test]
fn the_markdown_export_is_unchanged_and_the_pdf_regenerates_from_it() {
    let fixture = admitted_mission("pdf-service-derivative");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    let before = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect("the export reads");
    let first = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect("the document renders");
    assert!(!first.bytes().is_empty(), "an empty PDF is not a render");
    assert_eq!(first.publication_version(), 1);
    assert_eq!(first.publication_version_count(), 1);
    assert_eq!(first.profile(), "beacon-outcome-1");
    assert_eq!(first.candidate_digest(), before.digest());

    // Drop the derivative. Nothing was stored, so there is nothing to delete.
    let first_bytes = first.into_bytes();

    let after = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect("the export still reads");
    assert_eq!(
        before.bytes(),
        after.bytes(),
        "rendering a PDF changed the canonical export"
    );
    let second = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect("the document renders again");
    assert_eq!(
        first_bytes,
        second.bytes(),
        "the same document rendered to different bytes on a second call"
    );

    // And it is the render of THOSE bytes, computed here rather than taken on trust.
    assert_eq!(
        first_bytes,
        beacond::render_pdf(
            after.bytes(),
            "beacon.outcome.md.v1",
            &after.digest().value,
            &beacond::BEACON_OUTCOME_1,
        )
        .expect("the exported bytes render"),
        "the service's PDF is not the render of the exported bytes"
    );
}

/// The bytes the PDF renders are the STORED ones, not a fresh assembly (R30.3, mutant 1).
///
/// Byte equality on an ordinary publication cannot show this: re-assembling the same manifest emits
/// the same Markdown, so a service that quietly re-rendered would pass. It needs a stored candidate
/// the current assembler would not produce, which is 4a's plant idiom
/// (`exported_bytes_are_the_stored_candidate_bytes_byte_for_byte`) — the appendix rides along
/// untouched so `candidate_proves_manifest` still holds, and everything else may differ.
///
/// The planted line is APPENDED and is ordinary v1 prose, deliberately. 4a's plant prepends an HTML
/// comment, which the translator refuses on the unescaped `<` — that would also kill the mutant,
/// but through a refusal, and a bar that proves its point by failing to render proves less than one
/// that renders two different documents and says which it got.
#[test]
fn the_pdf_is_rendered_from_the_exported_bytes_not_a_fresh_render() {
    let fixture = admitted_mission("pdf-service-planted");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    let rendered = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap()
        .content()
        .to_vec();
    let mut planted_content = rendered.clone();
    planted_content.extend_from_slice(b"\nA line the assembler would never emit.\n");
    assert_ne!(planted_content, rendered);

    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &planted_content,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the planted candidate records");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);

    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![planted.digest().value, fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications
                     SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                     WHERE workspace_id = ?3",
                    params![
                        planted.digest().value,
                        planted_evaluation.digest().value,
                        fixture.workspace_id.to_string()
                    ],
                )
                .unwrap();
        },
    );

    // The control: the store really does hold bytes the assembler would not emit.
    let exported = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect("the planted publication reads back");
    assert_eq!(exported.bytes(), planted_content.as_slice());
    assert_ne!(
        exported.bytes(),
        rendered.as_slice(),
        "the plant did not take, so this proves nothing"
    );

    let served = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect("the planted document renders");

    // What a service that re-rendered would have produced.
    let from_a_fresh_assembly = beacond::render_pdf(
        &rendered,
        "beacon.outcome.md.v1",
        &planted.digest().value,
        &beacond::BEACON_OUTCOME_1,
    )
    .expect("the assembler's own bytes render too");

    assert_ne!(
        served.bytes(),
        from_a_fresh_assembly.as_slice(),
        "the service rendered the assembler's bytes, not the ones the store holds"
    );
    assert_eq!(
        served.bytes(),
        beacond::render_pdf(
            exported.bytes(),
            "beacon.outcome.md.v1",
            &exported.digest().value,
            &beacond::BEACON_OUTCOME_1,
        )
        .expect("the exported bytes render")
        .as_slice(),
        "the service's PDF is not the render of the exported bytes"
    );
}

/// The access check is export's, so export's bars have to hold through this method too (row 3).
///
/// Re-run rather than assumed: `render_pdf_version` adds no authorization of its own, and the only
/// way to say that truthfully is to ask it the same two questions 4a asks `export_markdown`.
#[test]
fn a_mission_in_another_workspace_renders_no_pdf_and_says_only_absent() {
    let fixture = admitted_mission("pdf-service-cross-workspace");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the Mission publishes in its own Workspace");
    assert!(
        fixture
            .outcomes
            .render_pdf_version(fixture.mission_id, 1)
            .is_ok()
    );

    let elsewhere = OutcomeService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    let refusal = elsewhere
        .render_pdf_version(fixture.mission_id, 1)
        .expect_err("another Workspace rendered this Mission's document");
    assert!(
        matches!(refusal, PublishRefusal::Absent),
        "R42S1.OPACITY.PDF.FOREIGN_VARIANT: {refusal:?}"
    );

    // Indistinguishable from a Mission that simply has no publication, which is why the refusal
    // confirms nothing about what the other Workspace holds (R29.6).
    let unpublished = admitted_mission("pdf-service-cross-workspace-unpublished");
    let never = unpublished
        .outcomes
        .render_pdf_version(unpublished.mission_id, 1)
        .expect_err("an unpublished Mission rendered");
    assert!(
        matches!(never, PublishRefusal::Absent),
        "R42S1.OPACITY.PDF.UNPUBLISHED_VARIANT: {never:?}"
    );

    // R42: the refusal is kept rather than consumed inside `matches!`, so both lines can be held
    // to the expected text before they are held to each other.
    assert_eq!(
        refusal.to_string(),
        "this Mission has no published Outcome",
        "R42S1.OPACITY.PDF.FOREIGN_LINE"
    );
    assert_eq!(
        never.to_string(),
        "this Mission has no published Outcome",
        "R42S1.OPACITY.PDF.UNPUBLISHED_LINE"
    );
    assert_eq!(
        format!("{refusal}"),
        format!("{never}"),
        "R42S1.OPACITY.PDF.SAME_LINE"
    );
}

#[test]
fn rendering_without_an_authenticated_session_is_refused_and_not_as_absent() {
    let fixture = admitted_mission("pdf-service-locked");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the pairing publishes while unlocked");
    assert!(
        fixture
            .outcomes
            .render_pdf_version(fixture.mission_id, 1)
            .is_ok()
    );

    fixture.accounts.lock().unwrap();

    // The ACCOUNT's refusal, carried typed rather than flattened into `Absent` -- a locked store
    // must not look like a Mission that has nothing.
    assert!(
        matches!(
            fixture
                .outcomes
                .render_pdf_version(fixture.mission_id, 1)
                .expect_err("a locked store rendered"),
            PublishRefusal::Account(OperatorAccountError::NotAuthenticated)
        ),
        "a locked store's refusal was not the account's"
    );
}

/// A stored document the translator cannot read is a refusal BY NAME, never an empty PDF (R30.3).
#[test]
fn a_document_the_translator_cannot_read_is_a_named_refusal_not_an_empty_pdf() {
    let fixture = admitted_mission("pdf-service-untranslatable");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    // 4a's plant, with ITS bytes: an HTML comment the renderer would never emit. `escape_v1`
    // escapes `<`, so an unescaped one is a construct outside the v1 shape.
    let rendered = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap()
        .content()
        .to_vec();
    let mut planted_content = b"<!-- planted: not what the assembler renders -->\n".to_vec();
    planted_content.extend_from_slice(&rendered);
    let planted = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &planted_content,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the planted candidate records");
    let planted_evaluation = fixture.evaluate(&planted.digest().value, Verdict::Meets);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET candidate_artifact_digest = ?1
                     WHERE workspace_id = ?2",
                    params![planted.digest().value, fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );
    with_trigger_down(
        &fixture.store,
        "outcome_publications_no_update",
        "outcome_publications",
        "Outcome publications are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_publications
                     SET candidate_artifact_digest = ?1, evaluation_digest = ?2
                     WHERE workspace_id = ?3",
                    params![
                        planted.digest().value,
                        planted_evaluation.digest().value,
                        fixture.workspace_id.to_string()
                    ],
                )
                .unwrap();
        },
    );

    // The export still succeeds: the bytes are the store's, and export never reads them.
    assert!(
        fixture
            .outcomes
            .export_markdown_version(fixture.mission_id, 1)
            .is_ok()
    );

    let refusal = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect_err("an untranslatable document produced a PDF");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Pdf(beacond::PdfRefusal::Translate(
                beacond::TypstRefusal::UnescapedConstruct { character: '<', .. }
            ))
        ),
        "the renderer's refusal was flattened or renamed: {refusal:?}"
    );
}

/// The PDF is its version's, not the newest one's, and not version 1's.
///
/// Fable's mutant at `5f12f68`: pin `get_version_within`'s version argument to `1` and all five
/// item-7 bars stay green — every one of them publishes once and asks for version 1, so the
/// argument was never keyed. On a versioned API that is the lie "the PDF you asked for is the
/// version you asked for", and export already has the bar this one mirrors
/// (`a_superseded_version_exports_at_its_own_number_and_says_a_newer_exists`).
///
/// Three things, and the third is what makes the first two more than a coincidence: the two PDFs
/// differ; each equals `render_pdf` of that version's OWN exported bytes; and each reports its own
/// number beside the count, so a superseded version can be told from the current one (R24.2).
#[test]
fn the_pdf_of_a_superseded_version_is_rendered_from_that_versions_bytes() {
    let fixture = admitted_mission("pdf-service-superseded");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, first_digest, first_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("version 1 publishes");

    // The correction: `Include` rather than `PolicyDefault`, so the request is in the manifest and
    // the canonical bytes differ even though this Mission is ordinary.
    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("the correction prepares");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .unwrap();
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("the correction publishes");

    let one = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect("a superseded version still renders");
    let two = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 2)
        .expect("the current version renders");

    assert_eq!(
        (one.publication_version(), one.publication_version_count()),
        (1, 2),
        "version 1 did not report itself as one of two"
    );
    assert_eq!(
        (two.publication_version(), two.publication_version_count()),
        (2, 2),
        "version 2 did not report itself as one of two"
    );
    assert_ne!(
        one.candidate_digest(),
        two.candidate_digest(),
        "the two versions cite one candidate, so this proves nothing"
    );
    assert_ne!(
        one.bytes(),
        two.bytes(),
        "two versions of one Mission rendered to the same PDF"
    );

    // Each is the render of ITS OWN exported bytes, computed here rather than taken on trust.
    for version in [1, 2] {
        let exported = fixture
            .outcomes
            .export_markdown_version(fixture.mission_id, version)
            .expect("the version exports");
        let served = if version == 1 { &one } else { &two };
        assert_eq!(served.candidate_digest(), exported.digest());
        assert_eq!(
            served.bytes(),
            beacond::render_pdf(
                exported.bytes(),
                "beacon.outcome.md.v1",
                &exported.digest().value,
                &beacond::BEACON_OUTCOME_1,
            )
            .expect("the exported bytes render")
            .as_slice(),
            "version {version}'s PDF is not the render of version {version}'s bytes"
        );
    }
}

/// The version `render_pdf_version` hands the translator is proved to be the document's, by the
/// reader, before this service ever looks at it (R38).
///
/// Why this bar and not the one that was asked for. The brief wanted a published document whose
/// manifest names an UNKNOWN renderer version, refused through the service by the translator. That
/// state is not reachable, and the reasons are three guards that were already there:
///
/// * `encode` refuses a manifest whose renderer version has no escaping function (`outcome.rs`),
///   so unknown-version bytes cannot be produced;
/// * `decode` refuses the same, so unknown-version bytes cannot be read back
///   (`MalformedOutcome::UnknownVersion`, covered in `tests/outcome.rs`);
/// * and `verified_manifest_within` proves the `renderer_version` PROJECTION COLUMN against the
///   decoded bytes, which is what this test drives.
///
/// Together they mean every manifest that reaches `render_pdf_version` names `beacon.outcome.md.v1`
/// — so reading the version from the manifest and hard-coding the constant are, today,
/// indistinguishable by construction. That is worth knowing rather than papering over: this bar
/// pins the guard that MAKES them equivalent, and the day `escaping_for` learns a second version it
/// is the enforcement this test describes that keeps the service honest.
#[test]
fn a_manifest_whose_renderer_version_disagrees_is_refused_through_the_pdf_service() {
    let fixture = admitted_mission("pdf-service-renderer-version");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("the honest pairing publishes");

    // Control: both paths work before the plant, so the refusals below are the plant's doing.
    assert!(
        fixture
            .outcomes
            .render_pdf_version(fixture.mission_id, 1)
            .is_ok()
    );
    assert!(
        fixture
            .outcomes
            .export_markdown_version(fixture.mission_id, 1)
            .is_ok()
    );

    // The projection column alone, past the Command and with the immutability trigger down -- the
    // shape a restore or an older binary produces. The canonical bytes still say v1; only the
    // column a reader might trust instead says otherwise.
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET renderer_version = 'beacon.outcome.md.v2'
                     WHERE workspace_id = ?1",
                    params![fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );

    let refusal = fixture
        .outcomes
        .render_pdf_version(fixture.mission_id, 1)
        .expect_err("a manifest whose renderer version disagrees rendered a PDF");
    assert!(
        matches!(
            refusal,
            PublishRefusal::Manifest(beacond::ManifestStoreRefusal::ColumnDisagrees {
                column: "renderer_version"
            })
        ),
        "the disagreement was not named, or was named as something else: {refusal:?}"
    );

    // And the PDF path is no laxer than the export it is built on: same store, same refusal.
    assert!(
        matches!(
            fixture
                .outcomes
                .export_markdown_version(fixture.mission_id, 1)
                .expect_err("the export accepted what the PDF path refused"),
            PublishRefusal::Manifest(beacond::ManifestStoreRefusal::ColumnDisagrees {
                column: "renderer_version"
            })
        ),
        "the two paths disagree about the same store"
    );
}

// ---------------------------------------------------------------------------------------------
// R39.2 — no invented identities: a refusal reports what was found
// ---------------------------------------------------------------------------------------------

/// A classification id column that is not a UUID is refused with the text that was there (R39.2).
///
/// `uuid_column` used to be `parse_str(..).unwrap_or(Uuid::nil())`, so a corrupt column became a
/// well-formed id the reader handed on as though the row had said it.
#[test]
fn a_classification_id_column_that_is_not_a_uuid_is_refused_with_what_was_found() {
    let fixture = admitted_mission("r39-bad-id-column");
    let (_, _, digest) = fixture.classify(ImpactLevel::Ordinary);

    with_trigger_down(
        &fixture.store,
        "mission_impact_classifications_no_update",
        "mission_impact_classifications",
        "Mission impact classifications are immutable",
        |connection| {
            // 0032 keys the row on (workspace, mission, project), so a non-UUID mission_id breaks
            // that foreign key. The column is what is under test, not the constraint.
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            connection
                .execute(
                    "UPDATE mission_impact_classifications SET mission_id = 'not-a-uuid-NEEDLE'
                     WHERE workspace_id = ?1",
                    params![fixture.workspace_id.to_string()],
                )
                .unwrap();
        },
    );

    let connection = fixture.store.connect();
    let refusal =
        beacond::verified_classification_within(&connection, fixture.workspace_id, &digest)
            .expect_err("a mission_id column that is not a UUID was read as one");
    let debug = format!("{refusal:?}");
    assert!(
        debug.contains("not-a-uuid-NEEDLE") && debug.contains("mission_id"),
        "the refusal does not carry what was found: {debug}"
    );
    assert!(
        !debug.contains("00000000-0000-0000-0000-000000000000"),
        "a nil UUID reached the refusal: {debug}"
    );
}

/// The two remaining R39.2 sites are DEFENSIVE, and this records the measurement that says so.
///
/// Both were fixed in the product -- neither invents an identity any more -- but neither arm can be
/// reached through the owners, so neither gets a bar that would pass for the wrong reason. Each
/// claim below is a refusal observed on this fixture, not an argument.
///
/// * `outcome_publication.rs`, the candidate with no Mission context: the Evaluation owner refuses
///   an unscoped Artifact first, with `Inconsistent(ArtifactHasNoMissionScope)`, so no publication
///   can cite one through any owner path.
/// * `mission_impact.rs`, the unparseable Workspace column at startup: a classification row whose
///   `workspace_id` is not a UUID needs a `workspaces` row with that same key (0032's composite
///   foreign key), and BOTH ways of planting one are refused by an earlier layer -- measured
///   below, not argued. A first draft of this bar asserted the raw text instead and passed for the
///   wrong reason; restoring the nil-carrying arm left it GREEN, which is how the vacuity was
///   found. The assertions here are on the refusals of the layers that run first, so they cannot
///   go green on my own pass's output.
#[test]
fn the_unreachable_r39_sites_are_recorded_rather_than_barred() {
    let fixture = admitted_mission("r39-defensive-sites");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (candidate, _, _) = fixture.publishable();

    // Site 1: the Evaluation owner refuses an unscoped Artifact, so the publication path never
    // reaches the candidate-context arm.
    let rendered = fixture
        .artifacts()
        .get(&candidate.candidate_artifact_digest)
        .unwrap()
        .content()
        .to_vec();
    let unscoped = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            &rendered,
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("an Artifact with no Mission context records");
    let refusal = fixture
        .evaluations()
        .record(EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(unscoped.digest().value.clone()),
            evaluator: Evaluator {
                resident_id: fixture.other_resident_id,
                configuration_id: fixture.other_configuration_id,
            },
            rubric: RubricDeclaration {
                id: "outcome-document-v1".to_owned(),
                version: 1,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: data_encoding::HEXLOWER.encode(&[0x20_u8; 32]),
            },
            cited_runs: Vec::new(),
            verdict: Verdict::Meets,
            findings: vec![Finding {
                severity: FindingSeverity::Note,
                summary: "Read the exact candidate bytes.".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "An independent Resident read the exact candidate bytes.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        })
        .expect_err("an unscoped Artifact was evaluated");
    assert!(
        format!("{refusal:?}").contains("ArtifactHasNoMissionScope"),
        "the Evaluation owner no longer refuses first, so the publication arm may now be \
         reachable and deserves its own bar: {refusal:?}"
    );

    // Site 2: the classification startup pass's unparseable Workspace key. The plant needs a
    // `workspaces` row carrying the same non-UUID key, and where that row sorts decides which
    // earlier layer answers -- neither of them the classification pass.
    fn boot_with_a_corrupt_workspace_key(name: &str, age_offset_ms: i64) -> SupervisorError {
        let fixture = admitted_mission(name);
        fixture.classify(ImpactLevel::Ordinary);
        with_trigger_down(
            &fixture.store,
            "mission_impact_classifications_no_update",
            "mission_impact_classifications",
            "Mission impact classifications are immutable",
            |connection| {
                connection
                    .pragma_update(None, "foreign_keys", "OFF")
                    .unwrap();
                connection
                    .execute(
                        "INSERT INTO workspaces (id, name, created_at_ms)
                         SELECT 'not-a-uuid-WSNEEDLE', name, created_at_ms + ?2 FROM workspaces
                         WHERE id = ?1",
                        params![fixture.workspace_id.to_string(), age_offset_ms],
                    )
                    .unwrap();
                connection
                    .execute(
                        "UPDATE mission_impact_classifications
                         SET workspace_id = 'not-a-uuid-WSNEEDLE' WHERE workspace_id = ?1",
                        params![fixture.workspace_id.to_string()],
                    )
                    .unwrap();
            },
        );
        astra_boot(&fixture).expect_err("a store holding a non-UUID Workspace key opened Ready")
    }

    // Planted older, it is the row `Supervisor::bootstrap` opens (`ORDER BY created_at_ms, id
    // LIMIT 1`), so `parse_workspace_id` refuses with what was found -- lib.rs:996, R39.2's shape
    // already applied there.
    let opened_first = boot_with_a_corrupt_workspace_key("r39-workspace-key-older", -1000);
    assert!(
        matches!(
            &opened_first,
            SupervisorError::InvalidWorkspaceId(value) if value == "not-a-uuid-WSNEEDLE"
        ),
        "the Workspace open no longer refuses first: {opened_first:?}"
    );

    // Planted newer, the open reads the good row and the composite foreign key is what the
    // classification row now breaks -- `PRAGMA foreign_key_check`, still before any pass.
    let checked_first = boot_with_a_corrupt_workspace_key("r39-workspace-key-newer", 1000);
    assert!(
        matches!(checked_first, SupervisorError::ForeignKeyViolations(_)),
        "the foreign-key check no longer refuses first: {checked_first:?}"
    );
}

mod candidate_evaluation {
    use super::*;
    use beacond::CandidateEvaluationService;

    fn record_counts(fixture: &Fixture) -> Vec<i64> {
        let connection = fixture.store.connect();
        [
            "events",
            "runs",
            "receipts",
            "evaluations",
            "artifacts",
            "outcome_manifests",
            "model_invocations",
            "agents",
            "resident_lifecycle_decisions",
            "resident_configuration_decisions",
            "resident_configuration_bindings",
            "resident_configuration_capabilities",
            "local_model_activation_decisions",
        ]
        .iter()
        .map(|table| {
            connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        })
        .collect()
    }

    #[test]
    fn current_evaluator_evidence_holds_exact_owner_records_without_authority() {
        let fixture = admitted_mission_with_reviewer_capabilities(
            "current-evaluation-context-exact",
            2,
            &[BeaconCapability::ArtifactEvaluate],
        );
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let held_resident =
            ResidentLifecycleService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .list()
                .unwrap()
                .into_iter()
                .find(|resident| resident.id == fixture.other_resident_id)
                .unwrap();
        let held_configuration =
            ResidentConfigurationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .list()
                .unwrap()
                .into_iter()
                .find(|configuration| configuration.resident_id == fixture.other_resident_id)
                .unwrap();
        assert_eq!(held_configuration.id, fixture.other_configuration_id);
        let held_binding = held_configuration.binding.as_ref().unwrap();
        assert_eq!(
            held_binding.capabilities,
            vec![BeaconCapability::ArtifactEvaluate]
        );
        // list() reads recorded decisions only; this endpoint is never inspected or contacted.
        let held_activation = LocalModelActivationService::new(
            Arc::clone(&fixture.accounts),
            fixture.workspace_id,
            LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
        )
        .list()
        .unwrap()
        .into_iter()
        .find(|activation| activation.id == held_binding.activation_id)
        .unwrap();
        let before = record_counts(&fixture);
        let context =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .context(&candidate.manifest_digest)
                .unwrap()
                .unwrap();

        let evidence = context.current_evaluator_evidence();
        assert_eq!(evidence.len(), 1, "B15.CURRENT.EXACT_HISTORICAL_SET");
        assert_eq!(
            evidence[0].admitted(),
            &Evaluator {
                resident_id: fixture.other_resident_id,
                configuration_id: fixture.other_configuration_id,
            }
        );
        assert_eq!(evidence[0].resident(), &held_resident);
        assert_eq!(evidence[0].current_configuration(), &held_configuration);
        let activation = evidence[0]
            .activation()
            .expect("configured record holds activation proof");
        assert_eq!(activation.configured(), &held_activation);
        assert_eq!(activation.latest_for_resource(), &held_activation);
        assert!(
            evidence[0].gaps().is_empty(),
            "B15.CURRENT.RECORDED_FACTS_MATCH"
        );
        assert_eq!(
            context.current_eligibility(),
            beacond::CurrentEvaluationEligibility::NotMeasured
        );
        assert_eq!(
            context.source_packet(),
            beacond::EvaluationSourcePacket::NotCollected
        );
        assert_eq!(
            record_counts(&fixture),
            before,
            "B15.CURRENT.READ_WRITES_NOTHING"
        );
    }

    #[test]
    fn current_evaluator_evidence_names_all_recorded_gaps_without_changing_admission() {
        use beacond::CurrentEvaluatorGap as Gap;
        let fixture = admitted_mission_with_reviewer_capabilities(
            "current-evaluation-context-gaps",
            2,
            &[BeaconCapability::ArtifactEvaluate],
        );
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let service =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let before = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        let prior = &before.current_evaluator_evidence()[0];
        let suspended =
            ResidentLifecycleService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .suspend(
                    fixture.other_resident_id,
                    prior.resident().latest_decision_id,
                )
                .unwrap();
        let configurations =
            ResidentConfigurationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let activation_id = prior.activation().unwrap().configured().id;
        let changed_configuration = configurations
            .configure(
                fixture.other_resident_id,
                Some(fixture.other_configuration_id),
                activation_id,
                vec![BeaconCapability::ArtifactProduce],
            )
            .unwrap();
        let deactivated = LocalModelActivationService::new(
            Arc::clone(&fixture.accounts),
            fixture.workspace_id,
            LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
        )
        .deactivate(activation_id)
        .unwrap();
        let changed = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        let evidence = &changed.current_evaluator_evidence()[0];
        assert_eq!(evidence.resident(), &suspended);
        assert_eq!(
            evidence.current_configuration().id,
            changed_configuration.id
        );
        assert_eq!(evidence.admitted(), prior.admitted());
        assert_eq!(
            evidence.activation().unwrap().configured(),
            prior.activation().unwrap().configured()
        );
        assert_eq!(
            evidence.activation().unwrap().latest_for_resource(),
            &deactivated
        );
        assert_eq!(
            evidence.gaps(),
            &[
                Gap::ResidentNotActive,
                Gap::ConfigurationChanged,
                Gap::MissingArtifactEvaluate,
                Gap::ActivationSuperseded,
                Gap::ActivationInactive,
            ],
            "B15.CURRENT.OVERLAPPING_GAPS_KEEP_ALL_FACTS"
        );
        let cleared = configurations
            .clear(fixture.other_resident_id, changed_configuration.id)
            .unwrap();
        let after_clear = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        let evidence = &after_clear.current_evaluator_evidence()[0];
        assert_eq!(evidence.current_configuration(), &cleared);
        assert!(evidence.activation().is_none());
        assert_eq!(
            evidence.gaps(),
            &[
                Gap::ResidentNotActive,
                Gap::ConfigurationCleared,
                Gap::ConfigurationChanged,
                Gap::MissingArtifactEvaluate,
            ],
            "B15.CURRENT.CLEARED_HAS_NO_ACTIVATION_FACTS"
        );
        assert_eq!(
            after_clear.admitted_non_author_residents(),
            before.admitted_non_author_residents()
        );
        assert_eq!(
            after_clear.current_eligibility(),
            beacond::CurrentEvaluationEligibility::NotMeasured
        );
    }

    #[test]
    fn current_evaluator_evidence_refuses_each_missing_owner_event_while_rows_remain() {
        for owner in ["Resident", "Configuration", "Activation"] {
            let fixture = admitted_mission_with_reviewer_capabilities(
                "current-evaluation-owner-event",
                2,
                &[BeaconCapability::ArtifactEvaluate],
            );
            fixture.classify(ImpactLevel::Ordinary);
            fixture.complete((fixture.resident_id, fixture.configuration_id));
            let candidate = fixture
                .outcomes
                .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
                .unwrap();
            let models = LocalModelActivationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
                LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
            );
            let activation = models.list().unwrap().remove(0);
            let (kind, selector, id) = match owner {
                "Resident" => (
                    "resident.activated",
                    "$.decision.residentId",
                    fixture.other_resident_id,
                ),
                "Configuration" => (
                    "resident.configuration_set",
                    "$.decision.id",
                    fixture.other_configuration_id,
                ),
                "Activation" => ("local_model.activated", "$.activation.id", activation.id),
                _ => unreachable!(),
            };
            assert_eq!(fixture.store.connect().execute(
                "DELETE FROM events WHERE kind = ?1 AND json_extract(payload_json, ?2) = ?3",
                params![kind, selector, id.to_string()],
            ).unwrap(), 1);
            assert!(
                ResidentLifecycleService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                    .list()
                    .unwrap()
                    .iter()
                    .any(|resident| resident.id == fixture.other_resident_id)
            );
            assert!(
                ResidentConfigurationService::new(
                    Arc::clone(&fixture.accounts),
                    fixture.workspace_id
                )
                .list()
                .unwrap()
                .iter()
                .any(|configuration| configuration.id == fixture.other_configuration_id)
            );
            assert_eq!(models.list().unwrap(), vec![activation]);
            let error = CandidateEvaluationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
            )
            .context(&candidate.manifest_digest)
            .expect_err("B15.CURRENT.COMPLETE_OWNER_EVENT_REQUIRED");
            assert_eq!(error.name(), owner, "B15.CURRENT.BROKEN_OWNER_IS_NOT_A_GAP");
        }
    }

    #[test]
    fn current_evaluator_evidence_bounds_selected_activation_before_owner_decode() {
        let fixture = admitted_mission_with_reviewer_capabilities(
            "current-evaluation-owner-cap",
            2,
            &[BeaconCapability::ArtifactEvaluate],
        );
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        with_trigger_down(
            &fixture.store,
            "local_model_activation_decisions_no_update",
            "local_model_activation_decisions",
            "local model activation decisions are append-only",
            |connection| {
                connection
                    .execute_batch("PRAGMA ignore_check_constraints = ON")
                    .unwrap();
                assert_eq!(connection.execute(
                    "UPDATE local_model_activation_decisions SET unrecognized_features_json = ?1",
                    [format!("[\"{}\"]", "x".repeat(65_537))],
                ).unwrap(), 1);
            },
        );
        let error =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .context(&candidate.manifest_digest)
                .unwrap_err();
        assert_eq!(
            error.name(),
            "ReadLimit",
            "B15.CURRENT.BOUND_BEFORE_OWNER_DECODE"
        );
    }

    #[test]
    fn current_evaluator_evidence_requires_each_owner_chain_even_when_events_match_rows() {
        for owner in ["Resident", "Configuration", "Activation"] {
            let fixture = admitted_mission_with_reviewer_capabilities(
                "current-evaluation-owner-chain",
                2,
                &[BeaconCapability::ArtifactEvaluate],
            );
            fixture.classify(ImpactLevel::Ordinary);
            fixture.complete((fixture.resident_id, fixture.configuration_id));
            let candidate = fixture
                .outcomes
                .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
                .unwrap();
            let residents =
                ResidentLifecycleService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
            let configurations = ResidentConfigurationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
            );
            let models = LocalModelActivationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
                LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
            );
            let prior_resident = residents
                .list()
                .unwrap()
                .into_iter()
                .find(|resident| resident.id == fixture.other_resident_id)
                .unwrap();
            let prior_activation = models.list().unwrap().remove(0);
            let (table, trigger, message, kind, selector, decision_id) = match owner {
                "Resident" => {
                    let changed = residents
                        .suspend(fixture.other_resident_id, prior_resident.latest_decision_id)
                        .unwrap();
                    assert_ne!(
                        changed.latest_decision_id,
                        prior_resident.latest_decision_id
                    );
                    (
                        "resident_lifecycle_decisions",
                        "resident_lifecycle_decisions_no_update",
                        "Resident lifecycle decisions are append-only",
                        "resident.activated",
                        "$.decision.id",
                        prior_resident.latest_decision_id.unwrap(),
                    )
                }
                "Configuration" => {
                    let changed = configurations
                        .configure(
                            fixture.other_resident_id,
                            Some(fixture.other_configuration_id),
                            prior_activation.id,
                            vec![BeaconCapability::ArtifactEvaluate],
                        )
                        .unwrap();
                    assert_ne!(changed.id, fixture.other_configuration_id);
                    (
                        "resident_configuration_decisions",
                        "resident_configuration_decisions_no_update",
                        "Resident configuration decisions are append-only",
                        "resident.configuration_set",
                        "$.decision.id",
                        fixture.other_configuration_id,
                    )
                }
                "Activation" => {
                    let changed = models.deactivate(prior_activation.id).unwrap();
                    assert_ne!(changed.id, prior_activation.id);
                    (
                        "local_model_activation_decisions",
                        "local_model_activation_decisions_no_update",
                        "local model activation decisions are append-only",
                        "local_model.activated",
                        "$.activation.id",
                        prior_activation.id,
                    )
                }
                _ => unreachable!(),
            };
            let held_residents = residents.list().unwrap();
            let held_configurations = configurations.list().unwrap();
            let held_activations = models.list().unwrap();
            let missing_predecessor = Uuid::new_v4();
            with_trigger_down(&fixture.store, trigger, table, message, |connection| {
                connection
                    .execute_batch(
                        "PRAGMA foreign_keys = OFF; PRAGMA ignore_check_constraints = ON",
                    )
                    .unwrap();
                assert_eq!(
                    connection
                        .execute(
                            &format!("UPDATE {table} SET predecessor_id = ?1 WHERE id = ?2"),
                            params![missing_predecessor.to_string(), decision_id.to_string()]
                        )
                        .unwrap(),
                    1
                );
                // The earlier initial row is corrupt; a valid later/current row still names it.
                // Replace only the null predecessor field in its canonical payload, preserving
                // field order and all other bytes so exact Event comparison matches that bad row.
                assert_eq!(
                    connection
                        .execute(
                            "UPDATE events SET payload_json = replace(payload_json, ?1, ?2)
                     WHERE kind = ?3 AND json_extract(payload_json, ?4) = ?5",
                            params![
                                "\"predecessorId\":null",
                                format!("\"predecessorId\":\"{missing_predecessor}\""),
                                kind,
                                selector,
                                decision_id.to_string()
                            ],
                        )
                        .unwrap(),
                    1
                );
            });
            assert_eq!(residents.list().unwrap(), held_residents);
            assert_eq!(configurations.list().unwrap(), held_configurations);
            assert_eq!(models.list().unwrap(), held_activations);
            let result = CandidateEvaluationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
            )
            .context(&candidate.manifest_digest);
            assert!(result.is_err(), "B15.CURRENT.OWNER_CHAIN_REQUIRED");
            let error = result.unwrap_err();
            assert_eq!(
                error.name(),
                owner,
                "B15.CURRENT.CHAIN_FAULT_STAYS_WITH_OWNER"
            );
        }
    }

    #[test]
    fn current_evaluator_evidence_refuses_nonpositive_owner_order_keys() {
        for owner in ["Resident", "Configuration", "Activation"] {
            let fixture = admitted_mission("current-evaluation-order-key");
            fixture.classify(ImpactLevel::Ordinary);
            fixture.complete((fixture.resident_id, fixture.configuration_id));
            let candidate = fixture
                .outcomes
                .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
                .unwrap();
            let (table, trigger, message, id) = match owner {
                "Resident" => {
                    let resident = ResidentLifecycleService::new(
                        Arc::clone(&fixture.accounts),
                        fixture.workspace_id,
                    )
                    .list()
                    .unwrap()
                    .into_iter()
                    .find(|resident| resident.id == fixture.other_resident_id)
                    .unwrap();
                    (
                        "resident_lifecycle_decisions",
                        "resident_lifecycle_decisions_no_update",
                        "Resident lifecycle decisions are append-only",
                        resident.latest_decision_id.unwrap(),
                    )
                }
                "Configuration" => (
                    "resident_configuration_decisions",
                    "resident_configuration_decisions_no_update",
                    "Resident configuration decisions are append-only",
                    fixture.other_configuration_id,
                ),
                "Activation" => {
                    let activation = LocalModelActivationService::new(
                        Arc::clone(&fixture.accounts),
                        fixture.workspace_id,
                        LocalModelEndpoint::new("127.0.0.1:1".parse().unwrap()).unwrap(),
                    )
                    .list()
                    .unwrap()
                    .remove(0);
                    (
                        "local_model_activation_decisions",
                        "local_model_activation_decisions_no_update",
                        "local model activation decisions are append-only",
                        activation.id,
                    )
                }
                _ => unreachable!(),
            };
            with_trigger_down(&fixture.store, trigger, table, message, |connection| {
                assert_eq!(
                    connection
                        .execute(
                            &format!("UPDATE {table} SET sequence = 0 WHERE id = ?1"),
                            [id.to_string()]
                        )
                        .unwrap(),
                    1
                );
            });
            let error = CandidateEvaluationService::new(
                Arc::clone(&fixture.accounts),
                fixture.workspace_id,
            )
            .context(&candidate.manifest_digest)
            .expect_err("B15.CURRENT.ORDER_KEY_DOMAIN_REQUIRED");
            assert_eq!(error.name(), owner);
        }
    }

    #[test]
    fn current_evaluator_evidence_refuses_an_overlong_history_without_truncation() {
        let fixture = admitted_mission_with_reviewer_capabilities(
            "current-evaluation-history-bound",
            2,
            &[BeaconCapability::ArtifactEvaluate],
        );
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let configurations =
            ResidentConfigurationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let mut current = configurations
            .list()
            .unwrap()
            .into_iter()
            .find(|configuration| configuration.resident_id == fixture.other_resident_id)
            .unwrap();
        let activation_id = current.binding.as_ref().unwrap().activation_id;
        // All 256 decisions are ordinary accepted Commands. Alternating clear/set keeps the
        // binding count below its separate limit, so the history guard has its own witness.
        for _ in 1..256 {
            current = if current.binding.is_some() {
                configurations
                    .clear(fixture.other_resident_id, current.id)
                    .unwrap()
            } else {
                configurations
                    .configure(
                        fixture.other_resident_id,
                        Some(current.id),
                        activation_id,
                        vec![BeaconCapability::ArtifactEvaluate],
                    )
                    .unwrap()
            };
        }
        let service =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let at_limit = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        assert_eq!(
            at_limit.current_evaluator_evidence()[0].current_configuration(),
            &current
        );
        configurations
            .configure(
                fixture.other_resident_id,
                Some(current.id),
                activation_id,
                vec![BeaconCapability::ArtifactEvaluate],
            )
            .unwrap();
        let error = service
            .context(&candidate.manifest_digest)
            .expect_err("B15.CURRENT.COMPLETE_HISTORY_LIMIT_REQUIRED");
        assert_eq!(error.name(), "ReadLimit");
    }

    #[test]
    fn context_holds_exact_candidate_and_non_author_admission_without_writes() {
        let fixture = admitted_mission("evaluation-context-exact");
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let held_manifest = fixture
            .outcomes
            .manifest(&candidate.manifest_digest)
            .unwrap();
        let held_artifact = fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap();
        let held_mission = MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .get(fixture.mission_id)
            .unwrap();
        let before = record_counts(&fixture);

        let context =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .context(&candidate.manifest_digest)
                .unwrap()
                .expect("the prepared candidate has a historical evaluation context");

        assert_eq!(context.manifest(), &held_manifest);
        assert_eq!(context.candidate(), &held_artifact);
        assert_eq!(context.mission_admission(), &held_mission);
        assert_eq!(
            context.admitted_non_author_residents(),
            &[Evaluator {
                resident_id: fixture.other_resident_id,
                configuration_id: fixture.other_configuration_id,
            }],
            "B15.CONTEXT.EXACT_NON_AUTHOR_ADMISSION"
        );
        assert_eq!(
            record_counts(&fixture),
            before,
            "B15.CONTEXT.READ_WRITES_NOTHING"
        );
    }

    #[test]
    fn later_author_reconfiguration_preserves_historical_non_author_context() {
        let fixture = admitted_mission("evaluation-context-author");
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let second = fixture.second_configuration_for_author();
        assert_ne!(second, fixture.configuration_id);

        let context =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .context(&candidate.manifest_digest)
                .unwrap()
                .unwrap();
        assert_eq!(
            context.admitted_non_author_residents(),
            &[Evaluator {
                resident_id: fixture.other_resident_id,
                configuration_id: fixture.other_configuration_id,
            }],
            "B15.CONTEXT.HISTORICAL_AUTHOR_CONFIGURATION_PRESERVED"
        );
        assert_eq!(
            context.mission_admission().members[0].admission_configuration_id,
            fixture.configuration_id,
            "historical author configuration stays admitted"
        );
    }

    #[test]
    fn absent_and_foreign_manifest_are_indistinguishable_but_broken_proof_refuses() {
        let fixture = admitted_mission("evaluation-context-opacity");
        fixture.classify(ImpactLevel::Ordinary);
        let (completion_digest, _) =
            fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let service =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let foreign_scope =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
        assert_eq!(
            service.context(&"ff".repeat(32)).unwrap(),
            foreign_scope.context(&candidate.manifest_digest).unwrap(),
            "B15.CONTEXT.ABSENT_FOREIGN_OPACITY"
        );
        assert!(service.context(&"ff".repeat(32)).unwrap().is_none());
        assert!(
            service
                .context(&candidate.manifest_digest)
                .unwrap()
                .is_some()
        );
        let changed = fixture
            .store
            .connect()
            .execute(
                "DELETE FROM events WHERE kind = 'mission.completed'
             AND json_extract(payload_json, '$.digestValue') = ?1",
                params![completion_digest],
            )
            .unwrap();
        assert_eq!(changed, 1);
        fixture
            .outcomes
            .manifest(&candidate.manifest_digest)
            .expect(
                "the lighter byte/candidate reader still succeeds without the completion Event",
            );
        assert!(
            matches!(
                service.context(&candidate.manifest_digest),
                Err(beacond::CandidateEvaluationError::Manifest(_))
            ),
            "B15.CONTEXT.BROKEN_OWNER_IS_NOT_ABSENT"
        );
    }

    #[test]
    fn later_team_or_configuration_cannot_repair_historical_admission() {
        let fixture = admitted_mission_with_members("evaluation-context-historical", 1);
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let service =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        let before = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        assert!(before.admitted_non_author_residents().is_empty());
        let new_author_configuration = fixture.second_configuration_for_author();
        TeamService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .form(TeamDraft {
                project_id: fixture.project_id,
                name: "Later reviewer team".to_owned(),
                members: vec![
                    TeamMemberDraft {
                        resident_id: fixture.resident_id,
                        expected_configuration_id: new_author_configuration,
                    },
                    TeamMemberDraft {
                        resident_id: fixture.other_resident_id,
                        expected_configuration_id: fixture.other_configuration_id,
                    },
                ],
            })
            .unwrap();
        let after = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        assert_eq!(after.mission_admission(), before.mission_admission());
        assert_eq!(after.mission_admission().members.len(), 1);
        assert!(
            after.admitted_non_author_residents().is_empty(),
            "B15.CONTEXT.LATER_TEAM_CANNOT_REPAIR_ADMISSION"
        );
    }

    #[test]
    fn oversized_selected_artifact_refuses_before_owner_decode() {
        let fixture = admitted_mission("evaluation-context-artifact-cap");
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        with_trigger_down(
            &fixture.store,
            "artifacts_no_update",
            "artifacts",
            "Artifacts are immutable",
            |connection| {
                connection
                    .execute_batch("PRAGMA ignore_check_constraints = ON")
                    .unwrap();
                assert_eq!(connection.execute(
                "UPDATE artifacts SET canonical_bytes = zeroblob(4097) WHERE digest_value = ?1",
                params![candidate.candidate_artifact_digest],
            ).unwrap(), 1);
            },
        );
        let refusal =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                .context(&candidate.manifest_digest)
                .expect_err("the selected record exceeds the read-owned cap");
        assert_eq!(
            refusal.name(),
            "ReadLimit",
            "B15.CONTEXT.BOUND_BEFORE_DECODE"
        );
    }

    #[test]
    fn public_errors_keep_nested_details_out_of_fixed_display() {
        use beacond::CandidateEvaluationError as Error;
        let private = "nested-private-driver-detail";
        let driver = || rusqlite::Error::InvalidParameterName(private.to_owned());
        let cases = [
            (
                Error::Account(OperatorAccountError::Store(driver())),
                "Account",
                true,
            ),
            (Error::InvalidManifestDigest, "InvalidManifestDigest", false),
            (Error::ReadLimit, "ReadLimit", false),
            (
                Error::Artifact(ArtifactError::Store(driver())),
                "Artifact",
                true,
            ),
            (
                Error::Manifest(ManifestStoreRefusal::Irreconcilable {
                    detail: private.to_owned(),
                }),
                "Manifest",
                true,
            ),
            (
                Error::Mission(MissionError::Store(driver())),
                "Mission",
                true,
            ),
            (
                Error::Resident(beacond::ResidentLifecycleError::Store(driver())),
                "Resident",
                true,
            ),
            (
                Error::Configuration(beacond::ResidentConfigurationError::Store(driver())),
                "Configuration",
                true,
            ),
            (
                Error::Activation(beacond::LocalModelActivationError::Store(driver())),
                "Activation",
                true,
            ),
            (Error::MissionAbsent, "MissionAbsent", false),
            (Error::ScopeMismatch, "ScopeMismatch", false),
            (Error::Store(driver()), "Store", true),
        ];
        for (error, expected, carries_details) in cases {
            // Independent literals above, and an exhaustive match: a new public arm needs a case.
            let expected_by_arm = match &error {
                Error::Account(_) => "Account",
                Error::InvalidManifestDigest => "InvalidManifestDigest",
                Error::ReadLimit => "ReadLimit",
                Error::Artifact(_) => "Artifact",
                Error::Manifest(_) => "Manifest",
                Error::Mission(_) => "Mission",
                Error::Resident(_) => "Resident",
                Error::Configuration(_) => "Configuration",
                Error::Activation(_) => "Activation",
                Error::MissionAbsent => "MissionAbsent",
                Error::ScopeMismatch => "ScopeMismatch",
                Error::Store(_) => "Store",
            };
            assert_eq!(expected, expected_by_arm);
            assert_eq!(error.name(), expected);
            assert_eq!(
                error.to_string(),
                expected,
                "B15.CONTEXT.CLOSED_ERROR_DISPLAY"
            );
            assert_eq!(format!("{error:?}").contains(private), carries_details);
            assert_eq!(std::error::Error::source(&error).is_some(), carries_details);
        }
    }

    #[test]
    fn authentication_precedes_selector_details_and_draft_is_explicit() {
        let fixture = admitted_mission("evaluation-context-auth");
        fixture.classify(ImpactLevel::Ordinary);
        fixture.complete((fixture.resident_id, fixture.configuration_id));
        let candidate = fixture
            .outcomes
            .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
            .unwrap();
        let service =
            CandidateEvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
        assert_eq!(
            service.context("malformed").unwrap_err().name(),
            "InvalidManifestDigest"
        );
        let context = service
            .context(&candidate.manifest_digest)
            .unwrap()
            .unwrap();
        assert_eq!(
            context.current_eligibility(),
            beacond::CurrentEvaluationEligibility::NotMeasured
        );
        assert_eq!(
            context.source_packet(),
            beacond::EvaluationSourcePacket::NotCollected
        );
        assert_eq!(context.rubric().id(), "outcome-fidelity-draft");
        assert_eq!(context.rubric().version(), 1);
        let rubric = std::str::from_utf8(context.rubric().bytes()).unwrap();
        for criterion in [
            "completion_fidelity:",
            "evidence_fidelity:",
            "honesty_states:",
            "limitations_and_authority:",
        ] {
            assert_eq!(rubric.matches(criterion).count(), 1);
        }
        fixture.accounts.lock().unwrap();
        for selector in ["malformed", candidate.manifest_digest.as_str()] {
            assert!(
                matches!(
                    service.context(selector),
                    Err(beacond::CandidateEvaluationError::Account(
                        OperatorAccountError::NotAuthenticated
                    ))
                ),
                "B15.CONTEXT.AUTHENTICATION_FIRST"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// B12 SCRATCH — the real transport boundary for an exported Outcome.
// ---------------------------------------------------------------------------------------------

/// A RUNNING server on this fixture's store, plus its client, with RAII stop/join/cleanup.
///
/// The first version of this probe bound a listener and never served it, so every request timed
/// out. A bound listener is not a running server: `serve_until` owns the accept loop
/// (`local_transport.rs:1156-1175`) and the integration fixture starts it on a thread (`:206`).
struct B12Wire {
    client: beacond::LocalSupervisorClient,
    running: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<Result<(), beacond::LocalTransportError>>>,
    socket: std::path::PathBuf,
    dir: std::path::PathBuf,
}

impl B12Wire {
    fn open(fixture: &Fixture) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("b12-{}", Uuid::new_v4().simple()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = dir.join("s.sock");
        let token = dir.join("t");
        beacond::initialize_local_auth_token(&token).unwrap();
        let server = beacond::LocalSupervisorServer::bind(beacond::LocalServerConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
            socket_path: socket.clone(),
            auth_token_path: token.clone(),
            lock_path: dir.join("l"),
            local_model_endpoint: LocalModelEndpoint::ollama_default(),
        })
        .unwrap();
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let server_running = Arc::clone(&running);
        let thread = std::thread::spawn(move || server.serve_until(server_running));
        let client = beacond::LocalSupervisorClient::connect(&socket, &token).unwrap();
        client
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("B12.WIRE.AUTHENTICATED");
        Self {
            client,
            running,
            thread: Some(thread),
            socket,
            dir,
        }
    }
}

impl Drop for B12Wire {
    fn drop(&mut self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Release);
        // Nudge the accept loop awake so the join is bounded, as the existing fixture does.
        std::os::unix::net::UnixStream::connect(&self.socket).ok();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The payload's real serialized size, from `serde_json`, not hand arithmetic.
///
/// This is the `number[]` the wire actually carries for `ExportedOutcome.bytes`. It is a **lower
/// bound on the response**: the envelope, the digest, the version fields and the media type are all
/// additional, and this figure counts none of them.
fn b12_payload_json_len(bytes: &[u8]) -> usize {
    serde_json::to_vec(&bytes.to_vec()).unwrap().len()
}

/// The completion's own maximum of unfinished-work items, at the largest summary each may carry.
///
/// 32 is `MAX_NARRATIVE_ITEMS` and each summary sits inside `MAX_SHORT_TEXT_SCALARS`: valid content
/// an owner really recorded, not a synthetic blob. Extracted from
/// `b16_a_large_owner_minted_outcome_crosses_byte_identically` so the large fixture recipe and that
/// test cannot drift into producing different documents while claiming the same provenance.
fn large_unfinished_work_items() -> Vec<NarrativeItem<UnfinishedKind>> {
    (0..32)
        .map(|ordinal| NarrativeItem {
            ordinal,
            kind: UnfinishedKind::DeferredScope,
            condition_ordinal: None,
            summary: format!("Deferred scope item {ordinal}: {}", "x".repeat(460)),
        })
        .collect()
}

#[test]
fn b16_a_large_owner_minted_outcome_crosses_byte_identically() {
    // SMALL CONTROL -----------------------------------------------------------------------
    let small = admitted_mission("b12-small");
    small.classify(ImpactLevel::Ordinary);
    small.complete((small.resident_id, small.configuration_id));
    let (_c, candidate_digest, evaluation_digest) = small.publishable();
    small
        .outcomes
        .publish(small.mission_id, &candidate_digest, &evaluation_digest)
        .expect("B12.SMALL.PUBLISHED");

    let direct = small
        .outcomes
        .export_markdown(small.mission_id)
        .expect("B12.SMALL.DIRECT_EXPORT");
    println!("B12.SMALL.stored_bytes = {}", direct.bytes().len());
    println!(
        "B12.SMALL.payload_json_len = {}",
        b12_payload_json_len(direct.bytes())
    );

    let wire = B12Wire::open(&small);
    let read = wire
        .client
        .outcome_document(small.mission_id, Some(1))
        .expect("B12.SMALL.WIRE_OK");
    assert_eq!(read.bytes(), direct.bytes(), "B12.SMALL.WIRE_MATCHES_OWNER");
    println!("B12.SMALL.wire = Ok, bytes = {}", read.bytes().len());
    drop(wire);

    // LARGER, minted through owner commands ------------------------------------------------
    // 32 unfinished-work items is the completion's own maximum (MAX_NARRATIVE_ITEMS), each summary
    // inside MAX_SHORT_TEXT_SCALARS. Valid content recorded by the owner, not a synthetic blob.
    let large = admitted_mission("b12-large");
    large.classify(ImpactLevel::Ordinary);
    large.complete_with(
        (large.resident_id, large.configuration_id),
        large_unfinished_work_items(),
    );
    let (_c2, digest2, evaluation2) = large.publishable();
    large
        .outcomes
        .publish(large.mission_id, &digest2, &evaluation2)
        .expect("B12.LARGE.PUBLISHED");

    let direct2 = large
        .outcomes
        .export_markdown(large.mission_id)
        .expect("B12.LARGE.DIRECT_EXPORT");
    let large_payload = b12_payload_json_len(direct2.bytes());
    println!("B12.LARGE.stored_bytes = {}", direct2.bytes().len());
    println!("B12.LARGE.payload_json_len = {large_payload}");
    println!(
        "B12.LARGE.exceeds_64KiB_payload_alone = {}",
        large_payload > 64 * 1024
    );

    // B12 only OBSERVED this; here it is the assertion. The document is minted by real owner
    // Commands and read through the real server, so nothing about the semantic path is stubbed.
    let wire2 = B12Wire::open(&large);
    let read2 = wire2
        .client
        .outcome_document(large.mission_id, Some(1))
        .expect("B16.OUTCOME.LARGE_STORED_BYTES_CROSS: the large export must reach the client");
    assert_eq!(
        read2.bytes(),
        direct2.bytes(),
        "B16.OUTCOME.LARGE_STORED_BYTES_CROSS"
    );
    assert!(
        large_payload > 64 * 1024,
        "B16.OUTCOME.LARGE_STORED_BYTES_CROSS: the fixture must exceed the ordinary bound, got {large_payload}"
    );
}

// ---------------------------------------------------------------------------------------------
// B16 -- the transport frame itself. These serve CRAFTED responses from a raw listener: they are
// about the client's response bound, not about the owner, and they say so. No owner path is
// stubbed anywhere in this file.
// ---------------------------------------------------------------------------------------------

struct B16Scratch(std::path::PathBuf);

impl Drop for B16Scratch {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if !std::thread::panicking() {
                panic!("B16.STUB.SCRATCH_REMOVED: {error}");
            }
            eprintln!("B16.STUB.SCRATCH_REMOVED during unwind: {error}");
        }
    }
}

/// A raw listener that answers one request with exactly the bytes given, then closes.
///
/// Returns the stub's own write result separately, because an intentionally over-limit response
/// legitimately ends in `BrokenPipe` when the client stops reading -- that is the stub's verdict,
/// never the client's.
fn b16_serve_once(
    response: Vec<u8>,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::thread::JoinHandle<std::io::Result<()>>,
    B16Scratch,
) {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("b16-{}", Uuid::new_v4().simple()));
    std::fs::create_dir(&dir).unwrap();
    let scratch = B16Scratch(dir.clone());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("s.sock");
    let token = dir.join("t");
    beacond::initialize_local_auth_token(&token).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        let mut request = Vec::new();
        stream.read_to_end(&mut request)?;
        stream.write_all(&response)?;
        stream.flush()
    });
    (socket, token, handle, scratch)
}

/// The maximum-domain success response, built with real `serde_json` over the DECLARED shape.
///
/// It is NOT the crate's own `ResponseEnvelope`, which is private; it replicates the declared
/// shape field for field (`local_transport.rs:934-940`, `outcome_publication.rs:195-203`) at the
/// widest each bounded field may be. It is synthetic: these are not owner-minted valid Markdown
/// bytes, and nothing here claims the owner can produce this exact document.
fn b16_max_domain_response(body_len: usize) -> Vec<u8> {
    let document = serde_json::json!({
        "bytes": vec![255u8; body_len],
        // The owner's CLOSED domain, as independent literals: `digest_envelope`
        // (`artifact.rs:250-256`) fixes these and the export copies them unchanged
        // (`outcome_publication.rs:234-242`). Generic 64-byte token maxima are NOT this domain --
        // that was the error in this file's first version.
        "digest": {
            "algorithm": "sha256",
            "canonicalizer": "beacon.artifact.v1",
            "value": "0".repeat(64),
        },
        "publicationVersion": u32::MAX,
        "publicationVersionCount": u32::MAX,
        "mediaType": "text/markdown; charset=utf-8",
    });
    serde_json::to_vec(&serde_json::json!({
        "protocol": 1,
        "result": {"kind": "outcome_document", "document": document},
        "error": serde_json::Value::Null,
    }))
    .unwrap()
}

#[test]
fn b16_the_maximum_domain_outcome_response_fits_and_one_byte_more_does_not() {
    // Keep an independent literal for the owner's Markdown bound, so a contract change requires
    // an explicit update to this witness rather than silently changing both sides.
    let at_bound = b16_max_domain_response(1_048_576);
    // Independent literal. B15's derivation is correct: 342 fixed bytes over the owner's closed
    // value domain. This file's first version used generic token maxima and got 482, which was
    // wrong about the domain, not about serde.
    assert_eq!(
        at_bound.len(),
        4_194_647,
        "B16.OUTCOME.MAX_DOMAIN_FITS: closed-domain size"
    );

    let (socket, token, stub, _scratch) = b16_serve_once(at_bound.clone());
    let client = beacond::LocalSupervisorClient::connect(&socket, &token).unwrap();
    let read = client.outcome_document(Uuid::new_v4(), Some(1));
    let stub_write = stub.join().unwrap();
    assert!(
        read.is_ok(),
        "B16.OUTCOME.MAX_DOMAIN_FITS: a maximum-domain response must be readable, got {read:?}"
    );
    assert_eq!(
        read.unwrap().bytes().len(),
        1_048_576,
        "B16.OUTCOME.MAX_DOMAIN_FITS: whole body"
    );
    println!("B16.MAX_DOMAIN.stub_write = {stub_write:?}");

    // Exercise the client, not just a comparison against the declared constant. JSON permits
    // trailing whitespace, so this exact +1 response would decode successfully without the bound.
    let mut over = at_bound;
    over.push(b' ');
    assert_eq!(over.len(), 4_194_648, "B16.OUTCOME.OVER_FIXTURE_IS_EXACT");
    assert!(
        serde_json::from_slice::<serde_json::Value>(&over).is_ok(),
        "B16.OUTCOME.OVER_FIXTURE_IS_VALID_JSON"
    );
    let (socket, token, stub, _over_scratch) = b16_serve_once(over);
    let client = beacond::LocalSupervisorClient::connect(&socket, &token).unwrap();
    let refused = client.outcome_document(Uuid::new_v4(), Some(1));
    println!("B16.OVER_LIMIT.stub_write = {:?}", stub.join().unwrap());
    assert!(
        matches!(refused, Err(beacond::LocalTransportError::Protocol)),
        "B16.OUTCOME.RESPONSE_REMAINS_BOUNDED"
    );
}

#[test]
fn b16_an_oversized_ordinary_response_is_still_refused() {
    // A VALID, otherwise-successful non-Outcome response: a publication list, well over 64 KiB.
    let publications: Vec<serde_json::Value> = (0..2_000)
        .map(|_| {
            serde_json::json!({
                "publicationId": Uuid::new_v4(),
                "publicationVersion": 1,
                "candidateArtifactDigest": "a".repeat(64),
                "evaluationDigest": "b".repeat(64),
                "isCurrent": true,
            })
        })
        .collect();
    let response = serde_json::to_vec(&serde_json::json!({
        "protocol": 1,
        "result": {"kind": "mission_publications", "publications": publications},
        "error": serde_json::Value::Null,
    }))
    .unwrap();
    assert!(
        response.len() > 64 * 1024,
        "the fixture must exceed the ordinary bound"
    );

    let (socket, token, stub, _scratch) = b16_serve_once(response);
    let client = beacond::LocalSupervisorClient::connect(&socket, &token).unwrap();
    let refused = client.mission_publications(Uuid::new_v4());
    println!("B16.ORDINARY.stub_write = {:?}", stub.join().unwrap());
    assert!(
        matches!(refused, Err(beacond::LocalTransportError::Protocol)),
        "B16.ORDINARY.RESPONSE_BOUND_UNCHANGED: got {refused:?}"
    );
}

#[test]
fn b16_an_oversized_request_does_not_succeed_through_the_public_client() {
    // An otherwise-VALID request, well over 64 KiB: a real command with a long but well-formed
    // residents list. If the request bound were raised, this would decode and be refused by the
    // starter-organization owner instead -- a different, non-Protocol outcome -- so this bar
    // cannot pass for an unrelated decode or owner reason.
    let fixture = admitted_mission("b16-request-bound");
    let wire = B12Wire::open(&fixture);
    let residents: Vec<beacond::StarterResidentDraft> = (0..4_000)
        .map(|index| beacond::StarterResidentDraft {
            role: beacond::StarterRole::Builder,
            display_name: Some(format!("Resident {index} {}", "n".repeat(24))),
        })
        .collect();
    let encoded = serde_json::to_vec(&residents).unwrap().len();
    assert!(
        encoded > 64 * 1024,
        "the fixture must exceed the request bound, got {encoded}"
    );

    // The server refuses at `read_request` and closes, so the client's own write observes
    // BrokenPipe. That is the request bound refusing -- what this bar asserts is that the request
    // never reaches the owner. With the bound raised, the same request decodes and comes back as a
    // starter-organization refusal instead, which is a Remote code and fails this match.
    let refused = wire.client.accept_starter_organization(1, residents);
    // NOT a named acceptance of the server's cause: a broad `Io` match would also pass for an
    // unrelated IO failure, which Astra proved by returning `Io(PermissionDenied)` without sending
    // a byte. The decisive bar is `b16_the_server_request_bound_refuses_an_oversized_valid_request`
    // in `local_transport.rs`, which calls the real `read_request`. This one only records that the
    // public client does not succeed.
    assert!(
        refused.is_err(),
        "B16.REQUEST.CLIENT_DOES_NOT_SUCCEED (weak; see the unit bar): got {refused:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// B18: a private, disposable Outcome fixture for a later packaged read check (Blue's DP-B17)
// ---------------------------------------------------------------------------------------------
//
// The `LocalCommand` interface exposes completion, classification and candidate preparation,
// but no Outcome publication operation. This fixture seeds a published Outcome through the
// public owner interfaces used throughout this file, then hands the result to a later check as
// three files in a directory the CALLER names.
//
// What it establishes, and what it does not. It establishes owner-interface SEEDING and snapshot
// READBACK: a store minted here reopens through `Supervisor::bootstrap` and the ordinary
// `OperatorAccountService` / `OutcomeService` and returns the exact retained bytes and the same
// publication identity. It does not exercise the packaged completion/classification/prepare path,
// prove a packaged publication path, or exercise the desktop UI or clipboard. Those stay NOT PROBED.

/// The fixture account's passphrase: this file's existing known phrase, and nothing else.
///
/// It is a test constant, not a credential, and it is the reason the minted store is an isolated
/// throwaway rather than anything resembling a real profile. It is deliberately NOT written to the
/// manifest: a file that ships next to a store must not also carry the way in.
const FIXTURE_PASSPHRASE: &str = "a long local passphrase";

/// Two literals the seeded records put in the document, asserted after the mint.
///
/// Without them "the fixture minted" and "the fixture minted an empty document" are the same
/// green: every digest, length and byte comparison downstream agrees with itself just as happily
/// about zero bytes. These are the assertions that know what the document should SAY. They are the
/// Mission's own declaration and the completion's own declared condition -- narrative that reaches
/// the page from the seeded records, not from the renderer's fixed prose, which a document about
/// no Mission at all would still carry.
///
/// Measured, not assumed: the completion's `summary` field is NOT rendered (`render` puts the
/// declaration, the conditions and the citations on the page, not the summary line), so a literal
/// taken from `complete_with` would have failed here for the right reason and been fixed the wrong
/// way. These two are read off the produced document.
const FIXTURE_MISSION_DECLARATION: &str = "Ship one reviewed Beacon feature.";

const FIXTURE_SNAPSHOT_FILE: &str = "store.sqlite3";
const FIXTURE_MARKDOWN_FILE: &str = "expected-outcome.md";
const FIXTURE_MANIFEST_FILE: &str = "fixture.json";

/// Why a mint did not happen. Every arm refuses BEFORE anything is created or written.
#[derive(Debug, PartialEq, Eq)]
enum FixtureMintRefusal {
    /// No output directory was named at all. A mint with nowhere to go must say so: the failure
    /// this replaces is a run that "passes" having written a fixture nobody can find.
    Unset,
    /// The named output does not LAND inside the validated private prefix. Landing, not spelling:
    /// the parent is resolved with `canonicalize`, so a `..` component and an intermediate
    /// directory symlink are both refused for going outside, however the caller wrote it.
    NotPrivate(PathBuf),
    /// Something is already at the named path -- a directory, a file, or a symlink.
    Occupied(PathBuf),
    /// The PREFIX itself is not a task-owned private directory, so nothing minted under it could
    /// be private either. Astra's B18 review: treating any `TMPDIR` string as private establishes
    /// no such property.
    PrefixNotPrivate(PathBuf, PrefixFault),
}

/// What disqualifies a directory from being the task-owned private prefix.
///
/// `/tmp` itself is deliberately its own arm rather than folding into `GroupOrWorldAccessible`:
/// `/tmp` is `1777` and shared by every process on the machine, and a fixture minted in it is
/// visible to all of them. It is a different fault from a private directory left too open.
#[derive(Debug, PartialEq, Eq)]
enum PrefixFault {
    NotAbsolute,
    Missing,
    NotUnderTmp,
    IsSharedTmp,
    NotADirectory,
    NotOwnedByThisUser { owner: u32, expected: u32 },
    GroupOrWorldAccessible { mode: u32 },
}

impl std::fmt::Display for FixtureMintRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unset => write!(
                formatter,
                "B18.FIXTURE.OUTPUT_UNSET: no output directory was named. Set \
                 BEACON_B18_FIXTURE_DIR to a path that does not exist yet, under {}",
                std::env::temp_dir().display()
            ),
            Self::NotPrivate(path) => write!(
                formatter,
                "B18.FIXTURE.OUTPUT_NOT_PRIVATE: {} is not a new path under this process's temp \
                 directory {}",
                path.display(),
                std::env::temp_dir().display()
            ),
            Self::Occupied(path) => write!(
                formatter,
                "B18.FIXTURE.OUTPUT_OCCUPIED: {} already exists (a directory, a file, or a \
                 symlink); a mint creates its own directory and never writes into someone else's",
                path.display()
            ),
            Self::PrefixNotPrivate(path, fault) => write!(
                formatter,
                "B18.FIXTURE.PREFIX_NOT_PRIVATE: {} is not a task-owned private prefix ({fault:?}). \
                 Make one first -- `export TMPDIR=\"$(mktemp -d)\"` -- and mint inside it; /tmp \
                 itself is 1777 and shared with every process on the machine",
                path.display()
            ),
        }
    }
}

/// One minted fixture: the three files, and the identity a later check compares against.
#[derive(Debug)]
struct MintedOutcomeFixture {
    directory: PathBuf,
    snapshot_path: PathBuf,
    markdown_path: PathBuf,
    manifest_path: PathBuf,
    source_head: String,
    schema_version: i64,
    workspace_id: Uuid,
    mission_id: Uuid,
    publication_version: u32,
    publication_version_count: u32,
    candidate_artifact_digest: String,
    media_type: String,
    /// The exact retained owner bytes -- the ones the export returned at mint time, held here so
    /// the readback compares against what was RETAINED and not against a second fresh export.
    markdown_bytes: Vec<u8>,
    markdown_sha256: String,
    snapshot_sha256: String,
}

/// What reopening a minted snapshot returned.
#[derive(Debug)]
struct MintedFixtureReadBack {
    workspace_id: Uuid,
    workspace_created: bool,
    schema_version: i64,
    publication_versions: Vec<(u32, bool, String)>,
    exported_bytes: Vec<u8>,
    exported_digest: String,
    publication_version: u32,
    publication_version_count: u32,
    media_type: String,
}

fn sha256_hex_of(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(&sha2_256(bytes))
}

/// The commit the fixture was minted from, read from git rather than typed by an operator.
fn fixture_source_head() -> String {
    let output = std::process::Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .expect("B18.FIXTURE.SOURCE_HEAD_UNREADABLE: `git rev-parse HEAD` did not run");
    assert!(
        output.status.success(),
        "B18.FIXTURE.SOURCE_HEAD_UNREADABLE: git rev-parse HEAD exited {:?}",
        output.status.code()
    );
    String::from_utf8(output.stdout)
        .expect("B18.FIXTURE.SOURCE_HEAD_UNREADABLE: git printed non-UTF-8")
        .trim()
        .to_owned()
}

/// The schema version a store DECLARES, read from the store's own row.
fn declared_schema_version_of(path: &Path) -> i64 {
    Connection::open(path)
        .expect("B18.FIXTURE.SNAPSHOT_OPENS")
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .expect("B18.FIXTURE.SNAPSHOT_DECLARES_A_SCHEMA_VERSION")
}

fn write_private_file(path: &Path, bytes: &[u8]) {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap_or_else(|error| {
            panic!(
                "B18.FIXTURE.PRIVATE_FILE_CREATED {}: {error}",
                path.display()
            )
        });
    file.write_all(bytes).unwrap_or_else(|error| {
        panic!(
            "B18.FIXTURE.PRIVATE_FILE_WRITTEN {}: {error}",
            path.display()
        )
    });
}

fn mode_of(path: &Path) -> u32 {
    fs::metadata(path)
        .unwrap_or_else(|error| panic!("B18.FIXTURE.STAT {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// The private-output guard and the `0700` directory, shared by every mint recipe.
///
/// Extracted rather than copied: a second recipe with its own copy of these checks is a
/// second place for them to drift, and these are the checks Astra's B18 probes were about.
fn prepare_private_output(
    prefix: &Path,
    output: Option<&Path>,
) -> Result<PathBuf, FixtureMintRefusal> {
    let output = output.ok_or(FixtureMintRefusal::Unset)?;
    let prefix = validated_private_prefix(prefix)
        .map_err(|fault| FixtureMintRefusal::PrefixNotPrivate(prefix.to_path_buf(), fault))?;

    // Where the output LANDS, not how it is spelled. `starts_with` on the given path is lexical:
    // `<prefix>/../elsewhere/mint` passes it, and so does `<prefix>/link/mint` where `link` points
    // out of the prefix -- Astra's two probes. Resolving the PARENT with `canonicalize` collapses
    // every `..` and follows every intermediate symlink, so both land outside and are refused.
    // The parent must already exist: a mint creates one directory, never a chain of them.
    if !output.is_absolute() || output.file_name().is_none() {
        return Err(FixtureMintRefusal::NotPrivate(output.to_path_buf()));
    }
    let parent = output
        .parent()
        .ok_or_else(|| FixtureMintRefusal::NotPrivate(output.to_path_buf()))?;
    let resolved_parent = fs::canonicalize(parent)
        .map_err(|_| FixtureMintRefusal::NotPrivate(output.to_path_buf()))?;
    if !resolved_parent.starts_with(&prefix) {
        return Err(FixtureMintRefusal::NotPrivate(output.to_path_buf()));
    }
    // `symlink_metadata`, not `exists`: a symlink is something that is already there, and a
    // DANGLING one is exactly what `exists()` reports as absent. Refusing on the link itself is
    // what stops a mint from following someone else's pointer into a directory it did not create.
    if fs::symlink_metadata(output).is_ok() {
        return Err(FixtureMintRefusal::Occupied(output.to_path_buf()));
    }

    // `0700` AT CREATION. `create_dir` then `set_permissions` leaves the directory readable for
    // however long the second call takes, under whatever umask the caller happens to have.
    fs::DirBuilder::new()
        .mode(0o700)
        .create(output)
        .unwrap_or_else(|error| panic!("B18.FIXTURE.DIRECTORY_CREATED: {error}"));
    Ok(output.to_path_buf())
}

/// Mints one private fixture into a directory that does not exist yet.
///
/// The seeding is this file's own public owner recipe, unchanged: `admitted_mission` ->
/// `classify` -> `complete` -> `publishable` -> `outcomes.publish`. Nothing is planted, no row is
/// written behind a Command, and the store is a throwaway the helper already deletes on drop --
/// what survives is the SNAPSHOT.
fn mint_private_outcome_fixture(
    output: Option<&Path>,
) -> Result<MintedOutcomeFixture, FixtureMintRefusal> {
    mint_private_outcome_fixture_within(&std::env::temp_dir(), output)
}

/// The effective user this process writes as.
///
/// `geteuid` and not `getuid`: what decides whether this process can read the prefix back is the
/// EFFECTIVE id, and those differ under setuid. The crate already calls libc directly
/// (`git_object_store.rs:902`), so this follows the tree's own idiom rather than adding a crate.
fn effective_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, touches no memory, and cannot fail.
    unsafe { libc::geteuid() }
}

/// The prefix a mint is allowed to write inside, or why this one is not it.
///
/// Astra's B18 review is the reason this exists: the first version treated any `TMPDIR` string as
/// private, which establishes nothing about the directory it names. Location, type, owner and
/// permissions are each checked against the filesystem, and the returned path is CANONICAL so the
/// caller compares resolved paths with a resolved prefix rather than two strings.
fn validated_private_prefix(prefix: &Path) -> Result<PathBuf, PrefixFault> {
    if !prefix.is_absolute() {
        return Err(PrefixFault::NotAbsolute);
    }
    let resolved = fs::canonicalize(prefix).map_err(|_| PrefixFault::Missing)?;
    // `/tmp` is canonicalized too: it is a symlink on some systems, and a prefix check that
    // compares one resolved path against one unresolved one is the bug this fold is fixing.
    let shared_tmp = fs::canonicalize("/tmp").map_err(|_| PrefixFault::NotUnderTmp)?;
    if resolved == shared_tmp {
        return Err(PrefixFault::IsSharedTmp);
    }
    if !resolved.starts_with(&shared_tmp) {
        return Err(PrefixFault::NotUnderTmp);
    }
    // `canonicalize` already resolved every link, so this stats the real directory.
    let metadata = fs::symlink_metadata(&resolved).map_err(|_| PrefixFault::Missing)?;
    if !metadata.is_dir() {
        return Err(PrefixFault::NotADirectory);
    }
    let expected = effective_uid();
    if metadata.uid() != expected {
        return Err(PrefixFault::NotOwnedByThisUser {
            owner: metadata.uid(),
            expected,
        });
    }
    let mode = metadata.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(PrefixFault::GroupOrWorldAccessible { mode });
    }
    Ok(resolved)
}

/// Mints inside an explicitly named prefix. The parameter is what makes the guard testable: a bar
/// can hand it a `/tmp`, a `0755` directory or a regular file and read the refusal, without any
/// test mutating the process environment out from under every other test in the binary.
fn mint_private_outcome_fixture_within(
    prefix: &Path,
    output: Option<&Path>,
) -> Result<MintedOutcomeFixture, FixtureMintRefusal> {
    let output = prepare_private_output(prefix, output)?;
    let output = output.as_path();

    // The public owner recipe, exactly as every other test in this file runs it.
    let fixture = admitted_mission("b18-private-fixture");
    fixture.classify(ImpactLevel::Ordinary);
    fixture.complete((fixture.resident_id, fixture.configuration_id));
    let (_, candidate_digest, evaluation_digest) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &candidate_digest, &evaluation_digest)
        .expect("B18.FIXTURE.PUBLISH_SUCCEEDS");
    let exported = fixture
        .outcomes
        .export_markdown(fixture.mission_id)
        .expect("B18.FIXTURE.EXPORT_SUCCEEDS");

    // A consistent snapshot through SQLite's own facility. Copying the main database file alone
    // would be a torn read of a WAL store: committed frames that have not been checkpointed live
    // in `-wal`, so the file by itself can be missing the publication that was just written.
    // `VACUUM INTO` reads one consistent view and writes a single self-contained file, and it
    // refuses if the destination already exists.
    let snapshot_path = output.join(FIXTURE_SNAPSHOT_FILE);
    Connection::open(&fixture.store.path)
        .expect("B18.FIXTURE.LIVE_STORE_OPENS")
        .execute(
            "VACUUM INTO ?1",
            params![
                snapshot_path
                    .to_str()
                    .expect("B18.FIXTURE.SNAPSHOT_PATH_IS_UTF8")
            ],
        )
        .expect("B18.FIXTURE.VACUUM_INTO_SUCCEEDS");
    fs::set_permissions(&snapshot_path, fs::Permissions::from_mode(0o600))
        .expect("B18.FIXTURE.SNAPSHOT_IS_0600");

    let markdown_path = output.join(FIXTURE_MARKDOWN_FILE);
    write_private_file(&markdown_path, exported.bytes());

    let markdown_sha256 = sha256_hex_of(exported.bytes());
    let snapshot_bytes = fs::read(&snapshot_path).expect("B18.FIXTURE.SNAPSHOT_READS");
    let snapshot_sha256 = sha256_hex_of(&snapshot_bytes);
    let schema_version = declared_schema_version_of(&snapshot_path);
    let source_head = fixture_source_head();

    // Identity and digests only. No passphrase, no recovery key, no auth token: the manifest is
    // the thing a later check reads, and a fixture manifest that carries the way in is a
    // credential file with a different name.
    let manifest_path = output.join(FIXTURE_MANIFEST_FILE);
    let manifest = serde_json::json!({
        "fixture": "beacon.b18.outcome-fixture.v1",
        "source_head": source_head,
        "schema_version": schema_version,
        "workspace_id": fixture.workspace_id.to_string(),
        "mission_id": fixture.mission_id.to_string(),
        "publication_version": exported.publication_version(),
        "publication_version_count": exported.publication_version_count(),
        "candidate_artifact_digest": exported.digest().value.clone(),
        "media_type": exported.media_type(),
        "markdown_file": FIXTURE_MARKDOWN_FILE,
        "markdown_bytes": exported.bytes().len(),
        "markdown_sha256": markdown_sha256,
        "snapshot_file": FIXTURE_SNAPSHOT_FILE,
        "snapshot_bytes": snapshot_bytes.len(),
        // The digest of the snapshot AS MINTED. Opening a store writes to it, so a consumer
        // verifies this before its first open, not after.
        "snapshot_sha256": snapshot_sha256,
    });
    write_private_file(
        &manifest_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("B18.FIXTURE.MANIFEST_SERIALIZES")
        )
        .as_bytes(),
    );

    Ok(MintedOutcomeFixture {
        directory: output.to_path_buf(),
        snapshot_path,
        markdown_path,
        manifest_path,
        source_head,
        schema_version,
        workspace_id: fixture.workspace_id,
        mission_id: fixture.mission_id,
        publication_version: exported.publication_version(),
        publication_version_count: exported.publication_version_count(),
        candidate_artifact_digest: exported.digest().value.clone(),
        media_type: exported.media_type().to_owned(),
        markdown_bytes: exported.bytes().to_vec(),
        markdown_sha256,
        snapshot_sha256,
    })
}

/// A minted snapshot reopened as a private COPY, through the ordinary owner services.
///
/// Shared by both readbacks. The copy is a new `0600` file under a `0700` directory whose guard
/// removes it -- and any `-wal`/`-shm` sibling opening leaves -- even if a later assertion unwinds.
struct ReopenedFixtureStore {
    _prefix: PrivatePrefix,
    path: PathBuf,
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    workspace_created: bool,
}

fn reopen_minted_snapshot(snapshot_bytes: &[u8]) -> ReopenedFixtureStore {
    let prefix = PrivatePrefix::new("readback");
    let path = prefix.path.join("store.sqlite3");
    write_private_file(&path, snapshot_bytes);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("B18.READBACK.STARTUP_SUCCEEDS");

    let accounts =
        Arc::new(OperatorAccountService::open(&path).expect("B18.READBACK.ACCOUNT_SERVICE_OPENS"));
    assert_eq!(
        accounts
            .authenticate_password(
                SecretString::new(FIXTURE_PASSPHRASE.to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("B18.READBACK.STORE_UNLOCKS")
            .phase,
        OperatorSessionPhase::Unlocked
    );

    ReopenedFixtureStore {
        _prefix: prefix,
        path,
        accounts,
        workspace_id: report.workspace.id,
        workspace_created: report.workspace.created,
    }
}

/// Reopens a byte-identical COPY of the minted snapshot through the ordinary owner services.
///
/// A copy, and not the artifact itself, because opening writes: `Supervisor::bootstrap` runs
/// reconciliation inside a transaction and the account service leaves `-wal`/`-shm` beside the
/// store. The recorded `snapshot_sha256` is what a later consumer checks BEFORE its first open, so
/// this proof must not be the thing that invalidates it -- and the caller asserts afterwards that
/// the artifact is still exactly the bytes its manifest names.
fn read_back_minted_fixture(minted: &MintedOutcomeFixture) -> MintedFixtureReadBack {
    let snapshot_bytes = fs::read(&minted.snapshot_path).expect("B18.READBACK.SNAPSHOT_READS");
    assert_eq!(
        sha256_hex_of(&snapshot_bytes),
        minted.snapshot_sha256,
        "B18.READBACK.SNAPSHOT_IS_THE_MINTED_BYTES"
    );
    let store = reopen_minted_snapshot(&snapshot_bytes);
    let opened = store.path.clone();
    let report_workspace_id = store.workspace_id;
    let report_workspace_created = store.workspace_created;
    let outcomes = OutcomeService::new(Arc::clone(&store.accounts), report_workspace_id);
    let history = outcomes
        .publication_history(minted.mission_id)
        .expect("B18.READBACK.PUBLICATION_HISTORY_READS");
    let exported = outcomes
        .export_markdown_version(minted.mission_id, minted.publication_version)
        .expect("B18.READBACK.VERSION_EXPORTS");

    MintedFixtureReadBack {
        workspace_id: report_workspace_id,
        workspace_created: report_workspace_created,
        schema_version: declared_schema_version_of(&opened),
        publication_versions: history
            .iter()
            .map(|row| {
                (
                    row.publication_version,
                    row.is_current,
                    row.candidate_artifact_digest.clone(),
                )
            })
            .collect(),
        exported_bytes: exported.bytes().to_vec(),
        exported_digest: exported.digest().value.clone(),
        publication_version: exported.publication_version(),
        publication_version_count: exported.publication_version_count(),
        media_type: exported.media_type().to_owned(),
    }
}

/// A task-owned private prefix the bar creates for itself: `0700` at creation, under the temp
/// directory, removed when the test ends.
///
/// The bars mint inside one of these rather than inside `TMPDIR` directly, so they hold whatever
/// `TMPDIR` happens to be -- including a bare `/tmp`, which the guard now refuses as a prefix. A
/// test whose result depends on how the operator exported one variable is measuring the operator.
struct PrivatePrefix {
    path: PathBuf,
}

impl PrivatePrefix {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("b18-prefix-{label}-{}", Uuid::new_v4().simple()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("B18.PREFIX.CREATED");
        Self { path }
    }
}

impl Drop for PrivatePrefix {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A path that does not exist yet, removed when the test ends.
struct PrivateOutput {
    path: PathBuf,
}

impl PrivateOutput {
    fn under(prefix: &PrivatePrefix, label: &str) -> Self {
        Self {
            path: prefix
                .path
                .join(format!("b18-{label}-{}", Uuid::new_v4().simple())),
        }
    }
}

impl Drop for PrivateOutput {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// Bar 1: the public owner interfaces really seed a published Outcome, and the mint writes exactly
/// three private files describing it.
#[test]
fn a_minted_outcome_fixture_seeds_one_published_outcome_into_three_private_files() {
    let prefix = PrivatePrefix::new("seed");
    let output = PrivateOutput::under(&prefix, "seed");
    let minted = mint_private_outcome_fixture_within(&prefix.path, Some(&output.path))
        .expect("B18.MINT.SUCCEEDS");

    assert_eq!(
        mode_of(&minted.directory),
        0o700,
        "B18.MINT.DIRECTORY_IS_0700"
    );
    assert_eq!(
        mode_of(&minted.snapshot_path),
        0o600,
        "B18.MINT.SNAPSHOT_IS_0600"
    );
    assert_eq!(
        mode_of(&minted.markdown_path),
        0o600,
        "B18.MINT.MARKDOWN_IS_0600"
    );
    assert_eq!(
        mode_of(&minted.manifest_path),
        0o600,
        "B18.MINT.MANIFEST_IS_0600"
    );
    let mut written: Vec<String> = fs::read_dir(&minted.directory)
        .expect("B18.MINT.DIRECTORY_LISTS")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![
            FIXTURE_MARKDOWN_FILE.to_owned(),
            FIXTURE_MANIFEST_FILE.to_owned(),
            FIXTURE_SNAPSHOT_FILE.to_owned(),
        ],
        "B18.MINT.WRITES_EXACTLY_THREE_FILES"
    );

    // The document says something. Without this, an empty document passes every digest, length and
    // byte comparison in this section by agreeing with itself.
    let document =
        String::from_utf8(minted.markdown_bytes.clone()).expect("B18.MINT.MARKDOWN_IS_UTF8");
    assert!(
        document.contains(FIXTURE_MISSION_DECLARATION),
        "B18.MINT.DOCUMENT_CARRIES_THE_SEEDED_MISSION_DECLARATION: {document}"
    );
    assert!(
        document.contains(CONDITION_TEXT),
        "B18.MINT.DOCUMENT_CARRIES_THE_SEEDED_CONDITION: {document}"
    );
    assert!(
        minted.markdown_bytes.len() < 64 * 1024,
        "B18.MINT.DOCUMENT_FITS_THE_WIRE_BOUND: {} bytes",
        minted.markdown_bytes.len()
    );
    assert_eq!(minted.media_type, "text/markdown; charset=utf-8");
    assert_eq!(minted.publication_version, 1);
    assert_eq!(minted.publication_version_count, 1);

    // The retained file is the retained bytes, and the recorded digests cover them.
    assert_eq!(
        fs::read(&minted.markdown_path).expect("B18.MINT.MARKDOWN_READS"),
        minted.markdown_bytes,
        "B18.MINT.MARKDOWN_FILE_IS_THE_EXPORTED_BYTES"
    );
    assert_eq!(
        sha256_hex_of(&minted.markdown_bytes),
        minted.markdown_sha256
    );
    assert_eq!(
        sha256_hex_of(&fs::read(&minted.snapshot_path).expect("B18.MINT.SNAPSHOT_READS")),
        minted.snapshot_sha256
    );

    // The manifest describes that fixture and carries no way into it.
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&minted.manifest_path).expect("B18.MINT.MANIFEST_READS"))
            .expect("B18.MINT.MANIFEST_IS_JSON");
    assert_eq!(
        manifest["source_head"], minted.source_head,
        "B18.MINT.MANIFEST_RECORDS_THE_SOURCE_HEAD"
    );
    assert_eq!(
        manifest["schema_version"], minted.schema_version,
        "B18.MINT.MANIFEST_RECORDS_THE_SCHEMA_VERSION"
    );
    assert_eq!(
        manifest["mission_id"],
        minted.mission_id.to_string(),
        "B18.MINT.MANIFEST_RECORDS_THE_MISSION"
    );
    assert_eq!(
        manifest["workspace_id"],
        minted.workspace_id.to_string(),
        "B18.MINT.MANIFEST_RECORDS_THE_WORKSPACE"
    );
    assert_eq!(
        manifest["publication_version"], 1,
        "B18.MINT.MANIFEST_RECORDS_THE_VERSION"
    );
    assert_eq!(
        manifest["candidate_artifact_digest"], minted.candidate_artifact_digest,
        "B18.MINT.MANIFEST_RECORDS_THE_CANDIDATE_DIGEST"
    );
    assert_eq!(
        manifest["snapshot_sha256"], minted.snapshot_sha256,
        "B18.MINT.MANIFEST_RECORDS_THE_SNAPSHOT_DIGEST"
    );
    assert_eq!(
        manifest["markdown_sha256"], minted.markdown_sha256,
        "B18.MINT.MANIFEST_RECORDS_THE_MARKDOWN_DIGEST"
    );
    let manifest_text = fs::read_to_string(&minted.manifest_path).unwrap();
    assert!(
        !manifest_text.contains(FIXTURE_PASSPHRASE),
        "B18.MINT.MANIFEST_CARRIES_NO_PASSPHRASE"
    );
}

/// Bar 2: the snapshot reopens through the normal owner and returns the exact retained bytes and
/// the same publication -- and the artifact survives being read.
#[test]
fn the_minted_snapshot_reopens_through_the_normal_owner_with_the_exact_bytes_and_publication() {
    let prefix = PrivatePrefix::new("readback");
    let output = PrivateOutput::under(&prefix, "readback");
    let minted = mint_private_outcome_fixture_within(&prefix.path, Some(&output.path))
        .expect("B18.MINT.SUCCEEDS");
    let read_back = read_back_minted_fixture(&minted);

    // Startup opened the SEEDED Workspace, and did not create a new one -- the difference between
    // reading the fixture and quietly bootstrapping an empty store beside it.
    assert!(
        !read_back.workspace_created,
        "B18.READBACK.WORKSPACE_WAS_NOT_CREATED_FRESH"
    );
    assert_eq!(
        read_back.workspace_id, minted.workspace_id,
        "B18.READBACK.SAME_WORKSPACE"
    );
    assert_eq!(
        read_back.schema_version, minted.schema_version,
        "B18.READBACK.SAME_SCHEMA_VERSION"
    );

    assert_eq!(
        read_back.publication_versions,
        vec![(
            minted.publication_version,
            true,
            minted.candidate_artifact_digest.clone()
        )],
        "B18.READBACK.SAME_PUBLICATION"
    );
    assert_eq!(
        read_back.exported_bytes, minted.markdown_bytes,
        "B18.READBACK.EXACT_RETAINED_BYTES"
    );
    assert_eq!(
        read_back.exported_digest, minted.candidate_artifact_digest,
        "B18.READBACK.SAME_CANDIDATE_DIGEST"
    );
    assert_eq!(
        (
            read_back.publication_version,
            read_back.publication_version_count
        ),
        (minted.publication_version, minted.publication_version_count),
        "B18.READBACK.SAME_VERSION_OF_SAME_COUNT"
    );
    assert_eq!(read_back.media_type, minted.media_type);
    let document =
        String::from_utf8(read_back.exported_bytes.clone()).expect("B18.READBACK.MARKDOWN_IS_UTF8");
    assert!(
        document.contains(FIXTURE_MISSION_DECLARATION) && document.contains(CONDITION_TEXT),
        "B18.READBACK.DOCUMENT_CARRIES_THE_SEEDED_NARRATIVE"
    );

    // And the artifact a later check will verify is still the bytes its manifest names.
    assert_eq!(
        sha256_hex_of(&fs::read(&minted.snapshot_path).expect("B18.READBACK.SNAPSHOT_STILL_READS")),
        minted.snapshot_sha256,
        "B18.READBACK.MINTED_SNAPSHOT_SURVIVES_ITS_OWN_PROOF"
    );
}

/// Bar 3: an occupied path, a symlink and a path outside the private prefix are all refused, and
/// the sentinel that was already there is byte-identical afterwards.
#[test]
fn a_mint_refuses_an_occupied_path_a_symlink_or_a_public_prefix_without_touching_anything() {
    const SENTINEL: &[u8] = b"B18 sentinel: these bytes were here first.\n";

    // (a) A directory that already exists, holding a file.
    let prefix = PrivatePrefix::new("refuse");
    let occupied = PrivateOutput::under(&prefix, "occupied");
    fs::create_dir(&occupied.path).unwrap();
    let sentinel_file = occupied.path.join("sentinel.txt");
    fs::write(&sentinel_file, SENTINEL).unwrap();
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&occupied.path))
            .expect_err("an occupied path minted"),
        FixtureMintRefusal::Occupied(occupied.path.clone())
    );
    assert_eq!(
        fs::read(&sentinel_file).unwrap(),
        SENTINEL,
        "B18.REFUSE.SENTINEL_IS_BYTE_IDENTICAL"
    );
    assert_eq!(
        fs::read_dir(&occupied.path).unwrap().count(),
        1,
        "B18.REFUSE.NOTHING_WAS_WRITTEN_BESIDE_THE_SENTINEL"
    );

    // (b) A symlink pointing at that same directory. `exists()` follows the link and would have
    // called this "a directory that is there"; a DANGLING link it would have called absent. Both
    // are things that are already at the path, and both are refused as such.
    let link = PrivateOutput::under(&prefix, "symlink");
    std::os::unix::fs::symlink(&occupied.path, &link.path).unwrap();
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&link.path))
            .expect_err("a symlink minted"),
        FixtureMintRefusal::Occupied(link.path.clone())
    );
    assert_eq!(
        fs::read(&sentinel_file).unwrap(),
        SENTINEL,
        "B18.REFUSE.SYMLINK_TARGET_IS_BYTE_IDENTICAL"
    );
    assert!(
        fs::symlink_metadata(&link.path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "B18.REFUSE.SYMLINK_WAS_NOT_REPLACED"
    );

    // (c) A dangling symlink: absent to `exists()`, present to the refusal.
    let dangling = PrivateOutput::under(&prefix, "dangling");
    std::os::unix::fs::symlink(prefix.path.join("b18-no-such-target"), &dangling.path).unwrap();
    assert!(
        !dangling.path.exists(),
        "the dangling link must look absent"
    );
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&dangling.path))
            .expect_err("a dangling symlink minted"),
        FixtureMintRefusal::Occupied(dangling.path.clone())
    );

    // (d) Outside the private prefix entirely -- refused for THAT reason, not for existing.
    let public = PathBuf::from("/etc/beacon-b18-never-written");
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&public))
            .expect_err("a public path minted"),
        FixtureMintRefusal::NotPrivate(public.clone())
    );
    assert!(!public.exists(), "B18.REFUSE.PUBLIC_PATH_WAS_NOT_CREATED");
    let relative = PathBuf::from("b18-relative");
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&relative))
            .expect_err("a relative path minted"),
        FixtureMintRefusal::NotPrivate(relative.clone())
    );
    assert!(
        !relative.exists(),
        "B18.REFUSE.RELATIVE_PATH_WAS_NOT_CREATED"
    );

    // (e) The private prefix ITSELF is not a mint target: a fixture directory is one this mint
    // created, never the parent everything else under it is already using.
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&prefix.path))
            .expect_err("the prefix itself minted"),
        FixtureMintRefusal::NotPrivate(prefix.path.clone())
    );

    // (f) Astra's two escapes, at the parameterized boundary as well as the environment one: a
    // `..` component and an intermediate directory symlink both LAND outside the prefix, however
    // they are spelled. The peer they escape to is created by this bar and holds a sentinel.
    let peer = PrivatePrefix::new("peer");
    let peer_sentinel = peer.path.join("sentinel.txt");
    fs::write(&peer_sentinel, SENTINEL).unwrap();
    let traversal = prefix
        .path
        .join("..")
        .join(peer.path.file_name().unwrap())
        .join("mint");
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&traversal))
            .expect_err("a `..` traversal minted"),
        FixtureMintRefusal::NotPrivate(traversal.clone())
    );
    let alias = prefix.path.join("alias");
    std::os::unix::fs::symlink(&peer.path, &alias).unwrap();
    let through_link = alias.join("mint");
    assert_eq!(
        mint_private_outcome_fixture_within(&prefix.path, Some(&through_link))
            .expect_err("a parent symlink minted"),
        FixtureMintRefusal::NotPrivate(through_link)
    );
    assert!(
        !peer.path.join("mint").exists(),
        "B18.REFUSE.NOTHING_WAS_WRITTEN_OUTSIDE_THE_PREFIX"
    );
    assert_eq!(
        fs::read(&peer_sentinel).unwrap(),
        SENTINEL,
        "B18.REFUSE.PEER_SENTINEL_IS_BYTE_IDENTICAL"
    );
}

/// Bar 5: a prefix that is not task-owned and private is refused, so nothing minted under it can
/// claim to be private either.
///
/// Parameterized rather than environment-driven on purpose: `std::env::set_var` is unsafe and
/// process-global, and a test that flips `TMPDIR` under a binary running its cases in parallel
/// changes the answer for every other test in it.
#[test]
fn a_mint_refuses_a_prefix_that_is_not_task_owned_and_private() {
    let prefix = PrivatePrefix::new("badprefix");
    let output = PrivateOutput::under(&prefix, "unused");

    // `/tmp` itself: it exists, it is a directory, and this process may write in it -- and it is
    // `1777`, shared with every process on the machine. A different fault from "left too open".
    let shared = PathBuf::from("/tmp");
    assert_eq!(
        mint_private_outcome_fixture_within(&shared, Some(&shared.join("b18-never")))
            .expect_err("the shared /tmp minted"),
        FixtureMintRefusal::PrefixNotPrivate(shared, PrefixFault::IsSharedTmp)
    );

    // Ours, under /tmp, a directory -- and group- and world-readable.
    let open = prefix.path.join("open-prefix");
    fs::DirBuilder::new().mode(0o755).create(&open).unwrap();
    assert_eq!(
        mint_private_outcome_fixture_within(&open, Some(&open.join("mint")))
            .expect_err("a 0755 prefix minted"),
        FixtureMintRefusal::PrefixNotPrivate(
            open.clone(),
            PrefixFault::GroupOrWorldAccessible { mode: 0o755 }
        )
    );

    // A regular file, a path that is not there at all, one outside /tmp, and a relative one.
    let file = prefix.path.join("not-a-directory");
    fs::write(&file, b"x").unwrap();
    assert_eq!(
        mint_private_outcome_fixture_within(&file, Some(&file.join("mint")))
            .expect_err("a file prefix minted"),
        FixtureMintRefusal::PrefixNotPrivate(file, PrefixFault::NotADirectory)
    );
    let missing = prefix.path.join("no-such-prefix");
    assert_eq!(
        mint_private_outcome_fixture_within(&missing, Some(&missing.join("mint")))
            .expect_err("a missing prefix minted"),
        FixtureMintRefusal::PrefixNotPrivate(missing, PrefixFault::Missing)
    );
    let outside = PathBuf::from("/etc");
    assert_eq!(
        mint_private_outcome_fixture_within(&outside, Some(&outside.join("b18-never")))
            .expect_err("a prefix outside /tmp minted"),
        FixtureMintRefusal::PrefixNotPrivate(outside, PrefixFault::NotUnderTmp)
    );
    let relative = PathBuf::from("b18-relative-prefix");
    assert_eq!(
        mint_private_outcome_fixture_within(&relative, Some(&output.path))
            .expect_err("a relative prefix minted"),
        FixtureMintRefusal::PrefixNotPrivate(relative, PrefixFault::NotAbsolute)
    );

    // The prefix is checked BEFORE the output, so a bad prefix is never rescued by a good output
    // and nothing was created anywhere.
    assert!(!output.path.exists(), "B18.PREFIX.NOTHING_WAS_CREATED");
    assert!(!PathBuf::from("/tmp/b18-never").exists());
    assert!(!PathBuf::from("/etc/b18-never").exists());
}

/// Bar 4: no output named at all refuses BY NAME, rather than passing having written nowhere.
#[test]
fn a_mint_with_no_output_named_refuses_by_name() {
    let refusal = mint_private_outcome_fixture(None).expect_err("a mint with no output succeeded");
    assert_eq!(refusal, FixtureMintRefusal::Unset);
    assert!(
        refusal.to_string().starts_with("B18.FIXTURE.OUTPUT_UNSET:"),
        "the refusal does not name itself: {refusal}"
    );
    assert!(
        refusal.to_string().contains("BEACON_B18_FIXTURE_DIR"),
        "the refusal does not say how to name an output: {refusal}"
    );
}

/// Mints the private fixture into a directory you name. Not part of the suite: it writes files
/// outside the repository and is run deliberately, for one packaged read check, then deleted.
///
/// **Two steps, and the first one is not optional.** The prefix this mints inside is
/// `std::env::temp_dir()`, and a bare `/tmp` is refused: `/tmp` is `1777` and shared with every
/// process on the machine, so a fixture in it is not private in any sense the name claims. Make a
/// task-owned one first.
///
///     export TMPDIR="$(mktemp -d)"    # 0700, yours, under /tmp -- the prefix the guard requires
///     BEACON_B18_FIXTURE_DIR="$(mktemp -u "$TMPDIR/beacon-b18-XXXXXX")" \
///         cargo test -p beacond --test outcome_prepare -- --ignored --exact \
///         mint_private_outcome_fixture_into_the_named_directory --nocapture
///
/// `mktemp -d` creates the prefix; `mktemp -u` names the output WITHOUT creating it, which is what
/// the mint requires -- an output that already exists is refused rather than written into. The two
/// ways this run fails rather than passing quietly: `BEACON_B18_FIXTURE_DIR` unset gives
/// `B18.FIXTURE.OUTPUT_UNSET`, and a `TMPDIR` that is not a task-owned private directory gives
/// `B18.FIXTURE.PREFIX_NOT_PRIVATE` with the specific fault.
///
/// **What the recorded `source_head` proves.** It is `git rev-parse HEAD` in the tree the test was
/// compiled from, so it names the commit whose generator produced these bytes -- provided the tree
/// was CLEAN when it ran. It is not a build fingerprint: it cannot tell a reader whether the
/// working tree had uncommitted edits, and a fixture minted before its own generator was committed
/// records the earlier commit and is a development sample, not provenance for a packaged check.
/// Mint the retained sample from a clean tree, after the commit, and check
/// `git status --porcelain` is empty when you do.
///
/// The fixture is private and disposable: three files under your own temp prefix, `0700`/`0600`,
/// none of them committed. Delete the directory when the check that used it is done.
#[test]
#[ignore = "mints a private fixture into a directory you name; run deliberately, never as part of the suite"]
fn mint_private_outcome_fixture_into_the_named_directory() {
    let named = std::env::var_os("BEACON_B18_FIXTURE_DIR").map(PathBuf::from);
    let minted = mint_private_outcome_fixture(named.as_deref())
        .unwrap_or_else(|refusal| panic!("{refusal}"));

    // A fixture nobody proved is worse than none: the mint verifies its own artifact before it
    // tells anyone the path.
    let read_back = read_back_minted_fixture(&minted);
    assert_eq!(read_back.exported_bytes, minted.markdown_bytes);
    assert_eq!(read_back.workspace_id, minted.workspace_id);
    assert!(!read_back.workspace_created);

    println!("B18 fixture minted at {}", minted.directory.display());
    println!("  source head        {}", minted.source_head);
    println!("  schema version     {}", minted.schema_version);
    println!("  mission            {}", minted.mission_id);
    println!(
        "  publication        version {} of {}",
        minted.publication_version, minted.publication_version_count
    );
    println!("  snapshot           {}", minted.snapshot_path.display());
    println!("  snapshot sha256    {}", minted.snapshot_sha256);
    println!("  expected markdown  {}", minted.markdown_path.display());
    println!("  markdown sha256    {}", minted.markdown_sha256);
    println!("  manifest           {}", minted.manifest_path.display());
}

#[test]
fn astra_b18_parent_traversal_cannot_escape_the_private_prefix() {
    // Own both peers: a bare /tmp must not make this probe try creating a directory under /.
    // Pass the private prefix explicitly instead of changing the process-wide environment.
    let inside = PrivatePrefix::new("astra-inside");
    let prefix = &inside.path;
    let outside = PrivatePrefix::new("astra-outside");
    let sentinel = outside.path.join("sentinel");
    fs::write(&sentinel, b"retained sibling").unwrap();
    let output = prefix
        .join("..")
        .join(outside.path.file_name().unwrap())
        .join("mint");
    let result = mint_private_outcome_fixture_within(prefix, Some(&output));
    let created = outside.path.join("mint").exists();
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"retained sibling",
        "B18.OUTPUT.EXISTING_SIBLING_UNCHANGED"
    );
    assert_eq!(
        result.err(),
        Some(FixtureMintRefusal::NotPrivate(output)),
        "B18.OUTPUT.NO_PARENT_ESCAPE"
    );
    assert!(!created, "B18.OUTPUT.NO_OUTSIDE_WRITES");
}

#[test]
fn astra_b18_parent_symlink_cannot_escape_the_private_prefix() {
    // Own both peers: a bare /tmp must not make this probe try creating a directory under /.
    // Pass the private prefix explicitly instead of changing the process-wide environment.
    let inside = PrivatePrefix::new("astra-inside");
    let prefix = &inside.path;
    let outside = PrivatePrefix::new("astra-outside");
    let links = PrivateOutput {
        path: prefix.join("parent-link"),
    };
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&links.path)
        .unwrap();
    std::os::unix::fs::symlink(&outside.path, links.path.join("alias")).unwrap();
    let sentinel = outside.path.join("sentinel");
    fs::write(&sentinel, b"retained sibling").unwrap();
    let output = links.path.join("alias").join("mint");
    let result = mint_private_outcome_fixture_within(prefix, Some(&output));
    let created = outside.path.join("mint").exists();
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"retained sibling",
        "B18.OUTPUT.EXISTING_SIBLING_UNCHANGED"
    );
    assert_eq!(
        result.err(),
        Some(FixtureMintRefusal::NotPrivate(output)),
        "B18.OUTPUT.NO_PARENT_SYMLINK_ESCAPE"
    );
    assert!(!created, "B18.OUTPUT.NO_OUTSIDE_WRITES");
}

// ---------------------------------------------------------------------------------------------
// B28: a TWO-publication private fixture, so a packaged selection test can discriminate
// ---------------------------------------------------------------------------------------------
//
// B26 measured why this is needed. With one publication, `MissionOutcomes` auto-selects it on
// load, so version 1's document and Copy control are already on screen BEFORE anything is clicked
// (`b26 evidence-940x800/21-initial.png`). A predicate that looks for "Version 1 of 1" or "Copy
// Markdown" after clicking Show therefore passes whether or not the click did anything. That is a
// gap in the harness and the brief, not a product-handler failure -- and the fix is a fixture where
// the requested version is NOT the one already selected.
//
// The recipe is the file's own: publish version 1 through `publishable`, then prepare a correction
// with `AppendixRequest::Include` -- exactly what
// `the_pdf_of_a_superseded_version_is_rendered_from_that_versions_bytes` does -- evaluate it
// independently, and publish it as version 2. Different appendix request, different canonical
// bytes, different candidate digest. Nothing is planted and no snapshot is edited.

const TWO_VERSION_MANIFEST_FILE: &str = "fixture-two-version.json";
const TWO_VERSION_V1_FILE: &str = "expected-outcome-v1.md";
const TWO_VERSION_V2_FILE: &str = "expected-outcome-v2.md";

/// One published version of a two-version fixture, as it was exported at mint time.
#[derive(Debug, Clone)]
struct MintedPublication {
    version: u32,
    is_current: bool,
    candidate_artifact_digest: String,
    markdown_file: &'static str,
    markdown_path: PathBuf,
    markdown_bytes: Vec<u8>,
    markdown_sha256: String,
}

#[derive(Debug)]
struct MintedTwoVersionFixture {
    directory: PathBuf,
    snapshot_path: PathBuf,
    manifest_path: PathBuf,
    source_head: String,
    schema_version: i64,
    workspace_id: Uuid,
    mission_id: Uuid,
    media_type: String,
    publications: Vec<MintedPublication>,
    snapshot_sha256: String,
}

impl MintedTwoVersionFixture {
    fn at(&self, version: u32) -> &MintedPublication {
        self.publications
            .iter()
            .find(|p| p.version == version)
            .expect("B28.FIXTURE.VERSION_PRESENT")
    }
}

#[derive(Debug)]
struct TwoVersionReadBack {
    workspace_id: Uuid,
    workspace_created: bool,
    history: Vec<(u32, bool, String)>,
    exports: Vec<(u32, u32, Vec<u8>, String)>,
}

/// Mints a private fixture holding TWO publications of one Mission.
///
/// Reuses `prepare_private_output` for the prefix/parent/no-overwrite guards and the `0700`
/// directory, and the same `VACUUM INTO` snapshot as the one-version recipe. The one-version
/// recipe, its `fixture.json` schema and its three-file output are untouched: this writes its own
/// manifest filename and its own per-version Markdown files, because a reader that cannot tell the
/// two shapes apart is a reader that will eventually mistake one for the other.
/// How much unfinished work the seeded completion records -- the ONLY thing that differs between
/// the ordinary two-version fixture and the large one.
///
/// A parameter rather than a second generator: the recipe, the guards, the snapshot, the manifest
/// and the readback are all the same code, so the two fixtures cannot drift apart in any respect
/// except the one they are meant to differ in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureScale {
    /// No unfinished work: the B28 default, unchanged.
    Ordinary,
    /// The completion's maximum unfinished-work items, which pushes the exported Markdown's
    /// serialized `number[]` payload past the old command-specific response allowance.
    LargeUnfinishedWork,
}

fn mint_two_version_outcome_fixture_within(
    prefix: &Path,
    output: Option<&Path>,
) -> Result<MintedTwoVersionFixture, FixtureMintRefusal> {
    mint_two_version_fixture_with_scale(prefix, output, FixtureScale::Ordinary)
}

fn mint_two_version_fixture_with_scale(
    prefix: &Path,
    output: Option<&Path>,
    scale: FixtureScale,
) -> Result<MintedTwoVersionFixture, FixtureMintRefusal> {
    let output = prepare_private_output(prefix, output)?;
    let output = output.as_path();

    let fixture = admitted_mission("b28-two-version-fixture");
    fixture.classify(ImpactLevel::Ordinary);
    match scale {
        FixtureScale::Ordinary => fixture.complete((fixture.resident_id, fixture.configuration_id)),
        FixtureScale::LargeUnfinishedWork => fixture.complete_with(
            (fixture.resident_id, fixture.configuration_id),
            large_unfinished_work_items(),
        ),
    };

    let (_, first_digest, first_evaluation) = fixture.publishable();
    fixture
        .outcomes
        .publish(fixture.mission_id, &first_digest, &first_evaluation)
        .expect("B28.FIXTURE.V1_PUBLISHES");

    // The correction. `Include` rather than `PolicyDefault`, so the request is inside the manifest
    // and the canonical bytes differ even though this Mission is ordinary.
    let corrected = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::Include)
        .expect("B28.FIXTURE.V2_PREPARES");
    let corrected_record = fixture
        .artifacts()
        .get(&corrected.candidate_artifact_digest)
        .expect("B28.FIXTURE.V2_CANDIDATE_READS");
    let corrected_evaluation =
        fixture.evaluate(&corrected.candidate_artifact_digest, Verdict::Meets);
    fixture
        .outcomes
        .publish(
            fixture.mission_id,
            corrected_record.digest(),
            corrected_evaluation.digest(),
        )
        .expect("B28.FIXTURE.V2_PUBLISHES");

    let one = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 1)
        .expect("B28.FIXTURE.V1_EXPORTS");
    let two = fixture
        .outcomes
        .export_markdown_version(fixture.mission_id, 2)
        .expect("B28.FIXTURE.V2_EXPORTS");

    // The whole point of this fixture. A mint that produced two identical versions would hand a
    // later selection test a predicate that still cannot discriminate, so it refuses here instead.
    assert_ne!(
        one.bytes(),
        two.bytes(),
        "B28.FIXTURE.VERSIONS_DIFFER_IN_BYTES"
    );
    assert_ne!(
        one.digest().value,
        two.digest().value,
        "B28.FIXTURE.VERSIONS_DIFFER_IN_DIGEST"
    );
    assert_eq!(
        (one.publication_version(), one.publication_version_count()),
        (1, 2),
        "B28.FIXTURE.V1_IS_ONE_OF_TWO"
    );
    assert_eq!(
        (two.publication_version(), two.publication_version_count()),
        (2, 2),
        "B28.FIXTURE.V2_IS_TWO_OF_TWO"
    );

    let snapshot_path = output.join(FIXTURE_SNAPSHOT_FILE);
    Connection::open(&fixture.store.path)
        .expect("B28.FIXTURE.LIVE_STORE_OPENS")
        .execute(
            "VACUUM INTO ?1",
            params![
                snapshot_path
                    .to_str()
                    .expect("B28.FIXTURE.SNAPSHOT_PATH_IS_UTF8")
            ],
        )
        .expect("B28.FIXTURE.VACUUM_INTO_SUCCEEDS");
    fs::set_permissions(&snapshot_path, fs::Permissions::from_mode(0o600))
        .expect("B28.FIXTURE.SNAPSHOT_IS_0600");

    let mut publications = Vec::new();
    for (exported, file) in [(&one, TWO_VERSION_V1_FILE), (&two, TWO_VERSION_V2_FILE)] {
        let markdown_path = output.join(file);
        write_private_file(&markdown_path, exported.bytes());
        publications.push(MintedPublication {
            version: exported.publication_version(),
            is_current: exported.publication_version() == exported.publication_version_count(),
            candidate_artifact_digest: exported.digest().value.clone(),
            markdown_file: file,
            markdown_path,
            markdown_bytes: exported.bytes().to_vec(),
            markdown_sha256: sha256_hex_of(exported.bytes()),
        });
    }

    let snapshot_bytes = fs::read(&snapshot_path).expect("B28.FIXTURE.SNAPSHOT_READS");
    let snapshot_sha256 = sha256_hex_of(&snapshot_bytes);
    let schema_version = declared_schema_version_of(&snapshot_path);
    let source_head = fixture_source_head();

    // Its OWN schema tag and filename. Identity and digests only -- no passphrase, no recovery key.
    let manifest_path = output.join(TWO_VERSION_MANIFEST_FILE);
    let manifest = serde_json::json!({
        "fixture": "beacon.b28.outcome-fixture.two-version.v1",
        "source_head": source_head,
        "schema_version": schema_version,
        "workspace_id": fixture.workspace_id.to_string(),
        "mission_id": fixture.mission_id.to_string(),
        "media_type": one.media_type(),
        "publication_version_count": 2,
        "publications": publications.iter().map(|p| serde_json::json!({
            "publication_version": p.version,
            "is_current": p.is_current,
            "candidate_artifact_digest": p.candidate_artifact_digest,
            "markdown_file": p.markdown_file,
            "markdown_bytes": p.markdown_bytes.len(),
            "markdown_sha256": p.markdown_sha256,
        })).collect::<Vec<_>>(),
        "snapshot_file": FIXTURE_SNAPSHOT_FILE,
        "snapshot_bytes": snapshot_bytes.len(),
        "snapshot_sha256": snapshot_sha256,
    });
    write_private_file(
        &manifest_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("B28.FIXTURE.MANIFEST_SERIALIZES")
        )
        .as_bytes(),
    );

    Ok(MintedTwoVersionFixture {
        directory: output.to_path_buf(),
        snapshot_path,
        manifest_path,
        source_head,
        schema_version,
        workspace_id: fixture.workspace_id,
        mission_id: fixture.mission_id,
        media_type: one.media_type().to_owned(),
        publications,
        snapshot_sha256,
    })
}

/// Reopens the two-version snapshot and exports EACH version at its own number.
fn read_back_two_version_fixture(minted: &MintedTwoVersionFixture) -> TwoVersionReadBack {
    let snapshot_bytes = fs::read(&minted.snapshot_path).expect("B28.READBACK.SNAPSHOT_READS");
    assert_eq!(
        sha256_hex_of(&snapshot_bytes),
        minted.snapshot_sha256,
        "B28.READBACK.SNAPSHOT_IS_THE_MINTED_BYTES"
    );
    let store = reopen_minted_snapshot(&snapshot_bytes);
    let outcomes = OutcomeService::new(Arc::clone(&store.accounts), store.workspace_id);

    let history = outcomes
        .publication_history(minted.mission_id)
        .expect("B28.READBACK.PUBLICATION_HISTORY_READS")
        .iter()
        .map(|row| {
            (
                row.publication_version,
                row.is_current,
                row.candidate_artifact_digest.clone(),
            )
        })
        .collect();

    let mut exports = Vec::new();
    for version in [1_u32, 2_u32] {
        let exported = outcomes
            .export_markdown_version(minted.mission_id, version)
            .expect("B28.READBACK.VERSION_EXPORTS");
        exports.push((
            exported.publication_version(),
            exported.publication_version_count(),
            exported.bytes().to_vec(),
            exported.digest().value.clone(),
        ));
    }

    TwoVersionReadBack {
        workspace_id: store.workspace_id,
        workspace_created: store.workspace_created,
        history,
        exports,
    }
}

/// The positive bar: two publications, different bytes, each reachable at its OWN number.
#[test]
fn a_two_version_fixture_reopens_and_exports_each_version_at_its_own_number() {
    let prefix = PrivatePrefix::new("b28-two");
    let output = PrivateOutput::under(&prefix, "two-version");
    let minted = mint_two_version_outcome_fixture_within(&prefix.path, Some(&output.path))
        .expect("B28.MINT.SUCCEEDS");

    // Four private files, and the one-version recipe's names are not among them.
    let mut written: Vec<String> = fs::read_dir(&minted.directory)
        .expect("B28.MINT.DIRECTORY_LISTS")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    written.sort();
    assert_eq!(
        written,
        vec![
            TWO_VERSION_V1_FILE.to_owned(),
            TWO_VERSION_V2_FILE.to_owned(),
            TWO_VERSION_MANIFEST_FILE.to_owned(),
            FIXTURE_SNAPSHOT_FILE.to_owned(),
        ],
        "B28.MINT.WRITES_ITS_OWN_FOUR_FILES"
    );
    assert_eq!(
        mode_of(&minted.directory),
        0o700,
        "B28.MINT.DIRECTORY_IS_0700"
    );
    for path in [
        &minted.snapshot_path,
        &minted.manifest_path,
        &minted.at(1).markdown_path,
        &minted.at(2).markdown_path,
    ] {
        assert_eq!(mode_of(path), 0o600, "B28.MINT.FILE_IS_0600");
    }

    let v1 = minted.at(1).clone();
    let v2 = minted.at(2).clone();
    assert_ne!(
        v1.markdown_bytes, v2.markdown_bytes,
        "B28.MINT.BYTES_DIFFER"
    );
    assert_ne!(
        v1.candidate_artifact_digest, v2.candidate_artifact_digest,
        "B28.MINT.DIGESTS_DIFFER"
    );
    assert!(!v1.is_current && v2.is_current, "B28.MINT.V2_IS_CURRENT");
    for p in [&v1, &v2] {
        assert_eq!(
            fs::read(&p.markdown_path).expect("B28.MINT.MARKDOWN_READS"),
            p.markdown_bytes,
            "B28.MINT.MARKDOWN_FILE_IS_THE_EXPORTED_BYTES"
        );
        assert!(
            String::from_utf8(p.markdown_bytes.clone())
                .expect("B28.MINT.MARKDOWN_IS_UTF8")
                .contains(FIXTURE_MISSION_DECLARATION),
            "B28.MINT.DOCUMENT_CARRIES_THE_SEEDED_DECLARATION"
        );
    }

    let read_back = read_back_two_version_fixture(&minted);
    assert!(
        !read_back.workspace_created,
        "B28.READBACK.WORKSPACE_WAS_NOT_CREATED_FRESH"
    );
    assert_eq!(read_back.workspace_id, minted.workspace_id);
    assert_eq!(
        read_back.history,
        vec![
            (2, true, v2.candidate_artifact_digest.clone()),
            (1, false, v1.candidate_artifact_digest.clone()),
        ],
        "B28.READBACK.HISTORY_IS_2_NEWEST_1_SUPERSEDED"
    );

    // Each version at its OWN number. This is the assertion the mutant below has to break: an
    // export that answers the newest for every request returns version 2's bytes here.
    let (v1_version, v1_count, v1_bytes, v1_digest) = read_back.exports[0].clone();
    let (v2_version, v2_count, v2_bytes, v2_digest) = read_back.exports[1].clone();
    assert_eq!(
        (v1_version, v1_count),
        (1, 2),
        "B28.READBACK.V1_REPORTS_1_OF_2"
    );
    assert_eq!(
        (v2_version, v2_count),
        (2, 2),
        "B28.READBACK.V2_REPORTS_2_OF_2"
    );
    assert_eq!(
        v1_bytes, v1.markdown_bytes,
        "B28.READBACK.V1_EXPORTS_ITS_OWN_BYTES"
    );
    assert_eq!(
        v2_bytes, v2.markdown_bytes,
        "B28.READBACK.V2_EXPORTS_ITS_OWN_BYTES"
    );
    assert_eq!(
        v1_digest, v1.candidate_artifact_digest,
        "B28.READBACK.V1_DIGEST"
    );
    assert_eq!(
        v2_digest, v2.candidate_artifact_digest,
        "B28.READBACK.V2_DIGEST"
    );
    assert_ne!(v1_bytes, v2_bytes, "B28.READBACK.THE_TWO_EXPORTS_DIFFER");

    // The manifest names both versions, and carries no way into the store.
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&minted.manifest_path).expect("B28.MINT.MANIFEST_READS"))
            .expect("B28.MINT.MANIFEST_IS_JSON");
    assert_eq!(
        manifest["fixture"], "beacon.b28.outcome-fixture.two-version.v1",
        "B28.MINT.MANIFEST_HAS_ITS_OWN_SCHEMA_TAG"
    );
    assert_eq!(manifest["source_head"], minted.source_head);
    assert_eq!(manifest["schema_version"], minted.schema_version);
    assert_eq!(manifest["snapshot_sha256"], minted.snapshot_sha256);
    assert_eq!(manifest["publication_version_count"], 2);
    for (index, p) in [(0, &v1), (1, &v2)] {
        let row = &manifest["publications"][index];
        assert_eq!(row["publication_version"], p.version);
        assert_eq!(row["is_current"], p.is_current);
        assert_eq!(
            row["candidate_artifact_digest"],
            p.candidate_artifact_digest
        );
        assert_eq!(row["markdown_file"], p.markdown_file);
        assert_eq!(row["markdown_sha256"], p.markdown_sha256);
    }
    assert!(
        !fs::read_to_string(&minted.manifest_path)
            .unwrap()
            .contains(FIXTURE_PASSPHRASE),
        "B28.MINT.MANIFEST_CARRIES_NO_PASSPHRASE"
    );
}

/// Mints the two-version fixture into a directory you name. Not part of the suite.
///
///     export TMPDIR="$(mktemp -d)"    # 0700, yours, under /tmp -- the prefix the guard requires
///     BEACON_B28_FIXTURE_DIR="$(mktemp -u "$TMPDIR/beacon-b28-XXXXXX")" \
///         cargo test -p beacond --test outcome_prepare -- --ignored --exact \
///         mint_two_version_outcome_fixture_into_the_named_directory --nocapture
///
/// Separate from the one-version manual test in every way that matters: its own env variable, its
/// own manifest filename and schema tag, and its own per-version Markdown files. Neither the
/// one-version recipe nor its `fixture.json` shape changes.
#[test]
#[ignore = "mints a two-version private fixture into a directory you name; run deliberately, never as part of the suite"]
fn mint_two_version_outcome_fixture_into_the_named_directory() {
    let named = std::env::var_os("BEACON_B28_FIXTURE_DIR").map(PathBuf::from);
    let prefix = std::env::temp_dir();
    let minted = mint_two_version_outcome_fixture_within(&prefix, named.as_deref())
        .unwrap_or_else(|refusal| panic!("{refusal}"));

    let read_back = read_back_two_version_fixture(&minted);
    assert_eq!(read_back.exports[0].2, minted.at(1).markdown_bytes);
    assert_eq!(read_back.exports[1].2, minted.at(2).markdown_bytes);
    assert_ne!(read_back.exports[0].2, read_back.exports[1].2);

    println!(
        "B28 two-version fixture minted at {}",
        minted.directory.display()
    );
    println!("  source head        {}", minted.source_head);
    println!("  schema version     {}", minted.schema_version);
    println!("  mission            {}", minted.mission_id);
    println!("  media type         {}", minted.media_type);
    println!("  snapshot           {}", minted.snapshot_path.display());
    println!("  snapshot sha256    {}", minted.snapshot_sha256);
    for p in &minted.publications {
        println!(
            "  version {} ({}) {} sha256 {} digest {}",
            p.version,
            if p.is_current {
                "current"
            } else {
                "superseded"
            },
            p.markdown_file,
            p.markdown_sha256,
            p.candidate_artifact_digest
        );
    }
    println!("  manifest           {}", minted.manifest_path.display());
}

// ---------------------------------------------------------------------------------------------
// B31: the same two-version fixture, large enough to cross the old response allowance
// ---------------------------------------------------------------------------------------------
//
// PR148's command-specific response limit has an owner/client test but no packaged large-document
// copy. This mints the discriminating input for that: a two-version fixture whose exported Markdown,
// serialized the way the wire actually carries it, exceeds the old allowance on its own. B26's
// package `0fce9e6` still carries the old client limit and can serve as the negative control; a
// later UI+PR148 package is the positive one. **Neither package is built here.**
//
// Only the completion's unfinished work differs from the ordinary two-version fixture. Same guards,
// same snapshot, same manifest schema, same readback.

/// The old command-specific response allowance, as a plain number rather than an imported constant.
///
/// Deliberately NOT `MAX_MESSAGE_BYTES`: a bar that reads the same constant the code reads moves
/// whenever the code moves and can never fail. This is the figure the fixture must beat, written out.
const B31_OLD_RESPONSE_ALLOWANCE: usize = 64 * 1024;

/// The positive bar: both versions are large, different, and each is reachable at its own number.
#[test]
fn a_large_two_version_fixture_exceeds_the_old_response_allowance_in_both_versions() {
    let prefix = PrivatePrefix::new("b31-large");
    let output = PrivateOutput::under(&prefix, "large-two-version");
    let minted = mint_two_version_fixture_with_scale(
        &prefix.path,
        Some(&output.path),
        FixtureScale::LargeUnfinishedWork,
    )
    .expect("B31.MINT.SUCCEEDS");

    let v1 = minted.at(1).clone();
    let v2 = minted.at(2).clone();

    // The size precondition, measured through serde_json's ACTUAL serialization of the `number[]`
    // the wire carries -- not hand arithmetic, and a lower bound on the whole response, since the
    // envelope, digest, version fields and media type are all additional.
    for p in [&v1, &v2] {
        let payload = b12_payload_json_len(&p.markdown_bytes);
        println!(
            "B31.VERSION {} stored_bytes = {} payload_json_len = {}",
            p.version,
            p.markdown_bytes.len(),
            payload
        );
        assert!(
            payload > B31_OLD_RESPONSE_ALLOWANCE,
            "B31.PAYLOAD_EXCEEDS_OLD_RESPONSE_ALLOWANCE: version {} serialized to {} bytes, \
             which does not exceed {}",
            p.version,
            payload,
            B31_OLD_RESPONSE_ALLOWANCE
        );
    }

    // Still two genuinely different versions, so the selection test stays discriminating.
    assert_ne!(
        v1.markdown_bytes, v2.markdown_bytes,
        "B31.MINT.BYTES_DIFFER"
    );
    assert_ne!(
        v1.candidate_artifact_digest, v2.candidate_artifact_digest,
        "B31.MINT.DIGESTS_DIFFER"
    );
    assert!(!v1.is_current && v2.is_current, "B31.MINT.V2_IS_CURRENT");

    // The owner-recorded content really is what makes them large.
    for p in [&v1, &v2] {
        let text = String::from_utf8(p.markdown_bytes.clone()).expect("B31.MINT.MARKDOWN_IS_UTF8");
        assert!(
            text.contains("Deferred scope item 0:") && text.contains("Deferred scope item 31:"),
            "B31.MINT.DOCUMENT_CARRIES_THE_OWNER_RECORDED_UNFINISHED_WORK"
        );
        assert!(
            text.contains(FIXTURE_MISSION_DECLARATION),
            "B31.MINT.DOCUMENT_CARRIES_THE_SEEDED_DECLARATION"
        );
    }

    // Modes and every retained hash, before anything reopens the store.
    assert_eq!(
        mode_of(&minted.directory),
        0o700,
        "B31.MINT.DIRECTORY_IS_0700"
    );
    for path in [
        &minted.snapshot_path,
        &minted.manifest_path,
        &v1.markdown_path,
        &v2.markdown_path,
    ] {
        assert_eq!(mode_of(path), 0o600, "B31.MINT.FILE_IS_0600");
    }
    for p in [&v1, &v2] {
        let on_disk = fs::read(&p.markdown_path).expect("B31.MINT.MARKDOWN_READS");
        assert_eq!(
            on_disk, p.markdown_bytes,
            "B31.MINT.MARKDOWN_FILE_IS_THE_EXPORT"
        );
        assert_eq!(
            sha256_hex_of(&on_disk),
            p.markdown_sha256,
            "B31.MINT.MARKDOWN_SHA"
        );
    }
    let snapshot_before =
        sha256_hex_of(&fs::read(&minted.snapshot_path).expect("B31.MINT.SNAPSHOT_READS"));
    assert_eq!(
        snapshot_before, minted.snapshot_sha256,
        "B31.MINT.SNAPSHOT_SHA"
    );

    // The B28 readback assertions, reused: a verified COPY is opened, never the retained original.
    let read_back = read_back_two_version_fixture(&minted);
    assert!(
        !read_back.workspace_created,
        "B31.READBACK.WORKSPACE_WAS_NOT_CREATED_FRESH"
    );
    assert_eq!(read_back.workspace_id, minted.workspace_id);
    assert_eq!(
        read_back.history,
        vec![
            (2, true, v2.candidate_artifact_digest.clone()),
            (1, false, v1.candidate_artifact_digest.clone()),
        ],
        "B31.READBACK.HISTORY_IS_2_NEWEST_1_SUPERSEDED"
    );
    let (v1_version, v1_count, v1_bytes, v1_digest) = read_back.exports[0].clone();
    let (v2_version, v2_count, v2_bytes, v2_digest) = read_back.exports[1].clone();
    assert_eq!(
        (v1_version, v1_count),
        (1, 2),
        "B31.READBACK.V1_REPORTS_1_OF_2"
    );
    assert_eq!(
        (v2_version, v2_count),
        (2, 2),
        "B31.READBACK.V2_REPORTS_2_OF_2"
    );
    assert_eq!(
        v1_bytes, v1.markdown_bytes,
        "B31.READBACK.V1_EXPORTS_ITS_OWN_BYTES"
    );
    assert_eq!(
        v2_bytes, v2.markdown_bytes,
        "B31.READBACK.V2_EXPORTS_ITS_OWN_BYTES"
    );
    assert_eq!(
        v1_digest, v1.candidate_artifact_digest,
        "B31.READBACK.V1_DIGEST"
    );
    assert_eq!(
        v2_digest, v2.candidate_artifact_digest,
        "B31.READBACK.V2_DIGEST"
    );

    // And the retained artifact is untouched by its own proof.
    assert_eq!(
        sha256_hex_of(&fs::read(&minted.snapshot_path).expect("B31.READBACK.SNAPSHOT_STILL_READS")),
        minted.snapshot_sha256,
        "B31.READBACK.RETAINED_SNAPSHOT_SURVIVES_ITS_OWN_PROOF"
    );
    for p in [&v1, &v2] {
        assert_eq!(
            sha256_hex_of(&fs::read(&p.markdown_path).expect("B31.READBACK.MARKDOWN_STILL_READS")),
            p.markdown_sha256,
            "B31.READBACK.RETAINED_MARKDOWN_SURVIVES_ITS_OWN_PROOF"
        );
    }
}

/// Mints the LARGE two-version fixture into a directory you name. Not part of the suite.
///
///     export TMPDIR="$(mktemp -d)"    # 0700, yours, under /tmp -- the prefix the guard requires
///     BEACON_B31_FIXTURE_DIR="$(mktemp -u "$TMPDIR/beacon-b31-XXXXXX")" \
///         cargo test -p beacond --test outcome_prepare -- --ignored --exact \
///         mint_large_two_version_outcome_fixture_into_the_named_directory --nocapture
///
/// Its own env variable, so neither the one-version nor the ordinary two-version manual test
/// changes meaning. It writes the SAME two-version manifest schema -- only the content is larger --
/// so a consumer reads it exactly as it reads the ordinary one.
#[test]
#[ignore = "mints a large two-version private fixture into a directory you name; run deliberately, never as part of the suite"]
fn mint_large_two_version_outcome_fixture_into_the_named_directory() {
    let named = std::env::var_os("BEACON_B31_FIXTURE_DIR").map(PathBuf::from);
    let prefix = std::env::temp_dir();
    let minted = mint_two_version_fixture_with_scale(
        &prefix,
        named.as_deref(),
        FixtureScale::LargeUnfinishedWork,
    )
    .unwrap_or_else(|refusal| panic!("{refusal}"));

    for p in &minted.publications {
        assert!(
            b12_payload_json_len(&p.markdown_bytes) > B31_OLD_RESPONSE_ALLOWANCE,
            "B31.PAYLOAD_EXCEEDS_OLD_RESPONSE_ALLOWANCE"
        );
    }
    let read_back = read_back_two_version_fixture(&minted);
    assert_eq!(read_back.exports[0].2, minted.at(1).markdown_bytes);
    assert_eq!(read_back.exports[1].2, minted.at(2).markdown_bytes);

    println!(
        "B31 large two-version fixture minted at {}",
        minted.directory.display()
    );
    println!("  source head        {}", minted.source_head);
    println!("  schema version     {}", minted.schema_version);
    println!("  mission            {}", minted.mission_id);
    println!("  snapshot           {}", minted.snapshot_path.display());
    println!("  snapshot sha256    {}", minted.snapshot_sha256);
    println!("  manifest           {}", minted.manifest_path.display());
    for p in &minted.publications {
        println!(
            "  version {} ({}) {} markdown_bytes {} payload_json_len {} sha256 {} digest {}",
            p.version,
            if p.is_current {
                "current"
            } else {
                "superseded"
            },
            p.markdown_file,
            p.markdown_bytes.len(),
            b12_payload_json_len(&p.markdown_bytes),
            p.markdown_sha256,
            p.candidate_artifact_digest
        );
    }
}
