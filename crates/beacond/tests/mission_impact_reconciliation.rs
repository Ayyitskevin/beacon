//! B6-REC: the classification owner reconciliation (R58.3, Fold 2 + adjudication B6.11).
//!
//! Every product record here is created through the public services. The fixture helpers below the
//! header are lifted verbatim from `tests/outcome_closure.rs` -- same Workspace, Project, Policy,
//! Residents, Team, admitted Mission, artifacts, evaluation, spend and completion, built by the
//! same public calls -- because a reconciliation test that invented its own store shape would prove
//! something about the fixture rather than the product. Only corruption and legacy-boundary setup
//! uses explicit SQL, and every such plant names its boundary.
//!
//! The unread helpers are kept, not trimmed: they are what make this the SAME fixture the Outcome
//! path uses, and the manifest bar below needs most of the chain they form.

#![allow(dead_code)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AppendixRequest, ArtifactContext, ArtifactKind, ArtifactService, AutonomyProfile,
    BeaconCapability, CheckPreflightService, ClassificationActor, ClassificationBasis,
    ClassificationEventError, ClassificationEventField, ClassificationStoreError,
    ClassificationSupersedes, CompletionActor, CompletionDisposition, CompletionSupersedes,
    ConditionOutcome, ConditionResultRecord, Conflicts, DeclaredCheck, Evaluation, EvaluationDraft,
    EvaluationService, EvaluationSubjectDraft, EvaluationSupersedes, Evaluator, Finding,
    FindingSeverity, ImpactLevel, LineAttribution, LocalModelActivationService, LocalModelEndpoint,
    MeasuredSpend, MeasuredSpendService, MeasuredSpendV1, MissionCompletionV1, MissionDraft,
    MissionImpactClassification, MissionImpactClassificationV1, MissionImpactService,
    MissionMemberDraft, MissionService, NarrativeItem, OperatorAccountService,
    OperatorSessionPhase, OutcomeService, PolicyRevisionRef, ProjectCharterDraft, ProjectDraft,
    ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService, ProjectService,
    ProposalService, ProviderFeature, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationService, ResidentLifecycleService, RubricDeclaration, RulingDecision,
    RulingService, RulingSupersedes, SecretString, SpendEra, SpendLine, SpendMeasurement,
    SpendOrigin, SpendReference, SpendSupersedes, SpendTotals, StarterOrganizationService,
    StarterResidentDraft, StarterRole, StartupConfig, Supervisor, SupervisorError, SupportRef,
    TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule,
    TeamLeadSelectionPolicy, TeamMemberDraft, TeamService, TrackRecordEvidenceKind,
    TrackRecordRequirement, UncertaintyKind, UnfinishedKind, Verdict, declared_conditions_digest,
    record_completion, verified_completion_within,
};
use rusqlite::{Connection, params};
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

fn supersede_artifact_evaluation(
    fixture: &Fixture,
    artifact_digest: &str,
    predecessor: &Evaluation,
) -> Evaluation {
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
                summary: "The later read confirms the originally cited candidate bytes.".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "This is a successor record, not a rewrite of the cited Evaluation."
                .to_owned(),
            supersedes: EvaluationSupersedes::Evaluation(predecessor.digest().value.clone()),
        })
        .expect("C11.10.EVALUATION_SUCCESSOR_RECORDED")
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
        era: SpendEra::V2,
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
            attribution: LineAttribution::unattributed("fixture"),
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

#[allow(clippy::too_many_arguments)]
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
        unfinished_work: if narratives {
            vec![NarrativeItem {
                ordinal: 0,
                kind: UnfinishedKind::ConditionNotMet,
                condition_ordinal: Some(1),
                summary: "Correct the remaining declared-check failure.".to_owned(),
            }]
        } else {
            Vec::new()
        },
        uncertainty: if narratives {
            vec![NarrativeItem {
                ordinal: 0,
                kind: UncertaintyKind::ConflictingEvidence,
                condition_ordinal: Some(3),
                summary: "Two verified observations still disagree.".to_owned(),
            }]
        } else {
            Vec::new()
        },
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

