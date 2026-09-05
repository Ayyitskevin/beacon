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
    CompletionSupersedes, ConditionOutcome, ConditionResult, ConditionResultRecord, Conflicts,
    DeclaredCheck, DiagnosticStatus, Evaluation, EvaluationDigest, EvaluationDraft,
    EvaluationError, EvaluationService, EvaluationSubject, EvaluationSubjectDraft,
    EvaluationSupersedes, Evaluator, Finding, FindingSeverity, ImpactLevel, InconsistentEvaluation,
    LocalErrorCode, LocalModelActivationService, LocalModelEndpoint, LocalServerConfig,
    LocalSupervisorClient, LocalSupervisorServer, LocalTransportError,
    MISSION_IMPACT_CANONICALIZER, ManifestStoreRefusal, MeasuredSpend, MeasuredSpendService,
    MeasuredSpendV1, MissionCompletionV1, MissionDraft, MissionImpactClassification,
    MissionImpactClassificationV1, MissionImpactService, MissionMemberDraft, MissionService,
    NarrativeItem, OperatorAccountError, OperatorAccountService, OperatorSessionPhase,
    OutcomeCandidate, OutcomeManifestV1, OutcomeService, OutcomeState, PolicyRevisionRef,
    PredecessorFault, PrepareRefusal, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProjectPolicyService, ProjectRepositoryService, ProjectService, ProposalService, Provenance,
    ProviderFeature, PublicationSeamRefusal, PublishRefusal, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationService, ResidentLifecycleService, RubricDeclaration, RulingDecision,
    RulingService, RulingSupersedes, SecretString, SourceDigest, SourceKind, SourceRef,
    SourcesRefusal, SpendCompleteness, SpendLine, SpendMeasurement, SpendMeasurementKind,
    SpendOrigin, SpendReference, SpendSupersedes, SpendTotals, StarterOrganizationService,
    StarterResidentDraft, StarterRole, StartupConfig, SuccessCondition, Supervisor,
    SupervisorError, SupportRef, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement, UncertaintyKind, UnfinishedKind, Verdict,
    VerifiedSources, VerifiedText, declared_conditions_digest, initialize_local_auth_token,
    publication_candidacy, record_completion, render, verified_completion_within,
    verified_manifest_within,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CONDITION_TEXTS: [&str; 4] = [
    "The implementation path works",
    "Every declared check passes",
    "The optional follow-up was assessed",
    "The ambiguous evidence was resolved",
];

#[derive(Clone)]
struct TestStore {
    path: PathBuf,
    guard: Arc<()>,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-outcome-prepare-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
            guard: Arc::new(()),
        }
    }

    fn connect(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        if Arc::strong_count(&self.guard) != 1 {
            return;
        }
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
            success_conditions: CONDITION_TEXTS
                .iter()
                .map(|text| (*text).to_owned())
                .collect(),
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

fn latest_event_seq(store: &TestStore, workspace_id: Uuid) -> u64 {
    store
        .connect()
        .query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).expect("C11.EVENT_SEQUENCE_DOMAIN"))
        .expect("C11.EVENT_SEQUENCE_READ")
}

fn event_count(store: &TestStore, workspace_id: Uuid) -> u64 {
    store
        .connect()
        .query_row(
            "SELECT count(*) FROM events WHERE workspace_id = ?1",
            params![workspace_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| u64::try_from(count).expect("C11.EVENT_COUNT_DOMAIN"))
        .expect("C11.EVENT_COUNT_READ")
}

fn publication_count(store: &TestStore, workspace_id: Uuid, mission_id: Uuid) -> u64 {
    store
        .connect()
        .query_row(
            "SELECT count(*) FROM outcome_publications
             WHERE workspace_id = ?1 AND mission_id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| u64::try_from(count).expect("C11.PUBLICATION_COUNT_DOMAIN"))
        .expect("C11.PUBLICATION_COUNT_READ")
}

fn digest_bytes(digest: &str) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    data_encoding::HEXLOWER
        .decode_mut(digest.as_bytes(), &mut bytes)
        .expect("C11.DIGEST_IS_CANONICAL_HEX");
    bytes
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

fn bind_repository(fixture: &Fixture) {
    ProjectRepositoryService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .bind(
            fixture.project_id,
            fixture.charter_revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: "/home/operator/projects/c11-missing-repository".to_owned(),
                base_revision: "0".repeat(40),
                declared_checks: vec![DeclaredCheck {
                    name: "cargo-test".to_owned(),
                    argv: vec!["cargo".to_owned(), "test".to_owned()],
                    working_directory: ".".to_owned(),
                }],
            },
        )
        .expect("C11.03.REPOSITORY_BOUND");
}

const PROPOSAL_MAGIC: &[u8] = b"beacon.proposal.v1\0";

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn push_count(out: &mut Vec<u8>, count: usize) {
    out.extend_from_slice(&(count as u32).to_be_bytes());
}

