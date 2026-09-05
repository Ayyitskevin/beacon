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
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use beacond::{
    AccessOrigin, AppendixMode, AppendixRequest, ArtifactContext, ArtifactDigest, ArtifactKind,
    ArtifactService, AutonomyProfile, BeaconCapability, COMPLETION_CANONICALIZER,
    CheckPreflightService, ClassificationActor, ClassificationBasis, ClassificationStoreError,
    ClassificationSupersedes, CollectionRequest, Collector, CompletionActor, CompletionDisposition,
    CompletionReadRefusal, CompletionSupersedes, ConditionOutcome, ConditionResult,
    ConditionResultRecord, Conflicts, DeclaredCheck, DiagnosticStatus, Evaluation,
    EvaluationDigest, EvaluationDraft, EvaluationError, EvaluationService, EvaluationSubjectDraft,
    EvaluationSupersedes, Evaluator, Finding, FindingSeverity, ImpactLevel, InconsistentEvaluation,
    LocalErrorCode, LocalModelActivationService, LocalModelEndpoint, LocalServerConfig,
    LocalSupervisorClient, LocalSupervisorServer, LocalTransportError,
    MISSION_IMPACT_CANONICALIZER, ManifestStoreRefusal, MissionCompletionV1, MissionDraft,
    MissionError, MissionImpactClassificationV1, MissionImpactService, MissionMemberDraft,
    MissionService, NarrativeItem, OperatorAccountError, OperatorAccountService,
    OperatorSessionPhase, OutcomeCandidate, OutcomeManifestV1, OutcomeService, OutcomeState,
    PolicyRevisionRef, PredecessorFault, PrepareRefusal, ProjectCharterDraft, ProjectDraft,
    ProjectError, ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService,
    ProjectService, Provenance, ProviderFeature, PublicationSeamRefusal, PublishRefusal,
    RepositoryDraft, RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService,
    RubricDeclaration, SecretString, SourceDigest, SourceKind, SourceRef, SourcesRefusal,
    SpendReference, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    SuccessCondition, Supervisor, SupervisorError, SupportRef, TeamDraft, TeamLeadAmbiguityRule,
    TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft,
    TeamService, TrackRecordEvidenceKind, TrackRecordRequirement, UnfinishedKind, Verdict,
    VerifiedSources, VerifiedText, declared_conditions_digest, initialize_local_auth_token,
    publication_candidacy, record_completion, render, verified_manifest_within,
};
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

    let admitted = admit_mission_in(&accounts, workspace_id);
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
        .map(|resident| {
            ResidentConfigurationService::new(Arc::clone(&accounts), workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    vec![
                        BeaconCapability::WorkCreateMission,
                        BeaconCapability::WorkCoordinateMission,
                    ],
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
    connection.execute("INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms) VALUES (?1, 'workspace.created', ?2, 1)",
        params![elsewhere.to_string(), serde_json::json!({"workspace_id": elsewhere, "name": "Elsewhere"}).to_string()]).unwrap();
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
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, payload_json, occurred_at_ms + 1 FROM events
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
    let (fixture, manifest, sources) = ordinary("r28-manifest-startup");
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
    let ordered: Vec<String> = connection
        .prepare(
            "SELECT manifest_digest FROM outcome_manifests ORDER BY workspace_id, manifest_digest",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        ordered.len(),
        2,
        "the fixture must hold two manifests: {ordered:?}"
    );
    let target = ordered.last().unwrap().clone();

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
    let fixture = admitted_mission("r28-mission-startup");
    let other = sol1_admit_other_mission(&fixture, "A second admitted Mission");
    assert_ne!(other, fixture.mission_id);

    let connection = fixture.store.connect();
    let ordered: Vec<String> = connection
        .prepare("SELECT id FROM missions ORDER BY workspace_id, id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        ordered.len(),
        2,
        "the fixture must hold two Missions: {ordered:?}"
    );
    let target = Uuid::parse_str(ordered.last().unwrap()).unwrap();

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

/// R28.1 pass 4 of 5: a Project whose charter history disagrees with its current revision is
/// refused at startup, and the refusal names WHICH Project.
///
/// R39.3's two-row shape, deliberate: a second Project is created and the tamper lands on
/// whichever sorts LAST in the pass's own `ORDER BY`.
#[test]
fn startup_refuses_a_project_whose_charter_history_disagrees_and_names_the_row() {
    let fixture = admitted_mission("r28-project-startup");
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
    let ordered: Vec<String> = connection
        .prepare("SELECT id FROM projects ORDER BY workspace_id, id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        ordered.len(),
        2,
        "the fixture must hold two Projects: {ordered:?}"
    );
    let target = Uuid::parse_str(ordered.last().unwrap()).unwrap();

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

    let refusal =
        astra_boot(&fixture).expect_err("a store whose charter history disagrees reopened Ready");
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

/// R28.1 pass 5 of 5: a completion whose projection disagrees with its own bytes is refused at
/// startup, and the refusal names WHICH completion and carries the reader's own fault.
///
/// R39.3's two-row shape, deliberate: a superseding completion is recorded, the pass's own
/// `ORDER BY digest_value` is read back, and the tamper lands on whichever sorts LAST.
#[test]
fn startup_refuses_a_completion_whose_column_disagrees_and_names_the_row() {
    let fixture = admitted_mission("r28-completion-startup");
    fixture.classify(ImpactLevel::Ordinary);
    let (first_digest, first) = fixture.complete((fixture.resident_id, fixture.configuration_id));

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
    let ordered: Vec<String> = connection
        .prepare("SELECT digest_value FROM mission_completions ORDER BY digest_value")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        ordered.len(),
        2,
        "the fixture must hold two completions: {ordered:?}"
    );
    let target = ordered.last().unwrap().clone();

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
    let read = beacond::verified_completion_within(&read_connection, fixture.workspace_id, &target)
        .expect_err("the reader trusted a projection its bytes do not support");
    assert!(
        matches!(
            read,
            CompletionReadRefusal::ColumnDisagrees("completed_at_ms")
        ),
        "the planted fault is not the one this test is about: {read:?}"
    );
    drop(read_connection);

    let refusal = astra_boot(&fixture)
        .expect_err("a store holding a completion that disagrees with its bytes reopened Ready");
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

/// R28.1 pass 1 of 5: a publication that does not re-prove is refused at startup, and the refusal
/// names WHICH publication and carries the reader's own fault.
///
/// Mirrors `sol1_get_refuses_a_valid_evaluation_swapped_into_the_relation` for the plant (the
/// relation's `evaluation_digest` swapped for a valid Evaluation of a different candidate, with
/// the immutability trigger dropped and recreated around the write) and
/// `an_unreadable_classification_is_refused_at_startup_with_the_readers_fault` for the reopen.
#[test]
fn startup_refuses_a_swapped_publication_evaluation_and_names_the_row() {
    let fixture = admitted_mission("r28-publication-startup-swap");
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
            "UPDATE outcome_publications SET evaluation_digest = ?1
             WHERE workspace_id = ?2 AND publication_version = 2",
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

    // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
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
                && *version == 2
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
            && line.contains("version 2"),
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
    let fixture = admitted_mission("classification-pass-forged-predecessor");

    // Version 1, planted, citing a predecessor nothing stored.
    let mut forged = MissionImpactClassificationV1 {
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
    let second = fixture.classify_superseding(2, ClassificationSupersedes::Record(planted.clone()));
    fixture.classify_superseding(3, ClassificationSupersedes::Record(second));

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
        fixture.plant_classification(&sol1_classification_record(
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
    let first = fixture.plant_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        1,
        ClassificationSupersedes::None,
        "The common root",
    ));
    fixture.plant_classification(&sol1_classification_record(
        &fixture,
        fixture.mission_id,
        2,
        ClassificationSupersedes::Record(first.clone()),
        "The first child",
    ));
    fixture.plant_classification(&sol1_classification_record(
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
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, ?2, '{}', 9000)",
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
    assert!(matches!(refusal, PublishRefusal::Absent), "{refusal:?}");

    // The SAME answer a Mission with no publication gives, which is the point: the two are not
    // distinguishable, so the refusal confirms nothing.
    let unpublished = admitted_mission("export-cross-workspace-unpublished");
    let never = fixture_outcomes(&unpublished)
        .export_markdown(unpublished.mission_id)
        .expect_err("an unpublished Mission exported");
    assert!(matches!(never, PublishRefusal::Absent), "{never:?}");
    assert_eq!(format!("{refusal}"), format!("{never}"));

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
    assert!(matches!(refusal, PublishRefusal::Absent), "{refusal:?}");

    // Indistinguishable from a Mission that simply has no publication, which is why the refusal
    // confirms nothing about what the other Workspace holds (R29.6).
    let unpublished = admitted_mission("pdf-service-cross-workspace-unpublished");
    assert!(matches!(
        unpublished
            .outcomes
            .render_pdf_version(unpublished.mission_id, 1)
            .expect_err("an unpublished Mission rendered"),
        PublishRefusal::Absent
    ));
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