/// Test-only PLANT boundary used only for R29.3's distinguishing stored-byte witness. The trigger
/// is restored immediately; the closure never calls this for its primary owner-written path.
fn with_update_trigger_down(
    store: &TestStore,
    name: &str,
    _table: &str,
    _message: &str,
    plant: impl FnOnce(&Connection),
) {
    let connection = store.connect();
    let original_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [name],
            |row| row.get(0),
        )
        .expect("C11.12.PLANT_TRIGGER_READ");
    connection
        .execute_batch(&format!("DROP TRIGGER {name};"))
        .expect("C11.12.PLANT_TRIGGER_DROP");
    plant(&connection);
    connection
        .execute_batch(&original_sql)
        .expect("C11.12.PLANT_TRIGGER_RESTORE");
}

/// Read the appendix as an outside verifier would: locate the one proof block, concatenate its
/// lines and decode them. This intentionally does not call the production renderer or codec.
fn manifest_proof_from_markdown(markdown: &str) -> Vec<u8> {
    assert_eq!(
        markdown.matches("Manifest proof").count(),
        1,
        "C11.09.APPENDIX_MANIFEST_PROOF"
    );
    let proof = markdown
        .split_once("Manifest proof")
        .expect("C11.09.APPENDIX_MANIFEST_PROOF")
        .1;
    let fenced = proof
        .split_once("```\n")
        .expect("C11.09.APPENDIX_MANIFEST_PROOF")
        .1;
    let hex = fenced
        .split_once("\n```")
        .expect("C11.09.APPENDIX_MANIFEST_PROOF")
        .0
        .lines()
        .collect::<String>();
    assert_eq!(
        data_encoding::HEXLOWER.encode(
            &data_encoding::HEXLOWER
                .decode(hex.as_bytes())
                .expect("C11.09.APPENDIX_MANIFEST_PROOF")
        ),
        hex,
        "C11.09.APPENDIX_MANIFEST_PROOF"
    );
    data_encoding::HEXLOWER
        .decode(hex.as_bytes())
        .expect("C11.09.APPENDIX_MANIFEST_PROOF")
}

/// Return only the appendix's cited-object data rows, preserving all eight displayed cells.
fn appendix_object_rows(markdown: &str) -> Vec<Vec<String>> {
    let section = markdown
        .split_once(" Cited objects\n")
        .expect("C11.09.APPENDIX_OBJECT_ROWS_EXACT")
        .1
        .split_once("\n### ")
        .expect("C11.09.APPENDIX_OBJECT_ROWS_EXACT")
        .0;
    assert!(
        section.contains(
            "| kind | identity | schema | digest algorithm | digest | canonicalizer | pinned revision | verification result |"
        ),
        "C11.09.APPENDIX_OBJECT_ROWS_EXACT"
    );
    section
        .lines()
        .filter(|line| line.starts_with("| `"))
        .map(|line| {
            let cells = line
                .split('|')
                .skip(1)
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .map(|cell| cell.to_owned())
                .collect::<Vec<_>>();
            assert_eq!(cells.len(), 8, "C11.09.APPENDIX_OBJECT_ROWS_EXACT");
            cells
        })
        .collect()
}

// ---------------------------------------------------------------------------
// B6-REC — the owner relation
// ---------------------------------------------------------------------------

/// One classified Mission, built entirely through the public services.
fn classified(label: &str) -> (Fixture, MissionImpactClassification) {
    let fixture = admitted_mission(label);
    let classification = classify_mission(
        &fixture,
        fixture.mission_id,
        1,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::None,
    );
    (fixture, classification)
}

fn reopen(fixture: &Fixture) -> Result<(), SupervisorError> {
    Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .map(|_| ())
}