fn proposal_bytes(fixture: &Fixture) -> Vec<u8> {
    let context_entries = [("src/lib.rs", [0x66_u8; 32])];
    let mut encoded_entries = Vec::new();
    push_count(&mut encoded_entries, context_entries.len());
    for (path, digest) in context_entries {
        push_string(&mut encoded_entries, path);
        encoded_entries.extend_from_slice(&digest);
    }

    let mut out = PROPOSAL_MAGIC.to_vec();
    for id in [
        fixture.workspace_id,
        fixture.project_id,
        fixture.mission_id,
        fixture.resident_id,
        fixture.configuration_id,
    ] {
        out.extend_from_slice(id.as_bytes());
    }
    out.push(0x01);
    push_string(
        &mut out,
        "The Owner selected this Resident to request this one Proposal.",
    );
    for revision in [fixture.charter_revision, fixture.policy_revision, 1] {
        out.extend_from_slice(&revision.to_be_bytes());
    }
    push_string(&mut out, &"0".repeat(40));
    push_string(&mut out, "Add the criterion-11 closure regression test.");
    out.push(0x02);
    out.extend_from_slice(&encoded_entries);
    out.extend_from_slice(&sha256_bytes(&encoded_entries));

    push_count(&mut out, 1);
    push_string(&mut out, "src/lib.rs");
    push_count(&mut out, 1);
    out.push(0x02);
    push_string(&mut out, "src/lib.rs");
    push_count(&mut out, 1);
    push_string(&mut out, "cargo-test");
    push_count(&mut out, 1);
    push_string(&mut out, "cargo-test");
    push_count(&mut out, 2);
    push_string(&mut out, "cargo");
    push_string(&mut out, "test");
    push_string(&mut out, ".");
    out.extend_from_slice(&[0x01; 7]);
    for bound in [300_000_u64, 240, 2 << 30, 256 << 20, 1 << 20, 0] {
        out.extend_from_slice(&bound.to_be_bytes());
    }
    out.push(0x01);
    out.push(0x01);
    out
}

fn artifacts(fixture: &Fixture) -> ArtifactService {
    ArtifactService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
}

fn record_artifact(fixture: &Fixture, content: &[u8]) -> beacond::VerifiedArtifactRecord {
    artifacts(fixture)
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
        .expect("C11.03.ARTIFACT_RECORDED")
}

fn evaluate_artifact(fixture: &Fixture, artifact_digest: &str) -> Evaluation {
    EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .record(EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(artifact_digest.to_owned()),
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
                summary: "Read the exact Artifact bytes.".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "An independent Resident read the exact Artifact bytes.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        })
        .expect("C11.03.EVALUATION_RECORDED")
}

fn classify_mission(
    fixture: &Fixture,
    mission_id: Uuid,
    version: u32,
    impact: ImpactLevel,
    supersedes: ClassificationSupersedes,
) -> MissionImpactClassification {
    let payload = MissionImpactClassificationV1 {
        workspace_id: fixture.workspace_id,
        mission_id,
        classification_id: Uuid::new_v4(),
        version,
        impact,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project_id,
            policy_revision: fixture.policy_revision,
            charter_revision: fixture.charter_revision,
        },
        rule_id: None,
        reason: "The Operator classified this Mission for its Outcome appendix.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000 + u64::from(version),
        supersedes,
    };
    let bytes = payload
        .canonical_bytes()
        .expect("C11.03.CLASSIFICATION_CANONICAL");
    let service = MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = service
        .classify(&bytes)
        .expect("C11.03.CLASSIFICATION_RECORDED");
    assert_eq!(
        recorded.canonical_bytes(),
        bytes,
        "C11.03.CLASSIFICATION_OWNER_BYTES"
    );
    assert_eq!(
        recorded.payload(),
        &payload,
        "C11.03.CLASSIFICATION_OWNER_BYTES"
    );
    recorded
}

fn record_spend(
    fixture: &Fixture,
    mission_id: Uuid,
    coverage_to_event_seq: u64,
    measurement: SpendMeasurement,
) -> (MeasuredSpendV1, MeasuredSpend) {
    let payload = MeasuredSpendV1 {
        workspace_id: fixture.workspace_id,
        mission_id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: 1,
        coverage_to_event_seq,
        measurement,
        supersedes: SpendSupersedes::None,
    };
    let bytes = payload.canonical_bytes().expect("C11.03.SPEND_CANONICAL");
    let service = MeasuredSpendService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = service.record(&bytes).expect("C11.03.SPEND_RECORDED");
    assert_eq!(
        recorded.canonical_bytes(),
        bytes,
        "C11.03.SPEND_OWNER_BYTES"
    );
    assert_eq!(recorded.payload(), &payload, "C11.03.SPEND_OWNER_BYTES");
    (payload, recorded)
}

fn measured_totals() -> SpendMeasurement {
    SpendMeasurement::Measured(SpendTotals {
        counted_units: 1,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 1234,
            minor_units_exponent: 2,
        }],
    })
}

fn support_in_canonical_order(
    delivered: &beacond::VerifiedArtifactRecord,
    supporting: &beacond::VerifiedArtifactRecord,
    evaluation: &Evaluation,
    ruling: &beacond::Ruling,
) -> Vec<SupportRef> {
    let mut artifact_digests = [
        digest_bytes(&delivered.digest().value),
        digest_bytes(&supporting.digest().value),
    ];
    artifact_digests.sort();
    vec![
        SupportRef::Artifact(artifact_digests[0]),
        SupportRef::Artifact(artifact_digests[1]),
        SupportRef::Evaluation(digest_bytes(&evaluation.digest().value)),
        SupportRef::Ruling(digest_bytes(&ruling.digest().value)),
    ]
}