/// PLANT BOUNDARY: corruption only. No public call can remove an Event -- that is the tamper.
///
/// The Event is selected through an explicit `ORDER BY`, never by assumed insertion order, and the
/// stored chain head is moved back **coherently** when the deleted Event was the terminal one.
///
/// The coherent head is not decoration. Measured on the first run of this probe: a bare delete of
/// the terminal Event is refused by `EventChainBroken(HeadMismatch { expected 12, found 11 })`
/// before the owner is ever asked. A test that stopped there would pin the chain and call it an
/// owner bar -- the exact masking this lane has been caught by before. R58.3's threat is the
/// truncation the chain CANNOT see, so the plant reproduces that one.
fn delete_the_classification_event(fixture: &Fixture) -> i64 {
    let connection = fixture.store.connect();
    let sequence: i64 = connection
        .query_row(
            "SELECT sequence FROM events WHERE kind = 'mission.impact_classified'
              ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("B6.PLANT.EVENT_PRESENT");
    connection
        .execute("DELETE FROM events WHERE sequence = ?1", params![sequence])
        .expect("B6.PLANT.EVENT_DELETED");
    recohere_chain_head(&connection);
    sequence
}

/// PLANT BOUNDARY: move `event_chain_state` to the newest surviving chained Event, so the chain
/// verifies and the owner is the only pass that can still object.
fn recohere_chain_head(connection: &Connection) {
    connection
        .execute_batch(
            "UPDATE event_chain_state
                SET head_sequence = (SELECT coalesce(max(sequence), 0) FROM events
                                      WHERE predecessor_kind != 'unchained'),
                    head_digest = (SELECT link_digest FROM events
                                    WHERE predecessor_kind != 'unchained'
                                    ORDER BY sequence DESC LIMIT 1)
              WHERE id = 1;",
        )
        .expect("B6.PLANT.HEAD_RECOHERED");
}

/// B6.C.INTACT — the honest control. Without it every refusal below could be a blanket refusal.
#[test]
fn a_classified_store_with_its_event_intact_opens_ready() {
    let (fixture, _) = classified("b6-intact");
    reopen(&fixture).expect("B6.C.INTACT: an untampered classified store reopens Ready");
}

/// B6.R.ABSENT — the defect this lane exists to close.
#[test]
fn a_classification_whose_event_was_deleted_is_refused_as_event_absent() {
    let (fixture, classification) = classified("b6-absent");
    delete_the_classification_event(&fixture);
    let refusal =
        reopen(&fixture).expect_err("B6.R.ABSENT: a classification with no Event refuses");
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch { workspace_id, digest, cause }
                if *workspace_id == fixture.workspace_id
                    && digest == &classification.digest().value
                    && matches!(cause, ClassificationStoreError::EventAbsent)
        ),
        "B6.R.ABSENT: expected EventAbsent keyed by the verified row, got {refusal:?}"
    );
}

/// PLANT BOUNDARY: rewrite the stored Event's payload. No public call can edit an Event.
fn rewrite_event_payload(fixture: &Fixture, sql_expr: &str) {
    fixture
        .store
        .connect()
        .execute_batch(&format!(
            "UPDATE events SET payload_json = {sql_expr}
              WHERE kind = 'mission.impact_classified'"
        ))
        .expect("B6.PLANT.PAYLOAD_REWRITTEN");
}

fn refusal(fixture: &Fixture) -> SupervisorError {
    reopen(fixture).expect_err("B6: the plant must refuse startup")
}

/// B6.R.DUPLICATED — two Events claim one classification; the fault names the count.
#[test]
fn a_second_event_for_one_classification_is_refused_as_duplicated() {
    let (fixture, classification) = classified("b6-duplicate");
    fixture
        .store
        .connect()
        .execute_batch(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms,
                                 predecessor_kind, predecessor_digest, link_digest)
             SELECT workspace_id, kind, payload_json, occurred_at_ms, 'unchained', NULL, NULL
               FROM events WHERE kind = 'mission.impact_classified' ORDER BY sequence LIMIT 1;",
        )
        .expect("B6.PLANT.EVENT_DUPLICATED");
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch { workspace_id, digest, cause }
                if *workspace_id == fixture.workspace_id
                    && digest == &classification.digest().value
                    && matches!(cause, ClassificationStoreError::EventDuplicated { count: 2 })
        ),
        "B6.R.DUPLICATED: {refusal:?}"
    );
}

/// B6.U.ORPHAN — an Event for a classification this store does not hold.
#[test]
fn an_event_for_a_classification_this_store_never_held_is_unaccounted() {
    let (fixture, _) = classified("b6-orphan");
    // PLANT BOUNDARY: a SECOND Event naming a classification this store never recorded. The rows
    // are immutable by trigger, so the orphan is made by adding an Event rather than removing a
    // row -- which is the truer shape anyway.
    fixture
        .store
        .connect()
        .execute_batch(&format!(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms,
                                 predecessor_kind, predecessor_digest, link_digest)
             SELECT workspace_id, kind,
                    json_set(payload_json, '$.digest.value', '{}'),
                    occurred_at_ms, 'unchained', NULL, NULL
               FROM events WHERE kind = 'mission.impact_classified' ORDER BY sequence LIMIT 1;",
            "c".repeat(64)
        ))
        .expect("B6.PLANT.ORPHAN_EVENT");
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                if matches!(cause, ClassificationEventError::Unaccounted)
        ),
        "B6.U.ORPHAN: {refusal:?}"
    );
}

/// B6.U.ASSOCIATION_KEY and B6.X.EVENT_BEFORE_ROW in one cell, which is what a precedence cell is
/// for: the altered key raises BOTH conditions and the Event-side one must win.
#[test]
fn an_event_whose_digest_value_locates_no_row_is_unaccounted_not_absent() {
    let (fixture, _) = classified("b6-association");
    rewrite_event_payload(
        &fixture,
        &format!(
            "json_set(payload_json, '$.digest.value', '{}')",
            "b".repeat(64)
        ),
    );
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                if matches!(cause, ClassificationEventError::Unaccounted)
        ),
        "B6.X.EVENT_BEFORE_ROW: the row-side EventAbsent must not win here: {refusal:?}"
    );
}

/// The Malformed armings, one invalid thing per vector.
#[test]
fn an_event_that_fails_one_decode_check_is_refused_as_malformed() {
    let vectors: [(&str, &str); 12] = [
        ("B6.M.UNKNOWN_TOP", "json_set(payload_json, '$.forged', 1)"),
        (
            "B6.M.UNKNOWN_DIGEST",
            "json_set(payload_json, '$.digest.forged', 1)",
        ),
        (
            "B6.M.VERSION_ZERO",
            "json_set(payload_json, '$.version', 0)",
        ),
        (
            "B6.M.POLICY_ZERO",
            "json_set(payload_json, '$.policy_revision', 0)",
        ),
        (
            "B6.M.CHARTER_ZERO",
            "json_set(payload_json, '$.charter_revision', 0)",
        ),
        (
            "B6.M.DIGEST_LEN",
            "json_set(payload_json, '$.digest.value', 'abc')",
        ),
        (
            "B6.M.TIME_NEGATIVE",
            "json_set(payload_json, '$.classified_at_ms', -1)",
        ),
        // A u64 that does not fit i64: it decodes, and the `i64::try_from` domain check refuses it.
        // Written as a text substitution because SQLite's own integers stop at i64, so `json_set`
        // cannot produce this value.
        (
            "B6.M.TIME_OVERFLOW",
            "replace(payload_json, '\"classified_at_ms\":1788400000001', \
             '\"classified_at_ms\":18446744073709551615')",
        ),
        (
            "B6.M.DIGEST_HEX",
            "json_set(payload_json, '$.digest.value', \
             'g000000000000000000000000000000000000000000000000000000000000000')",
        ),
        (
            "B6.M.DIGEST_ALGORITHM",
            "json_set(payload_json, '$.digest.algorithm', 'sha512')",
        ),
        (
            "B6.M.DIGEST_CANONICALIZER",
            "json_set(payload_json, '$.digest.canonicalizer', 'beacon.mission-impact.v2')",
        ),
        (
            "B6.M.BASE64",
            "json_set(payload_json, '$.canonical_bytes_base64', 'not base64!!')",
        ),
    ];
    for (case, expr) in vectors {
        let (fixture, _) = classified(&format!("b6-malformed-{}", case.to_lowercase()));
        rewrite_event_payload(&fixture, expr);
        let refusal = refusal(&fixture);
        assert!(
            matches!(
                &refusal,
                SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                    if matches!(cause, ClassificationEventError::Malformed)
            ),
            "{case}: {refusal:?}"
        );
    }
}