fn completion_record(
    fixture: &Fixture,
    mission_id: Uuid,
    boundary_event_seq: u64,
    first_support: Vec<SupportRef>,
    second_support: Vec<SupportRef>,
    delivered_artifacts: Vec<[u8; 32]>,
    spend: SpendReference,
    narratives: bool,
) -> MissionCompletionV1 {
    MissionCompletionV1 {
        workspace_id: fixture.workspace_id,
        project_id: fixture.project_id,
        mission_id,
        completion_id: Uuid::new_v4(),
        actor: CompletionActor::Operator,
        accountable_resident_id: fixture.resident_id,
        accountable_resident_configuration_id: fixture.configuration_id,
        boundary_event_seq,
        disposition: CompletionDisposition::Concluded,
        declared_conditions_sha256: declared_conditions_digest(
            &CONDITION_TEXTS
                .iter()
                .map(|text| (*text).to_owned())
                .collect::<Vec<_>>(),
        ),
        condition_results: vec![
            ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::Met,
                support: first_support,
                note: None,
            },
            ConditionResultRecord {
                ordinal: 1,
                outcome: ConditionOutcome::NotMet,
                support: second_support,
                note: Some("The declared check still needs one correction.".to_owned()),
            },
            ConditionResultRecord {
                ordinal: 2,
                outcome: ConditionOutcome::NotAssessed,
                support: vec![],
                note: None,
            },
            ConditionResultRecord {
                ordinal: 3,
                outcome: ConditionOutcome::AssessmentInconclusive,
                support: vec![],
                note: Some("The available evidence did not settle the condition.".to_owned()),
            },
        ],
        delivered_artifacts,
        unfinished_work: narratives
            .then(|| {
                vec![NarrativeItem {
                    ordinal: 0,
                    kind: UnfinishedKind::ConditionNotMet,
                    condition_ordinal: Some(1),
                    summary: "Correct the remaining declared-check failure.".to_owned(),
                }]
            })
            .unwrap_or_default(),
        uncertainty: narratives
            .then(|| {
                vec![NarrativeItem {
                    ordinal: 0,
                    kind: UncertaintyKind::ConflictingEvidence,
                    condition_ordinal: Some(3),
                    summary: "Two verified observations still disagree.".to_owned(),
                }]
            })
            .unwrap_or_default(),
        spend,
        supersedes: CompletionSupersedes::None,
        summary: "Criterion 11 records the Mission without inventing success.".to_owned(),
        completed_at_ms: 1_788_400_000_000,
    }
}

fn record_completion_for(fixture: &Fixture, record: &MissionCompletionV1) -> String {
    let bytes = record
        .canonical_bytes()
        .expect("C11.04.COMPLETION_CANONICAL");
    let digest = record_completion(&mut fixture.store.connect(), record)
        .expect("C11.04.DELIVERED_ARTIFACT_ACCEPTED");
    let reread =
        verified_completion_within(&fixture.store.connect(), fixture.workspace_id, &digest)
            .expect("C11.04.COMPLETION_OWNER_READ");
    assert_eq!(reread, *record, "C11.04.COMPLETION_OWNER_BYTES");
    assert_eq!(
        reread
            .canonical_bytes()
            .expect("C11.04.COMPLETION_REENCODE"),
        bytes,
        "C11.04.COMPLETION_OWNER_BYTES"
    );
    digest
}