/// B6.M.SEQUENCE_ZERO / _NEGATIVE / _MIN — B6.11's signed-domain adjudication, armed separately.
#[test]
fn an_event_whose_sequence_is_outside_its_domain_is_refused_as_malformed() {
    for (case, sequence) in [
        ("B6.M.SEQUENCE_ZERO", 0_i64),
        ("B6.M.SEQUENCE_NEGATIVE", -1),
        ("B6.M.SEQUENCE_MIN", i64::MIN),
    ] {
        let (fixture, _) = classified(&format!("b6-seq-{}", case.to_lowercase()));
        fixture
            .store
            .connect()
            .execute_batch(&format!(
                "UPDATE events SET sequence = {sequence}
                  WHERE kind = 'mission.impact_classified';"
            ))
            .expect("B6.PLANT.SEQUENCE");
        let refusal = refusal(&fixture);
        assert!(
            matches!(
                &refusal,
                SupervisorError::MissionImpactClassificationEventMismatch { sequence: named, cause }
                    if *named == sequence && matches!(cause, ClassificationEventError::Malformed)
            ),
            "{case}: the real signed value must be named: {refusal:?}"
        );
    }
}

/// Every reachable disagreement label, one vector each.
#[test]
fn an_event_that_disagrees_on_one_field_is_refused_and_names_that_field() {
    let vectors: [(ClassificationEventField, &str); 12] = [
        (ClassificationEventField::OuterOccurredAtMs, "OUTER_TIME"),
        (
            ClassificationEventField::PayloadWorkspaceId,
            "json_set(payload_json, '$.workspace_id', '11111111-1111-4111-8111-111111111111')",
        ),
        (
            ClassificationEventField::ClassificationId,
            "json_set(payload_json, '$.classification_id', '11111111-1111-4111-8111-111111111111')",
        ),
        (
            ClassificationEventField::MissionId,
            "json_set(payload_json, '$.mission_id', '11111111-1111-4111-8111-111111111111')",
        ),
        (
            ClassificationEventField::ProjectId,
            "json_set(payload_json, '$.project_id', '11111111-1111-4111-8111-111111111111')",
        ),
        (
            ClassificationEventField::Version,
            "json_set(payload_json, '$.version', 7)",
        ),
        (
            ClassificationEventField::Impact,
            "json_set(payload_json, '$.impact', 'high')",
        ),
        (
            ClassificationEventField::Basis,
            "json_set(payload_json, '$.basis', 'policy_rule')",
        ),
        (
            ClassificationEventField::PolicyRevision,
            "json_set(payload_json, '$.policy_revision', 9)",
        ),
        (
            ClassificationEventField::CharterRevision,
            "json_set(payload_json, '$.charter_revision', 9)",
        ),
        // NOT base+1: `classify_mission` stamps `1_788_400_000_000 + version`, so the obvious
        // "+1" vector equals the row it is meant to differ from and the cell proves nothing. It
        // passed as a no-op until the loop named which label failed.
        (
            ClassificationEventField::ClassifiedAtMs,
            "json_set(payload_json, '$.classified_at_ms', 1788499999999)",
        ),
        (
            ClassificationEventField::CanonicalBytesBase64,
            "json_set(payload_json, '$.canonical_bytes_base64', 'AA==')",
        ),
    ];
    assert_eq!(
        vectors.len(),
        ClassificationEventField::ALL.len(),
        "B6.D: one vector per reachable label, no label unarmed"
    );
    for (field, expr) in vectors {
        let (fixture, _) = classified(&format!("b6-d-{}", field.as_str()));
        if expr == "OUTER_TIME" {
            // A valid nonnegative value (B6.11), so the earlier global check cannot mask this.
            fixture
                .store
                .connect()
                .execute_batch(
                    "UPDATE events SET occurred_at_ms = occurred_at_ms + 1
                      WHERE kind = 'mission.impact_classified';",
                )
                .expect("B6.PLANT.OUTER_TIME");
        } else {
            rewrite_event_payload(&fixture, expr);
        }
        let refusal = reopen(&fixture).expect_err(&format!(
            "B6.D.{}: the plant must refuse startup",
            field.as_str()
        ));
        assert!(
            matches!(
                &refusal,
                SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                    if matches!(cause, ClassificationEventError::Disagrees { field: named } if *named == field)
            ),
            "B6.D.{}: {refusal:?}",
            field.as_str()
        );
    }
}

/// B6.S.SUPERSEDED_MIDDLE — a superseded row still needs its own Event, and the classification
/// fault must win over any chain refusal the same deletion causes.
#[test]
fn a_superseded_middle_row_still_needs_its_own_event() {
    let (fixture, older) = classified("b6-superseded");
    let _newer = classify_mission(
        &fixture,
        fixture.mission_id,
        2,
        ImpactLevel::High,
        ClassificationSupersedes::Record(older.digest().value.clone()),
    );
    fixture
        .store
        .connect()
        .execute_batch(&format!(
            "DELETE FROM events WHERE kind = 'mission.impact_classified'
               AND json_extract(payload_json, '$.digest.value') = '{}';",
            older.digest().value
        ))
        .expect("B6.PLANT.MIDDLE_EVENT_DELETED");
    recohere_chain_head(&fixture.store.connect());
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch { digest, cause, .. }
                if digest == &older.digest().value
                    && matches!(cause, ClassificationStoreError::EventAbsent)
        ),
        "B6.S.SUPERSEDED_MIDDLE: a chain refusal here would mask a removed owner guard: {refusal:?}"
    );
}

/// B6.X.MALFORMED_BEFORE_DISAGREES — an Event that is BOTH unreadable and disagreeing is refused as
/// malformed. Nothing can be compared until it decodes, so the later check must not be reached.
///
/// A precedence cell, and it stands for nothing else: both its armings are covered independently by
/// `B6.M.UNKNOWN_TOP` and `B6.D.version`.
#[test]
fn an_event_that_is_both_unreadable_and_disagreeing_is_refused_as_malformed() {
    let (fixture, _) = classified("b6-precedence-malformed");
    rewrite_event_payload(
        &fixture,
        "json_set(json_set(payload_json, '$.forged', 1), '$.version', 7)",
    );
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                if matches!(cause, ClassificationEventError::Malformed)
        ),
        "B6.X.MALFORMED_BEFORE_DISAGREES: {refusal:?}"
    );
}

/// B6.K.BOTH_ENDS_HELD — the pass reaches the FIRST and the LAST row of its own ordering.
///
/// The build report claimed this was covered because two different bars each asserted a literal
/// key. That is not the claim: one row asserted twice says nothing about whether the walk stops
/// early. This tampers each end in turn, selecting the ends by the pass's own
/// `ORDER BY workspace_id, digest_value` rather than by insertion order, and requires the refusal to
/// name that end.
#[test]
fn the_relation_reaches_both_ends_of_its_own_ordering() {
    for end in ["first", "last"] {
        let (fixture, older) = classified(&format!("b6-both-ends-{end}"));
        let newer = classify_mission(
            &fixture,
            fixture.mission_id,
            2,
            ImpactLevel::High,
            ClassificationSupersedes::Record(older.digest().value.clone()),
        );
        // The owner's own order, read back from the store rather than assumed.
        let ordered: Vec<String> = {
            let connection = fixture.store.connect();
            let mut statement = connection
                .prepare(
                    "SELECT digest_value FROM mission_impact_classifications
                      ORDER BY workspace_id, digest_value",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(ordered.len(), 2, "B6.K: two rows to have two ends");
        assert!(
            ordered.contains(&older.digest().value) && ordered.contains(&newer.digest().value),
            "B6.K: the ordering must hold both classifications"
        );
        let target = if end == "first" {
            ordered.first().unwrap().clone()
        } else {
            ordered.last().unwrap().clone()
        };
        fixture
            .store
            .connect()
            .execute_batch(&format!(
                "DELETE FROM events WHERE kind = 'mission.impact_classified'
                   AND json_extract(payload_json, '$.digest.value') = '{target}';"
            ))
            .expect("B6.PLANT.END_EVENT_DELETED");
        recohere_chain_head(&fixture.store.connect());
        let refusal = refusal(&fixture);
        assert!(
            matches!(
                &refusal,
                SupervisorError::MissionImpactClassificationStateMismatch { digest, cause, .. }
                    if digest == &target
                        && matches!(cause, ClassificationStoreError::EventAbsent)
            ),
            "B6.K.BOTH_ENDS_HELD[{end}]: the walk did not reach {target}: {refusal:?}"
        );
    }
}

/// PLANT BOUNDARY: legacy setup. Turn the store into the shape migration 0036 leaves behind for a
/// store that held Events before the chain existed -- every Event `unchained`, no link digests, and
/// a chain that begins at the migration boundary.
fn make_pre_boundary(fixture: &Fixture) {
    fixture
        .store
        .connect()
        .execute_batch(
            "UPDATE events SET predecessor_kind = 'unchained', predecessor_digest = NULL,
                               link_digest = NULL;
             UPDATE event_chain_state
                SET genesis_kind = 'migrated',
                    migrated_through_sequence = (SELECT max(sequence) FROM events),
                    head_digest = NULL,
                    head_sequence = (SELECT max(sequence) FROM events)
              WHERE id = 1;",
        )
        .expect("B6.PLANT.PRE_BOUNDARY");
}

/// B6.P.PRE_BOUNDARY_VALID — a migrated store whose Events the chain makes no byte claim about
/// still opens Ready when they are coherent.
#[test]
fn a_pre_boundary_unchained_classification_event_is_accepted_when_coherent() {
    let (fixture, _) = classified("b6-preboundary-valid");
    make_pre_boundary(&fixture);
    reopen(&fixture).expect("B6.P.PRE_BOUNDARY_VALID: a coherent migrated store reopens Ready");
}

/// B6.P.PRE_BOUNDARY_INVALID — and an incoherent one is refused by name.
///
/// This is the state the chain structurally cannot judge: the Event is `unchained`, so no link
/// digest attests to its bytes. If the owner relation did not run, nothing would.
#[test]
fn a_pre_boundary_unchained_classification_event_is_refused_when_incoherent() {
    let (fixture, _) = classified("b6-preboundary-invalid");
    make_pre_boundary(&fixture);
    rewrite_event_payload(&fixture, "json_set(payload_json, '$.version', 7)");
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationEventMismatch { cause, .. }
                if matches!(cause, ClassificationEventError::Disagrees {
                    field: ClassificationEventField::Version
                })
        ),
        "B6.P.PRE_BOUNDARY_INVALID: {refusal:?}"
    );
}