fn admit_companion_mission(fixture: &Fixture, desired_outcome: &str) -> Uuid {
    let team_id = fixture
        .store
        .connect()
        .query_row(
            "SELECT team_id FROM missions WHERE workspace_id = ?1 AND id = ?2",
            params![
                fixture.workspace_id.to_string(),
                fixture.mission_id.to_string()
            ],
            |row| row.get::<_, String>(0),
        )
        .map(|id| Uuid::parse_str(&id).expect("C11.COMPANION_TEAM_ID"))
        .expect("C11.COMPANION_TEAM_READ");
    MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .admit(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: fixture.charter_revision,
            team_id,
            desired_outcome: desired_outcome.to_owned(),
            success_conditions: CONDITION_TEXTS
                .iter()
                .map(|text| (*text).to_owned())
                .collect(),
            constraints: vec!["Keep the change reversible".to_owned()],
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
        .expect("C11.COMPANION_MISSION_ADMITTED")
        .id
}

fn run_preflight(fixture: &Fixture) -> (Uuid, u64) {
    let receipt = CheckPreflightService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .preflight(fixture.project_id)
        .expect("C11.03.REAL_TERMINAL_RUN_RECEIPT");
    let (sequence, payload): (i64, String) = fixture
        .store
        .connect()
        .query_row(
            "SELECT sequence, payload_json FROM events
             WHERE workspace_id = ?1 AND kind = 'run.preflighted'
             ORDER BY sequence DESC LIMIT 1",
            params![fixture.workspace_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("C11.03.REAL_RUN_EVENT");
    assert!(
        payload.contains(&receipt.run_id.to_string()),
        "C11.03.REAL_RUN_EVENT"
    );
    (
        receipt.run_id,
        u64::try_from(sequence).expect("C11.03.REAL_RUN_EVENT"),
    )
}

fn proposal_and_ruling(fixture: &Fixture) -> (beacond::Proposal, beacond::Ruling) {
    let proposal = ProposalService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .submit(&proposal_bytes(fixture))
        .expect("C11.03.PROPOSAL_RECORDED");
    let ruling = RulingService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .rule(
            &proposal.digest().value,
            RulingDecision::Approve,
            "The test-only change is scoped, reversible, and runs no deployment.",
            RulingSupersedes::None,
        )
        .expect("C11.03.RULING_RECORDED");
    (proposal, ruling)
}

fn table_count(store: &TestStore, table: &str, workspace_id: Uuid) -> u64 {
    store
        .connect()
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE workspace_id = ?1"),
            params![workspace_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| u64::try_from(count).expect("C11.TABLE_COUNT_DOMAIN"))
        .expect("C11.TABLE_COUNT_READ")
}

/// Deliberate non-assertions: this closure does not duplicate the codec, migration, trigger,
/// concurrency, UI, accessibility, PDF-layout, source-tail, withdrawal, sharing, redaction, or
/// elapsed-state matrices owned by their dedicated suites. It verifies records and byte identity
/// along one real completed-Mission path; it does not make a Meets verdict true, grant authority,
/// execute a model, or claim a published Outcome completed or succeeded at its Mission.
#[test]
fn criterion_11_outcome_closure_on_one_store() {
    let fixture = admitted_mission("criterion-11-closure");
    let saved_store_path = fixture.store.path.clone();
    assert!(saved_store_path.exists(), "C11.01.ONE_STORE");
    assert_eq!(
        fixture
            .accounts
            .status()
            .expect("C11.01.ACCOUNT_STATUS")
            .phase,
        OperatorSessionPhase::Unlocked,
        "C11.01.OPERATOR_UNLOCKED"
    );
    let schema_version: u32 = fixture
        .store
        .connect()
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .expect("C11.01.BOOTSTRAP_READY");
    assert!(schema_version >= 32, "C11.01.BOOTSTRAP_READY");

    let mission_service = MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let mission = mission_service
        .get(fixture.mission_id)
        .expect("C11.02.MISSION_READ");
    assert_eq!(
        mission.workspace_id, fixture.workspace_id,
        "C11.01.ONE_STORE"
    );
    assert_eq!(
        mission.project_id, fixture.project_id,
        "C11.02.DECLARATION_EXACT"
    );
    assert_eq!(
        mission.success_conditions,
        CONDITION_TEXTS
            .iter()
            .map(|text| (*text).to_owned())
            .collect::<Vec<_>>(),
        "C11.02.DECLARATION_EXACT"
    );
    assert!(
        matches!(
            fixture.outcomes.get(fixture.mission_id),
            Err(PublishRefusal::Absent)
        ),
        "C11.02.NO_PUBLICATION_YET"
    );
    assert!(
        matches!(
            fixture.outcomes.prepare_for_mission(
                fixture.mission_id,
                AppendixRequest::PolicyDefault
            ),
            Err(PrepareRefusal::MissionNotCompleted { mission_id })
                if mission_id == fixture.mission_id
        ),
        "C11.02.NO_COMPLETION_IS_NAMED"
    );

    // Step 3: all support records enter through their owners before the immutable cut exists.
    bind_repository(&fixture);
    let delivered = record_artifact(&fixture, b"# Delivered Artifact D\n");
    let supporting = record_artifact(&fixture, b"# Support-only Artifact S\n");
    let decoy = record_artifact(&fixture, b"# Uncited decoy Artifact\n");
    let support_evaluation = evaluate_artifact(&fixture, &supporting.digest().value);
    let decoy_evaluation = evaluate_artifact(&fixture, &decoy.digest().value);
    let (_proposal, ruling) = proposal_and_ruling(&fixture);
    for expected in [&delivered, &supporting, &decoy] {
        let reread = artifacts(&fixture)
            .get(&expected.digest().value)
            .expect("C11.03.SUPPORT_OWNER_BYTES");
        assert_eq!(reread, *expected, "C11.03.SUPPORT_OWNER_BYTES");
    }
    let evaluation_reread =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .get(&support_evaluation.digest().value)
            .expect("C11.03.SUPPORT_OWNER_BYTES");
    assert_eq!(
        evaluation_reread, support_evaluation,
        "C11.03.SUPPORT_OWNER_BYTES"
    );
    assert_ne!(
        support_evaluation.payload().evaluator.resident_id,
        fixture.resident_id,
        "C11.03.NONAUTHOR_RESIDENT"
    );
    assert_ne!(
        decoy_evaluation.digest(),
        support_evaluation.digest(),
        "C11.03.UNCITED_DECOY_DISTINCT"
    );
    let ruling_reread = RulingService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .get(&ruling.digest().value)
        .expect("C11.03.SUPPORT_OWNER_BYTES");
    assert_eq!(ruling_reread, ruling, "C11.03.SUPPORT_OWNER_BYTES");

    // Two pre-cut classifications make both "oldest" and "newest overall" wrong answers visible.
    let older = classify_mission(
        &fixture,
        fixture.mission_id,
        1,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::None,
    );
    let high = classify_mission(
        &fixture,
        fixture.mission_id,
        2,
        ImpactLevel::High,
        ClassificationSupersedes::Record(older.digest().value.clone()),
    );
    let (_primary_run_id, primary_run_seq) = run_preflight(&fixture);
    assert_eq!(
        primary_run_seq,
        latest_event_seq(&fixture.store, fixture.workspace_id),
        "C11.03.REAL_RUN_EVENT"
    );
    let coverage_to_primary = latest_event_seq(&fixture.store, fixture.workspace_id);
    let (primary_spend, primary_spend_record) = record_spend(
        &fixture,
        fixture.mission_id,
        coverage_to_primary,
        measured_totals(),
    );

    // Step 4: one owner-written completion freezes all four outcomes and exact cited sets.
    let boundary_primary = latest_event_seq(&fixture.store, fixture.workspace_id);
    let first_support =
        support_in_canonical_order(&delivered, &supporting, &support_evaluation, &ruling);
    let delivered_ref = digest_bytes(&delivered.digest().value);
    let primary_completion = completion_record(
        &fixture,
        fixture.mission_id,
        boundary_primary,
        first_support.clone(),
        vec![SupportRef::Artifact(delivered_ref)],
        vec![delivered_ref],
        SpendReference::Recorded {
            spend_id: primary_spend.spend_id,
            spend_version: primary_spend.version,
            spend_sha256: digest_bytes(&primary_spend_record.digest().value),
        },
        true,
    );
    assert!(
        coverage_to_primary < boundary_primary,
        "C11.04.BOUNDARY_CUT"
    );
    let primary_completion_digest = record_completion_for(&fixture, &primary_completion);
    assert!(
        latest_event_seq(&fixture.store, fixture.workspace_id) > boundary_primary,
        "C11.04.BOUNDARY_CUT"
    );
    assert_eq!(
        primary_completion.condition_results.len(),
        CONDITION_TEXTS.len(),
        "C11.04.CONDITIONS_EXACT"
    );
    assert_eq!(
        primary_completion.condition_results[0].support, first_support,
        "C11.04.SUPPORT_EXACT"
    );
    assert!(
        matches!(
            fixture.outcomes.get(fixture.mission_id),
            Err(PublishRefusal::Absent)
        ),
        "C11.04.COMPLETION_IS_NOT_PUBLICATION"
    );
    // Step 5: the cut, not "newest now", chooses both Policy and spend state.
    let _later_ordinary = classify_mission(
        &fixture,
        fixture.mission_id,
        3,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::Record(high.digest().value.clone()),
    );
    let primary_candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("C11.05.PRIMARY_PREPARES");
    let primary_stored = fixture
        .outcomes
        .manifest(&primary_candidate.manifest_digest)
        .expect("C11.05.PRIMARY_MANIFEST_READ");
    let primary_manifest = primary_stored.manifest();
    assert_eq!(
        primary_manifest.policy_classification.digest.value,
        high.digest().value,
        "C11.05.CLASSIFICATION_AT_BOUNDARY"
    );
    assert_eq!(
        primary_manifest.boundary_event_seq, boundary_primary,
        "C11.05.BOUNDARY_EXACT"
    );
    assert!(
        primary_manifest.policy_high_impact,
        "C11.05.HIGH_AT_BOUNDARY"
    );
    let primary_manifest_spend = primary_manifest
        .spend_ref
        .verified()
        .expect("C11.05.PRIMARY_SPEND_VERIFIED");
    assert_eq!(
        primary_manifest_spend.measurement,
        SpendMeasurementKind::Measured,
        "C11.05.PRIMARY_WINDOW_IS_MEASURED"
    );
    assert_eq!(
        primary_manifest_spend.amount_minor, 1234,
        "C11.05.SPEND_AMOUNT"
    );
    assert_eq!(
        primary_manifest_spend.currency, "USD",
        "C11.05.SPEND_CURRENCY"
    );
    assert_eq!(
        primary_manifest_spend.minor_units_exponent, 2,
        "C11.05.SPEND_EXPONENT"
    );
    assert_eq!(
        (
            primary_manifest_spend.coverage_from_event_seq,
            primary_manifest_spend.coverage_to_event_seq,
        ),
        (1, coverage_to_primary),
        "C11.05.SPEND_COVERAGE"
    );

    // The paired Mission has a real run inside (coverage_to, boundary]. It proves Partial is not
    // inferred from arithmetic distance alone and that the first uncovered event remains named.
    let gap_mission_id = admit_companion_mission(&fixture, "Prove the spend-window gap is named");
    let _gap_classification = classify_mission(
        &fixture,
        gap_mission_id,
        1,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::None,
    );
    let coverage_to_gap = latest_event_seq(&fixture.store, fixture.workspace_id);
    let (gap_spend, gap_spend_record) =
        record_spend(&fixture, gap_mission_id, coverage_to_gap, measured_totals());
    let (_gap_run_id, gap_run_seq) = run_preflight(&fixture);
    let boundary_gap = latest_event_seq(&fixture.store, fixture.workspace_id);
    assert_eq!(gap_run_seq, boundary_gap, "C11.05.GAP_REAL_RUN_AT_BOUNDARY");
    assert!(coverage_to_gap < gap_run_seq, "C11.05.GAP_INTERVAL_REAL");
    let gap_completion = completion_record(
        &fixture,
        gap_mission_id,
        boundary_gap,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        SpendReference::Recorded {
            spend_id: gap_spend.spend_id,
            spend_version: gap_spend.version,
            spend_sha256: digest_bytes(&gap_spend_record.digest().value),
        },
        false,
    );
    let gap_completion_digest = record_completion_for(&fixture, &gap_completion);
    let gap_connection = fixture.store.connect();
    let collected_gap = Collector::new(&gap_connection, fixture.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(gap_completion_digest),
            ..CollectionRequest::default()
        })
        .expect("C11.05.GAP_COLLECTS");
    match &collected_gap
        .spend
        .as_ref()
        .expect("C11.05.GAP_SPEND_PRESENT")
        .completeness
    {
        SpendCompleteness::Partial(gaps) => {
            assert_eq!(gaps.len(), 1, "C11.05.GAP_IS_SINGULAR");
            assert_eq!(
                gaps[0].kind, "spend_bearing_event_after_coverage",
                "C11.05.GAP_NAMES_CAUSE"
            );
            assert!(
                gaps[0].detail.contains("run.preflighted")
                    && gaps[0].detail.contains(&gap_run_seq.to_string()),
                "C11.05.GAP_NAMES_EVENT"
            );
        }
        other => panic!("C11.05.SPEND_WINDOW_IS_PARTIAL: {other:?}"),
    }
    drop(gap_connection);
    let gap_candidate = fixture
        .outcomes
        .prepare_for_mission(gap_mission_id, AppendixRequest::PolicyDefault)
        .expect("C11.05.GAP_PREPARES");
    let gap_stored = fixture
        .outcomes
        .manifest(&gap_candidate.manifest_digest)
        .expect("C11.05.GAP_MANIFEST_READ");
    assert_eq!(
        gap_stored
            .manifest()
            .spend_ref
            .verified()
            .expect("C11.05.GAP_SPEND_VERIFIED")
            .measurement,
        SpendMeasurementKind::PartiallyMeasured,
        "C11.05.SPEND_WINDOW_IS_PARTIAL"
    );

    // Step 6: verify all three hashes independently, then compare the frozen membership and prose
    // with the owner-side ledger assembled above -- never with a second call to the renderer.
    let primary_candidate_record = fixture
        .outcomes
        .candidate(&primary_candidate.manifest_digest)
        .expect("C11.06.CANDIDATE_OWNER_READ");
    let primary_candidate_from_artifacts = artifacts(&fixture)
        .get(&primary_candidate.candidate_artifact_digest)
        .expect("C11.06.CANDIDATE_ARTIFACT_OWNER_READ");
    assert_eq!(
        primary_candidate_record, primary_candidate_from_artifacts,
        "C11.06.MANIFEST_OWNER_BINDINGS"
    );
    assert_eq!(
        primary_stored.digest().value,
        data_encoding::HEXLOWER.encode(&sha256_bytes(primary_stored.canonical_bytes())),
        "C11.06.RECORD_AND_CONTENT_DIGESTS"
    );
    assert_eq!(
        primary_candidate_record.digest().value,
        primary_candidate.candidate_artifact_digest,
        "C11.06.RECORD_AND_CONTENT_DIGESTS"
    );
    assert_eq!(
        primary_candidate_record.payload().content_digest.value,
        data_encoding::HEXLOWER.encode(&sha256_bytes(primary_candidate_record.content())),
        "C11.06.RECORD_AND_CONTENT_DIGESTS"
    );
    assert_eq!(
        primary_manifest.mission_id, fixture.mission_id,
        "C11.06.MANIFEST_OWNER_BINDINGS"
    );
    assert_eq!(
        primary_manifest.completion.digest.value, primary_completion_digest,
        "C11.06.MANIFEST_OWNER_BINDINGS"
    );
    assert_eq!(
        primary_manifest.accountable_resident_id, fixture.resident_id,
        "C11.06.MANIFEST_OWNER_BINDINGS"
    );

    let conditions = &primary_manifest.success_conditions;
    assert_eq!(conditions.len(), 4, "C11.06.CONDITION_SUPPORT_EXACT");
    assert!(
        matches!(
            conditions[0].outcome,
            OutcomeState::Verified(ConditionResult::Met)
        ),
        "C11.06.CONDITION_STATES_EXACT"
    );
    assert!(
        matches!(
            conditions[1].outcome,
            OutcomeState::Verified(ConditionResult::NotMet)
        ),
        "C11.06.CONDITION_STATES_EXACT"
    );
    assert!(
        matches!(&conditions[2].outcome, OutcomeState::NotEvaluated(reason) if reason == "not_assessed"),
        "C11.06.CONDITION_STATES_EXACT"
    );
    assert!(
        matches!(
            conditions[3].outcome,
            OutcomeState::Verified(ConditionResult::Inconclusive)
        ),
        "C11.06.CONDITION_STATES_EXACT"
    );
    let condition_zero_refs = conditions[0]
        .citations
        .verified()
        .expect("C11.06.CONDITION_SUPPORT_EXACT");
    assert_eq!(
        condition_zero_refs.len(),
        4,
        "C11.06.CONDITION_SUPPORT_EXACT"
    );
    for (kind, digest) in [
        (SourceKind::Artifact, delivered.digest().value.as_str()),
        (SourceKind::Artifact, supporting.digest().value.as_str()),
        (
            SourceKind::Evaluation,
            support_evaluation.digest().value.as_str(),
        ),
        (SourceKind::Ruling, ruling.digest().value.as_str()),
    ] {
        assert!(
            condition_zero_refs
                .iter()
                .any(|reference| reference.kind == kind && reference.digest.value == digest),
            "C11.06.CONDITION_SUPPORT_EXACT: missing {kind:?} {digest}"
        );
    }
    let condition_one_refs = conditions[1]
        .citations
        .verified()
        .expect("C11.06.CONDITION_SUPPORT_EXACT");
    assert_eq!(
        condition_one_refs.len(),
        1,
        "C11.06.CONDITION_SUPPORT_EXACT"
    );
    assert_eq!(
        condition_one_refs[0].digest.value,
        delivered.digest().value,
        "C11.06.CONDITION_SUPPORT_EXACT"
    );
    for condition in &conditions[2..] {
        assert!(
            matches!(&condition.citations, OutcomeState::Absent(reason) if !reason.is_empty()),
            "C11.06.CONDITION_SUPPORT_EXACT"
        );
    }
    let delivered_refs = primary_manifest
        .delivered_artifacts
        .verified()
        .expect("C11.06.DELIVERIES_EXACT");
    assert_eq!(delivered_refs.len(), 1, "C11.06.DELIVERIES_EXACT");
    assert_eq!(
        delivered_refs[0].digest.value,
        delivered.digest().value,
        "C11.06.DELIVERIES_EXACT"
    );
    let frozen_digests = primary_manifest
        .success_conditions
        .iter()
        .flat_map(|condition| condition.citations.verified().into_iter().flatten())
        .chain(delivered_refs.iter())
        .map(|reference| reference.digest.value.as_str())
        .collect::<Vec<_>>();
    assert!(
        !frozen_digests.contains(&decoy.digest().value.as_str())
            && !frozen_digests.contains(&decoy_evaluation.digest().value.as_str()),
        "C11.06.CONDITION_SUPPORT_EXACT"
    );

    let primary_markdown =
        std::str::from_utf8(primary_candidate_record.content()).expect("C11.06.MARKDOWN_UTF8");
    for statement in CONDITION_TEXTS {
        assert!(
            primary_markdown.contains(statement),
            "C11.06.CONDITION_TEXT: {statement}"
        );
    }
    assert!(
        primary_markdown.contains("Correct the remaining declared-check failure.")
            && primary_markdown.contains("Two verified observations still disagree."),
        "C11.06.NARRATIVE_FROM_COMPLETION"
    );
    assert_eq!(
        primary_manifest.appendix_mode(),
        AppendixMode::Included,
        "C11.06.HIGH_IMPACT_APPENDIX"
    );

    let idempotence_counts = (
        table_count(&fixture.store, "outcome_manifests", fixture.workspace_id),
        table_count(&fixture.store, "artifacts", fixture.workspace_id),
        event_count(&fixture.store, fixture.workspace_id),
    );
    let primary_retry = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("C11.06.PREPARE_RETRY");
    assert_eq!(
        primary_retry, primary_candidate,
        "C11.06.PREPARE_IDEMPOTENT"
    );
    assert_eq!(
        (
            table_count(&fixture.store, "outcome_manifests", fixture.workspace_id),
            table_count(&fixture.store, "artifacts", fixture.workspace_id),
            event_count(&fixture.store, fixture.workspace_id),
        ),
        idempotence_counts,
        "C11.06.PREPARE_IDEMPOTENT"
    );

    let broken_mission_id = admit_companion_mission(
        &fixture,
        "Preserve a legitimate broken spend state without inventing an amount",
    );
    let _broken_classification = classify_mission(
        &fixture,
        broken_mission_id,
        1,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::None,
    );
    let broken_reason = "C11 ledger could not be verified";
    let broken_coverage = latest_event_seq(&fixture.store, fixture.workspace_id);
    let (broken_spend, broken_spend_record) = record_spend(
        &fixture,
        broken_mission_id,
        broken_coverage,
        SpendMeasurement::Broken(broken_reason.to_owned()),
    );
    let broken_boundary = latest_event_seq(&fixture.store, fixture.workspace_id);
    let broken_completion = completion_record(
        &fixture,
        broken_mission_id,
        broken_boundary,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        SpendReference::Recorded {
            spend_id: broken_spend.spend_id,
            spend_version: broken_spend.version,
            spend_sha256: digest_bytes(&broken_spend_record.digest().value),
        },
        false,
    );
    record_completion_for(&fixture, &broken_completion);
    let broken_candidate = fixture
        .outcomes
        .prepare_for_mission(broken_mission_id, AppendixRequest::PolicyDefault)
        .expect("C11.06.BROKEN_PREPARES");
    let broken_stored = fixture
        .outcomes
        .manifest(&broken_candidate.manifest_digest)
        .expect("C11.06.BROKEN_MANIFEST_READ");
    assert!(
        matches!(&broken_stored.manifest().spend_ref, OutcomeState::Broken(reason) if reason == broken_reason),
        "C11.06.BROKEN_IS_A_DOMAIN_STATE"
    );
    assert_eq!(
        broken_stored.manifest().appendix_mode(),
        AppendixMode::PolicyStateNotHighImpact,
        "C11.06.ORDINARY_PROOF_REMAINS"
    );
    let broken_record = fixture
        .outcomes
        .candidate(&broken_candidate.manifest_digest)
        .expect("C11.06.BROKEN_CANDIDATE_READ");
    let broken_markdown =
        std::str::from_utf8(broken_record.content()).expect("C11.06.BROKEN_MARKDOWN_UTF8");
    assert!(
        broken_markdown.contains("`broken`") && broken_markdown.contains(broken_reason),
        "C11.06.BROKEN_IS_A_DOMAIN_STATE"
    );
    assert!(
        broken_markdown.contains("Manifest proof")
            && !broken_markdown.contains("Cited objects")
            && !broken_markdown.contains("1234"),
        "C11.06.ORDINARY_PROOF_REMAINS"
    );
    assert!(
        matches!(
            fixture.outcomes.get(fixture.mission_id),
            Err(PublishRefusal::Absent)
        ),
        "C11.06.PREPARE_IS_NOT_PUBLICATION"
    );

    // Step 7: both negative cases use valid owner identities (or a valid absent envelope) and
    // preserve every byte already frozen. An arbitrary malformed digest would prove less.
    let absent_evaluation = EvaluationDigest {
        algorithm: support_evaluation.digest().algorithm.clone(),
        canonicalizer: support_evaluation.digest().canonicalizer.clone(),
        value: "ab".repeat(32),
    };
    let refusal_snapshot = (
        primary_stored.canonical_bytes().to_vec(),
        primary_candidate_record.content().to_vec(),
        latest_event_seq(&fixture.store, fixture.workspace_id),
        event_count(&fixture.store, fixture.workspace_id),
        publication_count(&fixture.store, fixture.workspace_id, fixture.mission_id),
    );
    assert!(
        matches!(
            fixture.outcomes.publish(
                fixture.mission_id,
                primary_candidate_record.digest(),
                &absent_evaluation,
            ),
            Err(PublishRefusal::Seam(
                PublicationSeamRefusal::EvaluationAbsent
            ))
        ),
        "C11.07.ABSENT_QUALIFIER_PRESERVES_CANDIDATE"
    );
    assert!(
        matches!(
            fixture.outcomes.publish(
                fixture.mission_id,
                primary_candidate_record.digest(),
                support_evaluation.digest(),
            ),
            Err(PublishRefusal::Seam(
                PublicationSeamRefusal::SubjectIsNotTheCandidate
            ))
        ),
        "C11.07.WRONG_SUBJECT_REFUSED"
    );
    let refused_manifest = fixture
        .outcomes
        .manifest(&primary_candidate.manifest_digest)
        .expect("C11.07.MANIFEST_SURVIVES_REFUSALS");
    let refused_candidate = fixture
        .outcomes
        .candidate(&primary_candidate.manifest_digest)
        .expect("C11.07.CANDIDATE_SURVIVES_REFUSALS");
    assert_eq!(
        (
            refused_manifest.canonical_bytes().to_vec(),
            refused_candidate.content().to_vec(),
            latest_event_seq(&fixture.store, fixture.workspace_id),
            event_count(&fixture.store, fixture.workspace_id),
            publication_count(&fixture.store, fixture.workspace_id, fixture.mission_id),
        ),
        refusal_snapshot,
        "C11.07.REFUSAL_WRITES_NOTHING"
    );
    assert!(
        matches!(
            fixture.outcomes.get(fixture.mission_id),
            Err(PublishRefusal::Absent)
        ),
        "C11.07.REFUSAL_WRITES_NOTHING"
    );

    // Step 8: qualify the candidate record digest (not its raw-content digest) and prove the only
    // state transition is the immutable publication relation.
    let qualifying_evaluation =
        evaluate_artifact(&fixture, &primary_candidate.candidate_artifact_digest);
    assert_ne!(
        qualifying_evaluation.payload().evaluator.resident_id,
        fixture.resident_id,
        "C11.08.EXACT_NONAUTHOR_QUALIFICATION"
    );
    match &qualifying_evaluation.payload().subject {
        EvaluationSubject::Artifact(subject) => {
            assert_eq!(
                subject.digest,
                *primary_candidate_record.digest(),
                "C11.08.EXACT_NONAUTHOR_QUALIFICATION"
            );
            assert_eq!(
                subject.mission_id, fixture.mission_id,
                "C11.08.EXACT_NONAUTHOR_QUALIFICATION"
            );
        }
        other => panic!("C11.08.EXACT_NONAUTHOR_QUALIFICATION: {other:?}"),
    }
    let mission_before_publish = mission_service
        .get(fixture.mission_id)
        .expect("C11.08.MISSION_SNAPSHOT");
    let completion_before_publish = verified_completion_within(
        &fixture.store.connect(),
        fixture.workspace_id,
        &primary_completion_digest,
    )
    .expect("C11.08.COMPLETION_SNAPSHOT");
    let policy_before_publish =
        ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .current(fixture.project_id)
            .expect("C11.08.POLICY_SNAPSHOT");
    let ruling_before_publish =
        RulingService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .get(&ruling.digest().value)
            .expect("C11.08.RULING_SNAPSHOT");
    let publish_event_count = event_count(&fixture.store, fixture.workspace_id);
    let publish_relation_count =
        publication_count(&fixture.store, fixture.workspace_id, fixture.mission_id);
    let publication_v1 = fixture
        .outcomes
        .publish(
            fixture.mission_id,
            primary_candidate_record.digest(),
            qualifying_evaluation.digest(),
        )
        .expect("C11.08.PUBLISH_V1");
    assert_eq!(
        publication_v1.publication_version(),
        1,
        "C11.08.PUBLICATION_PAIR_ONE_OF_ONE"
    );
    assert_eq!(
        publication_v1.publication_version_count(),
        1,
        "C11.08.PUBLICATION_PAIR_ONE_OF_ONE"
    );
    assert_eq!(
        publication_v1.cited_candidate_artifact_digest(),
        primary_candidate_record.digest(),
        "C11.08.PUBLICATION_PAIR_ONE_OF_ONE"
    );
    assert_eq!(
        publication_v1.cited_evaluation_digest(),
        qualifying_evaluation.digest(),
        "C11.08.PUBLICATION_PAIR_ONE_OF_ONE"
    );
    assert_eq!(
        publication_count(&fixture.store, fixture.workspace_id, fixture.mission_id),
        publish_relation_count + 1,
        "C11.08.PUBLICATION_PAIR_ONE_OF_ONE"
    );
    assert_eq!(
        mission_service
            .get(fixture.mission_id)
            .expect("C11.08.MISSION_RECHECK"),
        mission_before_publish,
        "C11.08.PUBLISH_CHANGES_NO_WORK_OR_AUTHORITY"
    );
    assert_eq!(
        verified_completion_within(
            &fixture.store.connect(),
            fixture.workspace_id,
            &primary_completion_digest,
        )
        .expect("C11.08.COMPLETION_RECHECK"),
        completion_before_publish,
        "C11.08.PUBLISH_CHANGES_NO_WORK_OR_AUTHORITY"
    );
    assert_eq!(
        ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .current(fixture.project_id)
            .expect("C11.08.POLICY_RECHECK"),
        policy_before_publish,
        "C11.08.PUBLISH_CHANGES_NO_WORK_OR_AUTHORITY"
    );
    assert_eq!(
        RulingService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
            .get(&ruling.digest().value)
            .expect("C11.08.RULING_RECHECK"),
        ruling_before_publish,
        "C11.08.PUBLISH_CHANGES_NO_WORK_OR_AUTHORITY"
    );
    assert_eq!(
        event_count(&fixture.store, fixture.workspace_id),
        publish_event_count,
        "C11.08.PUBLISH_CHANGES_NO_WORK_OR_AUTHORITY"
    );
    assert!(
        matches!(
            publication_v1.manifest().manifest().success_conditions[1].outcome,
            OutcomeState::Verified(ConditionResult::NotMet)
        ),
        "C11.08.PUBLISH_IS_NOT_SUCCESS"
    );

    let _ = (
        saved_store_path,
        primary_candidate,
        primary_stored,
        primary_candidate_record,
        qualifying_evaluation,
        publication_v1,
    );
    panic!("C11.09.NOT_YET");
}