/// B6.O.BEFORE_CHAIN — the classification fault wins over a chain fault the same tamper causes.
///
/// **This is not B6.O.BEFORE_MANIFEST**, which is still unbuilt; see the report. It witnesses the
/// half of the ordering claim reachable without a completed Mission, and it is the half that has
/// actually bitten this lane: `a_superseded_middle_row_still_needs_its_own_event` deliberately
/// recoheres the head so the chain stays quiet, and if the owner pass ran after the chain instead,
/// that bar would pass for the wrong reason and the owner mutant would survive it.
///
/// Here the head is left UNCOHERED, so the store holds a classification with no Event *and* a chain
/// whose walk no longer ends where its head says. The owner must be the one that speaks.
#[test]
fn the_classification_fault_is_reached_before_the_chain_fault() {
    let (fixture, classification) = classified("b6-before-chain");
    fixture
        .store
        .connect()
        .execute(
            "DELETE FROM events WHERE kind = 'mission.impact_classified'",
            [],
        )
        .expect("B6.PLANT.EVENT_DELETED_NO_RECOHERE");
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch { digest, cause, .. }
                if digest == &classification.digest().value
                    && matches!(cause, ClassificationStoreError::EventAbsent)
        ),
        "B6.O.BEFORE_CHAIN: the chain spoke first, so every owner bar above could be a chain bar: \
         {refusal:?}"
    );
}

/// A store carried all the way to a prepared Outcome manifest, entirely through the public path:
/// repository, three Artifacts, two evaluations, a Ruling, three classifications, a Run preflight,
/// measured spend, an owner-written completion, then `prepare_for_mission`.
///
/// Lifted from `tests/outcome_closure.rs`'s own sequence. Returns the fixture, the OLDEST
/// classification (version 1, superseded twice and therefore still owed an Event), and the manifest
/// digest.
fn prepared_manifest(label: &str) -> (Fixture, MissionImpactClassification, String) {
    let fixture = admitted_mission(label);
    bind_repository(&fixture);
    let delivered = record_artifact(&fixture, b"# Delivered Artifact D\n");
    let supporting = record_artifact(&fixture, b"# Support-only Artifact S\n");
    let decoy = record_artifact(&fixture, b"# Uncited decoy Artifact\n");
    let support_evaluation = evaluate_artifact(&fixture, &supporting.digest().value);
    let _decoy_evaluation = evaluate_artifact(&fixture, &decoy.digest().value);
    let (_proposal, ruling) = proposal_and_ruling(&fixture);
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
    let (_run_id, _run_seq) = run_preflight(&fixture);
    let coverage_to = latest_event_seq(&fixture.store, fixture.workspace_id);
    let (spend, spend_record) =
        record_spend(&fixture, fixture.mission_id, coverage_to, measured_totals());
    let boundary = latest_event_seq(&fixture.store, fixture.workspace_id);
    let support = support_in_canonical_order(&delivered, &supporting, &support_evaluation, &ruling);
    let delivered_ref = digest_bytes(&delivered.digest().value);
    let completion = completion_record(
        &fixture,
        fixture.mission_id,
        boundary,
        support,
        vec![SupportRef::Artifact(delivered_ref)],
        vec![delivered_ref],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: digest_bytes(&spend_record.digest().value),
        },
        true,
    );
    let _completion_digest = record_completion_for(&fixture, &completion);
    let _later = classify_mission(
        &fixture,
        fixture.mission_id,
        3,
        ImpactLevel::Ordinary,
        ClassificationSupersedes::Record(high.digest().value.clone()),
    );
    let candidate = fixture
        .outcomes
        .prepare_for_mission(fixture.mission_id, AppendixRequest::PolicyDefault)
        .expect("B6.MANIFEST.PREPARED");
    (fixture, older, candidate.manifest_digest)
}

/// PLANT BOUNDARY: corruption only. Manifests are immutable by trigger, so the guard is dropped and
/// recreated from `sqlite_master`'s own stored text -- byte-identical by construction, so the schema
/// signature is untouched and the manifest pass, not the schema check, is what objects.
fn corrupt_the_manifest(fixture: &Fixture) {
    let connection = fixture.store.connect();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
              WHERE type = 'trigger' AND name = 'outcome_manifests_no_update'",
            [],
            |row| row.get(0),
        )
        .expect("B6.PLANT.MANIFEST_GUARD_PRESENT");
    connection
        .execute_batch(
            "DROP TRIGGER outcome_manifests_no_update;
             UPDATE outcome_manifests SET renderer_version = 'forged.renderer.v0';",
        )
        .expect("B6.PLANT.MANIFEST_CORRUPTED");
    connection
        .execute_batch(&trigger)
        .expect("B6.PLANT.MANIFEST_GUARD_RESTORED");
}

/// The control for the ordering bar: the manifest corruption REALLY refuses on its own.
///
/// Without this, the joint-corrupt store below could name the classification fault simply because
/// the manifest was never broken, and the ordering claim would be vacuous.
#[test]
fn a_corrupted_manifest_alone_is_refused_by_the_manifest_pass() {
    let (fixture, _older, _digest) = prepared_manifest("b6-manifest-alone");
    corrupt_the_manifest(&fixture);
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::OutcomeManifestStateMismatch { .. }
        ),
        "B6.O.MANIFEST_ALONE: the manifest corruption must refuse on its own: {refusal:?}"
    );
}

/// B6.O.BEFORE_MANIFEST — both broken, and the EARLIER pass speaks.
///
/// The Outcome manifest cites a classification and re-checks the high-impact bit against it, so a
/// classification that no Event accounts for must be refused before anything relies on it. This is
/// the ordering `verify_classification_state` before `verify_manifest_state` exists to give, and
/// the control above is what makes the result mean something.
#[test]
fn the_classification_fault_is_reached_before_the_manifest_fault() {
    let (fixture, older, _digest) = prepared_manifest("b6-before-manifest");
    corrupt_the_manifest(&fixture);
    fixture
        .store
        .connect()
        .execute_batch(&format!(
            "DELETE FROM events WHERE kind = 'mission.impact_classified'
               AND json_extract(payload_json, '$.digest.value') = '{}';",
            older.digest().value
        ))
        .expect("B6.PLANT.CLASSIFICATION_EVENT_DELETED");
    recohere_chain_head(&fixture.store.connect());
    let refusal = refusal(&fixture);
    assert!(
        matches!(
            &refusal,
            SupervisorError::MissionImpactClassificationStateMismatch { digest, cause, .. }
                if digest == &older.digest().value
                    && matches!(cause, ClassificationStoreError::EventAbsent)
        ),
        "B6.O.BEFORE_MANIFEST: the manifest pass spoke first: {refusal:?}"
    );
}
