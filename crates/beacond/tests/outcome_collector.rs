//! Outcome Document slice 2-B: the verified collector.
//!
//! The tests are §7 slice 2's names. The four the skeleton could write against `main` alone are
//! unchanged; the rest wait on no reader any more -- measured spend, the Mission impact
//! classification and `MissionCompletionV1` are all on `main`, so a test that collects one now
//! collects a real record through its owning module rather than asserting a typed hole.
//!
//! The Artifact entity store is the one seam left, and it is still asserted as a refusal.
//!
//! The fixture chain down to `mission_draft` is `tests/outcome_stores.rs`'s, which is
//! `tests/mission.rs`'s: a completion has a foreign key to the Mission's admission roster, so a
//! record cannot exist without the admission chain that produces one. Copied rather than shared
//! because the suites must be able to disagree about what a Mission is -- a shared fixture is how
//! two tests agree about something wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    ArtifactContext, ArtifactKind, ArtifactService, AutonomyProfile, BeaconCapability,
    ClassificationActor, ClassificationBasis, ClassificationSupersedes, CollectedDigest,
    CollectedSupport, CollectedUncounted, CollectionRequest, Collector, CollectorRefusal,
    CompletionActor, CompletionDisposition, ConditionOutcome, ConditionResultRecord, Conflicts,
    DeclaredCheck, EvaluationDraft, EvaluationService, EvaluationSubjectDraft,
    EvaluationSupersedes, Evaluator, Finding, FindingSeverity, ImpactLevel,
    LocalModelActivationService, LocalModelEndpoint, MeasuredSpendService, MeasuredSpendV1,
    MissionCompletionV1, MissionDraft, MissionImpactClassificationV1, MissionImpactService,
    MissionMemberDraft, MissionService, MissionView, OUTCOME_RECEIPT_CHECKSUM_LABEL,
    OperatorAccountService, OperatorSessionPhase, OutcomeState, PolicyRevisionRef,
    ProjectCharterDraft, ProjectDraft, ProjectRepositoryService, ProjectService, ProjectView,
    ProposalService, ProviderFeature, RepositoryDraft, RepositorySourceKind, ResidentConfiguration,
    ResidentConfigurationService, ResidentLifecycleService, RubricDeclaration, RulingDecision,
    RulingService, RulingSupersedes, SecretString, SourceKind, SpendCompleteness, SpendLine,
    SpendMeasurement, SpendOrigin, SpendReference, SpendSupersedes, SpendTotals,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    SupportRef, TeamDraft, TeamMemberDraft, TeamService, TeamView, UncertaintyKind, UncountedEntry,
    UncountedKind, UnfinishedKind, Verdict, declared_conditions_digest, positionable_sources,
    record_completion,
};
use data_encoding::HEXLOWER;
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

struct TestStore {
    path: PathBuf,
    workspace_id: Uuid,
}

impl TestStore {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "beacond-outcome-collector-{label}-{}.sqlite3",
            Uuid::new_v4()
        ));
        let report = Supervisor::bootstrap(StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();
        Self {
            path,
            workspace_id: report.workspace.id,
        }
    }

    fn connection(&self) -> Connection {
        let connection = Connection::open(&self.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn enrolled_accounts(store: &TestStore) -> Arc<OperatorAccountService> {
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
    accounts
}

fn tags_response() -> String {
    json!({
        "models": [{
            "name": MODEL_NAME,
            "model": MODEL_NAME,
            "modified_at": "2026-08-14T14:38:20.13373148-04:00",
            "size": 17_741_872_154_u64,
            "digest": MODEL_DIGEST,
            "details": {
                "format": "gguf",
                "family": "qwen35",
                "families": ["qwen35"],
                "parameter_size": "27.3B",
                "quantization_level": "Q4_K_M"
            }
        }]
    })
    .to_string()
}

fn serve_ollama_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..count {
            for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(
                    request.starts_with(expected),
                    "unexpected request: {request}"
                );
                let body = if expected.starts_with("POST") {
                    json!({ "capabilities": ["completion", "tools"] }).to_string()
                } else {
                    tags_response()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    (address, handle)
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

struct PreparedTeam {
    project: ProjectView,
    team: TeamView,
    configurations: Vec<ResidentConfiguration>,
}

fn prepare_team(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
    member_count: usize,
) -> PreparedTeam {
    let project = ProjectService::new(Arc::clone(accounts), store.workspace_id)
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
    let roles = [
        StarterRole::Lead,
        StarterRole::Builder,
        StarterRole::Reviewer,
    ];
    let residents = StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id)
        .accept(
            1,
            roles[..member_count]
                .iter()
                .copied()
                .map(|role| StarterResidentDraft {
                    role,
                    display_name: None,
                })
                .collect(),
        )
        .unwrap()
        .residents;
    let lifecycle = ResidentLifecycleService::new(Arc::clone(accounts), store.workspace_id);
    for resident in &residents {
        lifecycle.activate(resident.id, None).unwrap();
    }
    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(accounts),
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let activation = models
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();
    let configurations = residents
        .iter()
        .map(|resident| {
            ResidentConfigurationService::new(Arc::clone(accounts), store.workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    vec![BeaconCapability::WorkCreateMission],
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let team = TeamService::new(Arc::clone(accounts), store.workspace_id)
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
    PreparedTeam {
        project,
        team,
        configurations,
    }
}

fn mission_draft(prepared: &PreparedTeam) -> MissionDraft {
    MissionDraft {
        project_id: prepared.project.id,
        expected_charter_revision: prepared.project.charter.revision,
        team_id: prepared.team.id,
        desired_outcome: "  Ship one reviewed Beacon feature.\nPreserve its evidence.  ".to_owned(),
        success_conditions: vec!["Relevant gates pass".to_owned()],
        constraints: vec!["Keep the change reversible".to_owned()],
        prohibitions: vec!["Do not deploy".to_owned()],
        members: prepared
            .configurations
            .iter()
            .map(|configuration| MissionMemberDraft {
                resident_id: configuration.resident_id,
                expected_configuration_id: configuration.id,
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------------------------
// An independent encoder for the Proposal grammar
//
// Copied from `tests/evaluation.rs`, which built it from the published table rather than from
// `proposal.rs`'s encoder. `ProposalV1` has no public `canonical_bytes`, so a suite that needs a
// stored Proposal has to spell the bytes itself -- and a fixture that called Beacon's own encoder
// would prove only that the encoder agrees with itself.
// ---------------------------------------------------------------------------------------------

const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const MAGIC: &[u8] = b"beacon.proposal.v1\0";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let output = digest.finalize();
    let mut value = [0_u8; 32];
    value.copy_from_slice(&output[..]);
    value
}
#[derive(Debug, Clone, PartialEq, Eq)]
enum Context {
    Declared {
        entries: Vec<(String, [u8; 32])>,
        /// `None` means "the digest of the encoded entry list", which is what a truthful record
        /// carries. `Some` is how a test states a manifest that does not match.
        manifest: Option<[u8; 32]>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Constraint {
    NotDeclared,
}

/// One canonical record, field by field, so a test can change exactly one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    magic: Vec<u8>,
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    bootstrap_tag: u8,
    bootstrap_reason: String,
    charter_revision: u32,
    policy_revision: u32,
    repository_revision: u32,
    base_revision: String,
    summary: String,
    context: Context,
    targets: Vec<String>,
    mutations: Vec<(u8, String)>,
    check_runs: Vec<String>,
    checks: Vec<(String, Vec<String>, String)>,
    requirements: Vec<Constraint>,
    bounds: [u64; 6],
    budget_tag: u8,
    rollback_tag: u8,
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn push_count(out: &mut Vec<u8>, count: usize) {
    out.extend_from_slice(&(count as u32).to_be_bytes());
}

fn encoded_entries(entries: &[(String, [u8; 32])]) -> Vec<u8> {
    let mut out = Vec::new();
    push_count(&mut out, entries.len());
    for (path, digest) in entries {
        push_string(&mut out, path);
        out.extend_from_slice(digest);
    }
    out
}

impl Record {
    fn bytes(&self) -> Vec<u8> {
        let mut out = self.magic.clone();
        for id in [
            self.workspace_id,
            self.project_id,
            self.mission_id,
            self.resident_id,
            self.configuration_id,
        ] {
            out.extend_from_slice(id.as_bytes());
        }
        out.push(self.bootstrap_tag);
        push_string(&mut out, &self.bootstrap_reason);

        for revision in [
            self.charter_revision,
            self.policy_revision,
            self.repository_revision,
        ] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, &self.base_revision);

        push_string(&mut out, &self.summary);
        match &self.context {
            Context::Declared { entries, manifest } => {
                out.push(0x02);
                let encoded = encoded_entries(entries);
                out.extend_from_slice(&encoded);
                out.extend_from_slice(&manifest.unwrap_or_else(|| sha256(&encoded)));
            }
        }
        push_count(&mut out, self.targets.len());
        for target in &self.targets {
            push_string(&mut out, target);
        }
        push_count(&mut out, self.mutations.len());
        for (tag, target) in &self.mutations {
            out.push(*tag);
            push_string(&mut out, target);
        }
        push_count(&mut out, self.check_runs.len());
        for name in &self.check_runs {
            push_string(&mut out, name);
        }
        push_count(&mut out, self.checks.len());
        for (name, argv, working_directory) in &self.checks {
            push_string(&mut out, name);
            push_count(&mut out, argv.len());
            for argument in argv {
                push_string(&mut out, argument);
            }
            push_string(&mut out, working_directory);
        }

        for constraint in &self.requirements {
            match constraint {
                Constraint::NotDeclared => out.push(0x01),
            }
        }
        for bound in self.bounds {
            out.extend_from_slice(&bound.to_be_bytes());
        }
        out.push(self.budget_tag);
        out.push(self.rollback_tag);
        out
    }
}

// ---------------------------------------------------------------------------------------------
// The collector's fixture: a Workspace with the chain a completion needs
// ---------------------------------------------------------------------------------------------

struct Fixture {
    workspace_id: Uuid,
    projects: ProjectService,
    store: TestStore,
}

fn enrolled(label: &str) -> Fixture {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    Fixture {
        workspace_id: store.workspace_id,
        projects: ProjectService::new(Arc::clone(&accounts), store.workspace_id),
        store,
    }
}

impl Fixture {
    fn project(&self) -> Uuid {
        let project_id = self
            .projects
            .create(ProjectDraft {
                name: "Collector".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "A Project that exists so a Run can point at it.".to_owned(),
                    desired_outcomes: vec!["The collector reads bytes".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not deploy publicly".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap()
            .id;
        // A Run references a bound repository revision. The write path has its own suite; this one
        // is about the read, so the row is seeded rather than produced by Git.
        self.store
            .connection()
            .execute(
                "INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind, source,
                     base_revision, declared_checks_json, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 1, 'local_path', '/private/beacon/repo',
                           '0123456789012345678901234567890123456789',
                           '[{\"name\":\"unit\"}]', 'operator', 0)",
                params![self.workspace_id.to_string(), project_id.to_string()],
            )
            .unwrap();
        project_id
    }

    /// One Run and its Receipt, written straight to the store. `outcome_column` is the denormalized
    /// projection beside `receipt_json`; the tests vary it deliberately.
    fn receipt(&self, project_id: Uuid, body: &str, outcome_column: &str) -> Uuid {
        let run_id = Uuid::new_v4();
        let connection = self.store.connection();
        connection
            .execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'verified', 0, 10, 11)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, ?3, ?4, 11)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    outcome_column,
                    body,
                ],
            )
            .unwrap();
        run_id
    }
}

/// A Receipt body whose own `outcome` field is `verified`, so a disagreeing column is visible.
fn receipt_body(workspace_id: Uuid, project_id: Uuid, run_id: Uuid) -> String {
    format!(
        r#"{{"version":1,"runId":"{run_id}","workspaceId":"{workspace_id}","projectId":"{project_id}","outcome":"verified","note":"the bytes are the record"}}"#
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The Workspace's newest Event sequence -- the honest end of any coverage window, and the number a
/// boundary is measured against.
fn newest_event(store: &TestStore) -> u64 {
    store
        .connection()
        .query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            params![store.workspace_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).unwrap())
        .unwrap()
}

/// The Mission's admission Event sequence -- the floor a completion's boundary must be at or above.
fn admitted_sequence(store: &TestStore, mission_id: Uuid) -> u64 {
    store
        .connection()
        .query_row(
            "SELECT min(sequence) FROM events WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?1",
            [mission_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).unwrap())
        .unwrap()
}

/// A Workspace with one admitted Mission and a published Project Policy revision.
struct Admitted {
    store: TestStore,
    accounts: Arc<OperatorAccountService>,
    mission: MissionView,
    project: ProjectView,
}

fn admitted(label: &str, conditions: Vec<String>) -> Admitted {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 2);
    let mut draft = mission_draft(&prepared);
    draft.success_conditions = conditions;
    let mission = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .admit(draft)
        .expect("the Mission is admitted");
    Admitted {
        project: prepared.project,
        mission,
        accounts,
        store,
    }
}

impl Admitted {
    /// One Artifact scoped to this Mission, authored by its first admitted member.
    fn record_mission_artifact(&self, content: &[u8]) -> beacond::VerifiedArtifactRecord {
        let member = &self.mission.members[0];
        ArtifactService::new(Arc::clone(&self.accounts), self.store.workspace_id)
            .record(
                ArtifactKind::OutcomeDocument,
                content,
                ArtifactContext::Mission {
                    project_id: self.mission.project_id,
                    mission_id: self.mission.id,
                },
                member.resident_id,
                member.admission_configuration_id,
                None,
            )
            .expect("the Artifact records")
    }

    fn connection(&self) -> Connection {
        self.store.connection()
    }

    /// A published Project Policy revision, seeded directly: the classification's foreign key needs
    /// a parent and the publish path has its own suite.
    fn policy_revision(&self) -> u32 {
        self.connection()
            .execute(
                "INSERT INTO project_policy_revisions (
                     workspace_id, project_id, revision, charter_revision,
                     required_capabilities_json, availability_rule, track_record_requirements_json,
                     conflict_rule, ambiguity_rule, authored_by, authored_at_ms
                 ) VALUES (?1, ?2, 1, ?3, '[\"work.coordinate_mission\"]',
                           'current_team_eligibility', '[{\"evidenceKind\":\"completed_mission\"}]',
                           'require_no_declared_conflict', 'require_operator_decision',
                           'operator', 0)",
                params![
                    self.store.workspace_id.to_string(),
                    self.project.id.to_string(),
                    self.project.charter.revision
                ],
            )
            .unwrap();
        1
    }

    /// One measured spend record, through its own write path so the Event that positions it exists.
    fn measure_spend(&self, measurement: SpendMeasurement) -> MeasuredSpendV1 {
        let record = MeasuredSpendV1 {
            workspace_id: self.store.workspace_id,
            mission_id: self.mission.id,
            spend_id: Uuid::new_v4(),
            version: 1,
            origin: SpendOrigin::DaemonLedger,
            measured_at_ms: 1_788_400_000_000,
            coverage_from_event_seq: 1,
            coverage_to_event_seq: newest_event(&self.store),
            measurement,
            supersedes: SpendSupersedes::None,
        };
        MeasuredSpendService::new(Arc::clone(&self.accounts), self.store.workspace_id)
            .record(&record.canonical_bytes().unwrap())
            .expect("the spend record is written");
        record
    }

    /// One Mission impact classification, through its own write path.
    fn classify(&self, impact: ImpactLevel) -> MissionImpactClassificationV1 {
        let policy_revision = self.policy_revision();
        let record = MissionImpactClassificationV1 {
            workspace_id: self.store.workspace_id,
            mission_id: self.mission.id,
            classification_id: Uuid::new_v4(),
            version: 1,
            impact,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: self.project.id,
                policy_revision,
                charter_revision: self.project.charter.revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes: ClassificationSupersedes::None,
        };
        MissionImpactService::new(Arc::clone(&self.accounts), self.store.workspace_id)
            .classify(&record.canonical_bytes().unwrap())
            .expect("the classification is written");
        record
    }

    /// A completion for this Mission, at `boundary`, with one outcome per declared condition.
    fn completion(
        &self,
        boundary: u64,
        outcomes: Vec<ConditionOutcome>,
        spend: SpendReference,
    ) -> MissionCompletionV1 {
        let member = &self.mission.members[0];
        MissionCompletionV1 {
            workspace_id: self.store.workspace_id,
            project_id: self.mission.project_id,
            mission_id: self.mission.id,
            completion_id: Uuid::new_v4(),
            actor: CompletionActor::Operator,
            accountable_resident_id: member.resident_id,
            accountable_resident_configuration_id: member.admission_configuration_id,
            boundary_event_seq: boundary,
            disposition: CompletionDisposition::Concluded,
            declared_conditions_sha256: declared_conditions_digest(
                &self.mission.success_conditions,
            ),
            condition_results: outcomes
                .into_iter()
                .enumerate()
                .map(|(ordinal, outcome)| ConditionResultRecord {
                    ordinal: u32::try_from(ordinal).unwrap(),
                    outcome,
                    support: vec![],
                    note: None,
                })
                .collect(),
            delivered_artifacts: vec![],
            unfinished_work: vec![],
            uncertainty: vec![],
            spend,
            supersedes: beacond::CompletionSupersedes::None,
            summary: "One reviewed feature shipped.".to_owned(),
            completed_at_ms: 9_000,
        }
    }

    /// A bound repository revision for this Project, through the bind path rather than a seeded
    /// row: a Proposal's bindings are checked against what that path actually writes, and a
    /// hand-written row is how a fixture agrees with itself about a shape nothing produces.
    fn repository_revision(&self) {
        ProjectRepositoryService::new(Arc::clone(&self.accounts), self.store.workspace_id)
            .bind(
                self.project.id,
                self.project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source: "/home/operator/projects/sample-tracker".to_owned(),
                    base_revision: BASE_REVISION.to_owned(),
                    declared_checks: vec![DeclaredCheck {
                        name: "cargo-test".to_owned(),
                        argv: vec![
                            "cargo".to_owned(),
                            "test".to_owned(),
                            "--workspace".to_owned(),
                        ],
                        working_directory: ".".to_owned(),
                    }],
                },
            )
            .expect("the repository binds");
    }

    /// One Proposal record, field by field, bound to this Mission's own chain.
    fn proposal_record(&self) -> Record {
        let member = &self.mission.members[0];
        Record {
            magic: MAGIC.to_vec(),
            workspace_id: self.store.workspace_id,
            project_id: self.project.id,
            mission_id: self.mission.id,
            resident_id: member.resident_id,
            configuration_id: member.admission_configuration_id,
            bootstrap_tag: 0x01,
            bootstrap_reason: "The Owner selected this Resident to request this one Proposal."
                .to_owned(),
            charter_revision: self.project.charter.revision,
            policy_revision: 1,
            repository_revision: 1,
            base_revision: BASE_REVISION.to_owned(),
            summary: "Add a regression test for the invariant checker.".to_owned(),
            context: Context::Declared {
                entries: vec![("src/lib.rs".to_owned(), [0x66; 32])],
                manifest: None,
            },
            targets: vec!["src/lib.rs".to_owned(), "tests/invariants.rs".to_owned()],
            mutations: vec![
                (0x02, "src/lib.rs".to_owned()),
                (0x01, "tests/invariants.rs".to_owned()),
            ],
            check_runs: vec!["cargo-test".to_owned()],
            checks: vec![(
                "cargo-test".to_owned(),
                vec![
                    "cargo".to_owned(),
                    "test".to_owned(),
                    "--workspace".to_owned(),
                ],
                ".".to_owned(),
            )],
            requirements: vec![Constraint::NotDeclared; 7],
            bounds: [300_000, 240, 2 << 30, 256 << 20, 1 << 20, 0],
            budget_tag: 0x01,
            rollback_tag: 0x01,
        }
    }

    /// A Run and its Receipt, seeded directly -- and deliberately with **no run Event**, which is
    /// what makes it unpositionable against a cut.
    fn seeded_receipt(&self) -> Uuid {
        let run_id = Uuid::new_v4();
        let connection = self.connection();
        connection
            .execute(
                "INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind, source,
                     base_revision, declared_checks_json, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, ?3, 'local_path', '/private/beacon/repo',
                           '0123456789012345678901234567890123456789',
                           '[{\"name\":\"unit\"}]', 'operator', 0)",
                params![
                    self.store.workspace_id.to_string(),
                    self.project.id.to_string(),
                    self.project.charter.revision
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'check_preflight', 1, ?4, 'verified', 0, 10, 11)",
                params![
                    self.store.workspace_id.to_string(),
                    run_id.to_string(),
                    self.project.id.to_string(),
                    self.project.charter.revision
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, 'verified', ?3, 11)",
                params![
                    self.store.workspace_id.to_string(),
                    run_id.to_string(),
                    receipt_body(self.store.workspace_id, self.project.id, run_id),
                ],
            )
            .unwrap();
        run_id
    }

    fn record(&self, record: &MissionCompletionV1) -> String {
        let mut connection = self.connection();
        record_completion(&mut connection, record).expect("the completion is written")
    }
}

// ---------------------------------------------------------------------------------------------

/// §7 slice 2. The bound is Outcome's own, and "before a partial manifest" is a property of the
/// collector's ordering rather than of its arithmetic.
///
/// Order is observable without measuring allocation: the store is **empty**, so every citation would
/// refuse as `Absent` if a read happened first. A `Bound` refusal can only mean nothing was read.
#[test]
fn collector_bound_is_owned_by_outcome_and_refuses_before_partial_manifest() {
    let fixture = enrolled("bound-first");
    let connection = fixture.store.connection();
    let collector = Collector::new(&connection, fixture.workspace_id);

    let request = CollectionRequest {
        receipt_run_uuids: (0..257).map(|_| Uuid::new_v4()).collect(),
        ..CollectionRequest::default()
    };
    match collector
        .collect(&request)
        .expect_err("257 citations refuse")
    {
        CollectorRefusal::Bound(bound) => {
            assert_eq!(bound.bound.as_str(), "citation_count");
            assert_eq!(bound.configured, 256);
            assert_eq!(bound.observed_at_least, 257);
        }
        other => panic!("read before the bound was checked: {other}"),
    }

    // And 256 gets as far as reading, which is what makes the refusal above a statement about
    // ordering rather than about the number.
    let request = CollectionRequest {
        receipt_run_uuids: (0..256).map(|_| Uuid::new_v4()).collect(),
        ..CollectionRequest::default()
    };
    match collector.collect(&request).expect_err("nothing is stored") {
        CollectorRefusal::Absent { kind, .. } => assert_eq!(kind, "receipt"),
        other => panic!("expected the reader to answer, got {other}"),
    }
}

/// §7 slice 2. The checksum covers the exact stored bytes and nothing else.
#[test]
fn collector_receipt_checksum_is_over_exact_stored_receipt_json_bytes() {
    let fixture = enrolled("receipt-checksum");
    let project_id = fixture.project();
    let run_id = Uuid::new_v4();
    let body = receipt_body(fixture.workspace_id, project_id, run_id);
    let stored_run = fixture.receipt(project_id, &body, "verified");

    let connection = fixture.store.connection();
    let collector = Collector::new(&connection, fixture.workspace_id);
    let collected = collector
        .collect(&CollectionRequest {
            receipt_run_uuids: vec![stored_run],
            ..CollectionRequest::default()
        })
        .expect("the Receipt is stored");

    assert_eq!(collected.receipts.len(), 1);
    let receipt = &collected.receipts[0];
    assert_eq!(receipt.run_uuid, stored_run);
    assert_eq!(receipt.receipt_version, 1);
    assert_eq!(receipt.workspace_id, fixture.workspace_id);

    // Read the bytes back independently and hash them here, so the assertion does not go through
    // the same code path it is checking.
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT CAST(receipt_json AS BLOB) FROM receipts WHERE workspace_id = ?1 AND run_id = ?2",
            params![fixture.workspace_id.to_string(), stored_run.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt.outcome_checksum, sha256_hex(&bytes));

    // One byte of difference changes the checksum: it covers the bytes, not a summary of them. The
    // store refuses to mutate a Receipt -- `Receipts are immutable` -- so the probe is a second
    // Receipt rather than an edit, which is the store being right rather than the test being coy.
    let nudged = fixture.receipt(
        project_id,
        &body.replace("the bytes are the record", "the bytes are the recorD"),
        "verified",
    );
    let after = collector
        .collect(&CollectionRequest {
            receipt_run_uuids: vec![nudged],
            ..CollectionRequest::default()
        })
        .expect("stored");
    assert_ne!(after.receipts[0].outcome_checksum, receipt.outcome_checksum);
}

/// §7 slice 2, and Kevin's ruling 5: the checksum is Outcome's verification of stored bytes, not a
/// Receipt content digest. ADR 0027 and Receipt v2 define none, and this slice mints none.
#[test]
fn collector_labels_receipt_checksum_as_outcome_verification_not_receipt_identity() {
    assert_eq!(
        OUTCOME_RECEIPT_CHECKSUM_LABEL,
        "beacon.outcome.receipt-checksum.v1"
    );
    // The label names Outcome, not Receipt: a reader cannot mistake it for the Receipt's own
    // identity, because the Receipt does not have one and the token says whose it is.
    assert!(OUTCOME_RECEIPT_CHECKSUM_LABEL.starts_with("beacon.outcome."));
    assert!(!OUTCOME_RECEIPT_CHECKSUM_LABEL.starts_with("beacon.receipt"));

    let fixture = enrolled("receipt-label");
    let project_id = fixture.project();
    let run_id = Uuid::new_v4();
    let stored_run = fixture.receipt(
        project_id,
        &receipt_body(fixture.workspace_id, project_id, run_id),
        "verified",
    );
    let connection = fixture.store.connection();
    let collected = Collector::new(&connection, fixture.workspace_id)
        .collect(&CollectionRequest {
            receipt_run_uuids: vec![stored_run],
            ..CollectionRequest::default()
        })
        .expect("stored");

    // The citation is `(Workspace, run UUID, version)` plus the checksum -- the identity a Receipt
    // actually has. Nothing in it is a digest of the Receipt as a domain object.
    let receipt = &collected.receipts[0];
    assert_eq!(receipt.outcome_checksum.len(), 64);
    assert!(
        receipt
            .outcome_checksum
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    // No Receipt digest column was written or read: the store has none to mint one from.
    let has_digest: i64 = connection
        .query_row(
            "SELECT count(*) FROM pragma_table_info('receipts') WHERE name LIKE '%digest%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_digest, 0, "the Receipt grew a digest column");
}

/// §7 slice 2's centrepiece. A joined projection can never change a rendered claim.
///
/// `receipts.outcome` is a denormalized copy of a value inside `receipt_json`, and nothing in the
/// store reconciles the two. Two Receipts with identical bytes and *different* columns must produce
/// identical citations: the column is allowed to identify a candidate and is never allowed to
/// supply a claim.
#[test]
fn collector_joined_projection_decoys_cannot_change_rendered_claims() {
    let fixture = enrolled("decoy");
    let project_id = fixture.project();

    // Identical bytes, deliberately different projection columns. The body says `verified` in both.
    let run_a = Uuid::new_v4();
    let body = receipt_body(fixture.workspace_id, project_id, run_a);
    let honest = fixture.receipt(project_id, &body, "verified");
    let decoy = fixture.receipt(project_id, &body, "preflight_failed");

    let connection = fixture.store.connection();
    let collected = Collector::new(&connection, fixture.workspace_id)
        .collect(&CollectionRequest {
            receipt_run_uuids: vec![honest, decoy],
            ..CollectionRequest::default()
        })
        .expect("both are stored");

    assert_eq!(collected.receipts.len(), 2);
    assert_eq!(
        collected.receipts[0].outcome_checksum, collected.receipts[1].outcome_checksum,
        "the projection column reached the citation"
    );

    // The decoy really is a decoy: the two rows disagree in the store, so a collector that read the
    // column would have had something different to say.
    let columns: Vec<String> = connection
        .prepare("SELECT outcome FROM receipts WHERE workspace_id = ?1 ORDER BY run_id")
        .unwrap()
        .query_map(params![fixture.workspace_id.to_string()], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut distinct = columns.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        2,
        "the fixture did not create a disagreement"
    );

    // And the honest row's own bytes still say `verified`, so the disagreement is the column's.
    let bytes: Vec<u8> = connection
        .query_row(
            "SELECT CAST(receipt_json AS BLOB) FROM receipts WHERE workspace_id = ?1 AND run_id = ?2",
            params![fixture.workspace_id.to_string(), decoy.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains(r#""outcome":"verified""#));
}

// ---------------------------------------------------------------------------------------------
// §7 slice 2, against the readers that now exist
// ---------------------------------------------------------------------------------------------

/// §7 slice 2. Every source goes to its owning module; the collector reports what that module said
/// and never answers from a row it read itself.
///
/// Three of the skeleton's four seams are gone — spend, classification and completion are real
/// reads here. The Artifact entity store is the one left, and it is still a typed refusal.
#[test]
fn collector_reads_each_source_through_its_owning_verified_reader() {
    let fixture = admitted(
        "readers",
        vec!["Gates pass".to_owned(), "Evidence filed".to_owned()],
    );
    let connection = fixture.connection();
    let collector = Collector::new(&connection, fixture.store.workspace_id);
    let absent = "0".repeat(64);

    // Proposal, Ruling and Evaluation each answer "no such object in this Workspace" through their
    // own reader. The collector reports that answer; it does not invent a reference.
    for (label, request) in [
        (
            "proposal",
            CollectionRequest {
                proposal_digests: vec![absent.clone()],
                ..CollectionRequest::default()
            },
        ),
        (
            "ruling",
            CollectionRequest {
                ruling_digests: vec![absent.clone()],
                ..CollectionRequest::default()
            },
        ),
        (
            "evaluation",
            CollectionRequest {
                evaluation_digests: vec![absent.clone()],
                ..CollectionRequest::default()
            },
        ),
        (
            "completion",
            CollectionRequest {
                completion_digest: Some(absent.clone()),
                ..CollectionRequest::default()
            },
        ),
        (
            "classification",
            CollectionRequest {
                classification_digest: Some(absent.clone()),
                ..CollectionRequest::default()
            },
        ),
    ] {
        match collector.collect(&request).expect_err("nothing is stored") {
            CollectorRefusal::Absent { kind, id } => {
                assert_eq!(kind, label);
                assert_eq!(id, absent);
            }
            other => panic!("{label}: expected the owning reader to answer, got {other}"),
        }
    }

    // ...and the positive half: a real completion, a real classification and a real spend record,
    // each of them collected through the module that owns it.
    let spend = fixture.measure_spend(SpendMeasurement::Measured(SpendTotals {
        counted_units: 412,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 123_456,
            minor_units_exponent: 2,
        }],
    }));
    let classification = fixture.classify(ImpactLevel::High);
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met, ConditionOutcome::NotMet],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let completion_digest = fixture.record(&completion);

    let connection = fixture.connection();
    let collector = Collector::new(&connection, fixture.store.workspace_id);
    let collected = collector
        .collect(&CollectionRequest {
            completion_digest: Some(completion_digest.clone()),
            classification_digest: Some(classification.digest().unwrap().value),
            ..CollectionRequest::default()
        })
        .expect("every source is verified");

    let read = collected.completion.expect("the completion is collected");
    assert_eq!(read.digest_value, completion_digest);
    assert_eq!(read.completion_id, completion.completion_id);
    assert_eq!(read.boundary_event_seq, boundary);
    assert!(
        collected
            .classification
            .expect("the classification is collected")
            .policy_high_impact,
        "R13.7: the high-impact bit comes from the record's own recheck, not from a copy"
    );
    let collected_spend = collected
        .spend
        .expect("the completion cited a spend record");
    assert_eq!(collected_spend.spend_id, Some(spend.spend_id));
    assert_eq!(collected_spend.measurement, "measured");

    // The last seam is gone: the Artifact store landed (#63) and this reads through it.
    let artifact = fixture.record_mission_artifact(b"# Outcome\n\nThe delivered document.\n");
    let collected_artifact = collector
        .artifact(&artifact.digest().value, fixture.mission.id)
        .expect("the Artifact reads through its owning module");
    assert_eq!(collected_artifact.digest_value, artifact.digest().value);
    assert_eq!(collected_artifact.workspace_id, fixture.store.workspace_id);
}

#[test]
fn verified_completion_refuses_condition_rows_belonging_to_another_mission() {
    let fixture = admitted(
        "completion-condition-cross-mission",
        vec!["Gates pass".to_owned()],
    );
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let digest = fixture.record(&completion);

    let other_mission = Uuid::new_v4();
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO missions SELECT ?1, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json, prohibitions_json,
                    lifecycle, admitted_by, admitted_at_ms
               FROM missions WHERE id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_admission_members
               SELECT workspace_id, ?1, team_id, position, resident_id, configuration_id
               FROM mission_admission_members WHERE mission_id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_success_conditions
               SELECT workspace_id, ?1, ordinal, text
               FROM mission_success_conditions WHERE mission_id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER mission_completion_conditions_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE mission_completion_conditions SET mission_id = ?1
             WHERE completion_digest = ?2",
            params![other_mission.to_string(), digest],
        )
        .unwrap();

    let result =
        beacond::verified_completion_within(&connection, fixture.store.workspace_id, &digest);
    assert!(
        result.is_err(),
        "completion reader accepted condition rows owned by Mission {other_mission}: {result:?}",
    );
}

#[test]
fn completion_refuses_a_project_that_does_not_own_its_mission() {
    let fixture = admitted("completion-cross-project", vec!["Gates pass".to_owned()]);
    let other_project =
        ProjectService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .create(ProjectDraft {
                name: "Unrelated Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Stay unrelated to the Mission.".to_owned(),
                    desired_outcomes: vec!["No cross-Project completion".to_owned()],
                    constraints: vec!["Keep scope structural".to_owned()],
                    prohibitions: vec!["Do not borrow a Mission".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
    let boundary = newest_event(&fixture.store);
    let mut completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    completion.project_id = other_project.id;
    let mut connection = fixture.connection();
    let result = record_completion(&mut connection, &completion);
    assert!(
        result.is_err(),
        "Mission {} was completed under unrelated Project {}: {result:?}",
        completion.mission_id,
        other_project.id,
    );
}

#[test]
fn completion_supersession_cannot_name_a_record_from_another_workspace() {
    let other = admitted("completion-super-other", vec!["Gates pass".to_owned()]);
    let other_completion = other.completion(
        newest_event(&other.store),
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger".to_owned()),
    );
    let foreign_predecessor = sha256_bytes(
        &other_completion
            .canonical_bytes()
            .expect("canonical completion"),
    );
    other.record(&other_completion);

    let local = admitted("completion-super-local", vec!["Gates pass".to_owned()]);
    let mut successor = local.completion(
        newest_event(&local.store),
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger".to_owned()),
    );
    successor.supersedes = beacond::CompletionSupersedes::Record(foreign_predecessor);
    let mut connection = local.connection();
    let result = record_completion(&mut connection, &successor);

    assert!(
        result.is_err(),
        "a completion accepted another Workspace's predecessor: {result:?}"
    );
    // sol1's probe asserts only that it is refused, and a refusal under any name would satisfy it.
    // A mutant that refuses this record for an unrelated reason -- a fabricated condition-count
    // mismatch -- survives that assertion, so the name is asserted too. Added beneath sol1's own
    // assertion rather than replacing it.
    assert!(
        matches!(
            result,
            Err(beacond::CompletionReadRefusal::SupersessionAbsent)
        ),
        "refused, but not as a foreign predecessor: {result:?}"
    );
}

/// §7 slice 2. A digest that exists is not a digest that is yours.
///
/// The point of the test is the *second* Workspace being real: a random UUID would prove only that
/// an unknown scope finds nothing. Here the object exists, the id is correct, and the answer is
/// still a refusal — and for the completion it is a refusal that says which kind, because "not
/// yours" and "not there" are different answers about your own store.
#[test]
fn collector_refuses_cross_workspace_reference_even_when_ids_exist() {
    let fixture = admitted("cross-workspace", vec!["Gates pass".to_owned()]);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let completion_digest = fixture.record(&completion);

    let elsewhere = Uuid::new_v4();
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
            params![elsewhere.to_string()],
        )
        .unwrap();

    // Both digests resolve in the Workspace that owns them.
    let owner = Collector::new(&connection, fixture.store.workspace_id);
    owner
        .collect(&CollectionRequest {
            completion_digest: Some(completion_digest.clone()),
            classification_digest: Some(classification.digest().unwrap().value.clone()),
            ..CollectionRequest::default()
        })
        .expect("the owning Workspace reads them");

    // From the other Workspace, the same two identities are refused rather than returned.
    let stranger = Collector::new(&connection, elsewhere);
    match stranger
        .collect(&CollectionRequest {
            completion_digest: Some(completion_digest),
            ..CollectionRequest::default()
        })
        .expect_err("a completion that is not this Workspace's")
    {
        CollectorRefusal::Unverified { kind, reason } => {
            assert_eq!(kind, "completion");
            assert!(
                reason.contains("CrossWorkspace"),
                "the reader's own word for it is not repeated: {reason}"
            );
        }
        other => panic!("expected the owning reader's cross-Workspace refusal, got {other}"),
    }
    assert!(matches!(
        stranger
            .collect(&CollectionRequest {
                classification_digest: Some(classification.digest().unwrap().value),
                ..CollectionRequest::default()
            })
            .expect_err("a classification that is not this Workspace's"),
        CollectorRefusal::Absent { kind, .. } | CollectorRefusal::Unverified { kind, .. }
            if kind == "classification"
    ));
}

/// §7 slice 2. Three tampered rows, all refused, none of them reaching a rendered claim.
///
/// **Measured, and it contradicts the test's own name:** the record grammars put the version inside
/// the magic (`beacon.mission-impact.v1\0`, ADR 0034), so "unknown version" and "corrupt envelope"
/// are the *same* refusal from the decoder — `Magic`. Three inputs, two refusal names. The test
/// asserts what is true rather than asserting a third name into existence.
#[test]
fn collector_refuses_source_digest_mismatch_unknown_version_and_corrupt_envelope() {
    let fixture = admitted("tampered", vec!["Gates pass".to_owned()]);
    let policy_revision = fixture.policy_revision();
    let honest = MissionImpactClassificationV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        classification_id: Uuid::new_v4(),
        version: 7,
        impact: ImpactLevel::Ordinary,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project.id,
            policy_revision,
            charter_revision: fixture.project.charter.revision,
        },
        rule_id: None,
        reason: "A record whose bytes the decoys are built from.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::None,
    };
    let good_bytes = honest.canonical_bytes().unwrap();

    // 1. Bytes that are a real record, filed under a digest of something else.
    let mismatched_digest = sha256_hex(b"not these bytes");
    // 2. A well-formed record whose grammar version is one nobody has: v1 -> v9 in the magic.
    let mut unknown_version = good_bytes.clone();
    let position = unknown_version
        .windows(2)
        .position(|window| window == b"v1")
        .expect("the magic carries the version");
    unknown_version[position + 1] = b'9';
    // 3. Bytes that are not a record at all.
    let corrupt = b"this envelope belongs to nothing at all".to_vec();

    let connection = fixture.connection();
    let mut reasons: Vec<(&str, String)> = Vec::new();
    for (version, bytes, digest, label) in [
        (
            7_u32,
            good_bytes.clone(),
            mismatched_digest,
            "digest mismatch",
        ),
        (
            8,
            unknown_version.clone(),
            sha256_hex(&unknown_version),
            "unknown version",
        ),
        (9, corrupt.clone(), sha256_hex(&corrupt), "corrupt envelope"),
    ] {
        connection
            .execute(
                "INSERT INTO mission_impact_classifications (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, classification_id, mission_id, project_id, version, impact,
                     basis, policy_revision, charter_revision, classified_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, ?7,
                           'ordinary', 'operator_declared', ?8, ?9, 1788400000000)",
                params![
                    fixture.store.workspace_id.to_string(),
                    digest,
                    bytes,
                    Uuid::new_v4().to_string(),
                    fixture.mission.id.to_string(),
                    fixture.project.id.to_string(),
                    version,
                    policy_revision,
                    fixture.project.charter.revision,
                ],
            )
            .unwrap_or_else(|error| panic!("{label}: {error}"));

        let collector = Collector::new(&connection, fixture.store.workspace_id);
        let refusal = collector
            .collect(&CollectionRequest {
                classification_digest: Some(digest.clone()),
                ..CollectionRequest::default()
            })
            .expect_err(label);
        match refusal {
            CollectorRefusal::Unverified { kind, reason } => {
                assert_eq!(kind, "classification", "{label}");
                assert!(!reason.is_empty(), "{label}: the reader gave no reason");
                reasons.push((label, reason));
            }
            other => panic!("{label}: expected the owning reader's refusal, got {other}"),
        }
    }

    // The digest mismatch is its own refusal; the two envelope failures share one, because the
    // grammar version IS the envelope. Asserted rather than assumed, so a later grammar that
    // separates them fails here and gets its third name on purpose.
    assert_eq!(reasons.len(), 3);
    assert_ne!(
        reasons[0].1, reasons[1].1,
        "a digest mismatch and a bad envelope must not read alike"
    );
    assert_eq!(
        reasons[1].1, reasons[2].1,
        "an unknown version and a corrupt envelope are one refusal in this grammar"
    );
}

fn sha256_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// §7 slice 2. The exactly-once rule is `MissionCompletionV1`'s, and the collector reports its
/// verdict rather than reimplementing it.
///
/// Three shapes, three refusals from the owning module, and one accepted record whose collected
/// results are the Mission's declaration in ordinal order.
#[test]
fn mission_completion_verified_reader_validates_every_declared_success_condition_exactly_once() {
    let fixture = admitted(
        "exactly-once",
        vec![
            "Gates pass".to_owned(),
            "Evidence filed".to_owned(),
            "Review recorded".to_owned(),
        ],
    );
    let boundary = newest_event(&fixture.store);
    let spend = SpendReference::NotMeasured("no ledger was read".to_owned());

    // A gap: two results for three declared conditions.
    let mut gap = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met, ConditionOutcome::Met],
        spend.clone(),
    );
    gap.condition_results.truncate(2);
    let mut connection = fixture.connection();
    record_completion(&mut connection, &gap).expect_err("a gap is refused");

    // A duplicate: ordinal 1 reported twice, and ordinal 2 never.
    let mut duplicate = fixture.completion(
        boundary,
        vec![
            ConditionOutcome::Met,
            ConditionOutcome::Met,
            ConditionOutcome::Met,
        ],
        spend.clone(),
    );
    duplicate.condition_results[2].ordinal = 1;
    record_completion(&mut connection, &duplicate).expect_err("a duplicate ordinal is refused");

    // An ordinal past the end of the declaration.
    let mut past_end = fixture.completion(
        boundary,
        vec![
            ConditionOutcome::Met,
            ConditionOutcome::Met,
            ConditionOutcome::Met,
        ],
        spend.clone(),
    );
    past_end.condition_results[2].ordinal = 9;
    record_completion(&mut connection, &past_end).expect_err("an ordinal past the end is refused");

    // ...and the record that names each declared condition once is accepted, and collects as three
    // results in the Mission's own ordinal order.
    let honest = fixture.completion(
        boundary,
        vec![
            ConditionOutcome::Met,
            ConditionOutcome::NotAssessed,
            ConditionOutcome::AssessmentInconclusive,
        ],
        spend,
    );
    let digest = record_completion(&mut connection, &honest).expect("it is accepted");
    drop(connection);

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("the completion verifies")
        .completion
        .expect("it is collected");
    assert_eq!(
        collected
            .condition_results
            .iter()
            .map(|condition| (condition.ordinal, condition.outcome))
            .collect::<Vec<_>>(),
        vec![
            (0, ConditionOutcome::Met),
            (1, ConditionOutcome::NotAssessed),
            (2, ConditionOutcome::AssessmentInconclusive),
        ],
        "the collected results are not the Mission's declaration, once each, in order"
    );
    // The vocabulary is the completion's own (R13.3). `not_assessed` and `assessment_inconclusive`
    // survive collection as themselves; a two-arm mapping here would print both as `not_met`.
    assert_ne!(
        collected.condition_results[1].outcome,
        ConditionOutcome::NotMet
    );
    assert_ne!(
        collected.condition_results[2].outcome,
        ConditionOutcome::NotMet
    );
}

/// §7 slice 2. A stored completion the write path would never have produced is refused at read.
///
/// The row is inserted directly, because that is the only way a store reaches this state: the write
/// path refuses a record whose results are not the Mission's declared set. The reader is what stands
/// between a corrupted store and a rendered claim, so the corruption is built on purpose.
#[test]
fn collector_refuses_completion_that_fails_its_owning_modules_validation() {
    let fixture = admitted(
        "invalid-completion",
        vec![
            "Gates pass".to_owned(),
            "Evidence filed".to_owned(),
            "Review recorded".to_owned(),
        ],
    );
    let boundary = newest_event(&fixture.store);
    let mut record = fixture.completion(
        boundary,
        vec![
            ConditionOutcome::Met,
            ConditionOutcome::Met,
            ConditionOutcome::Met,
        ],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    record.condition_results.truncate(2);
    let bytes = record.canonical_bytes().unwrap();
    let digest = sha256_hex(&bytes);

    let connection = fixture.connection();
    let member = &fixture.mission.members[0];
    connection
        .execute(
            "INSERT INTO mission_completions (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, completion_id, mission_id, project_id, accountable_resident_id,
                 accountable_configuration_id, boundary_event_seq, disposition, completed_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission_completion.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, 'concluded', 9000)",
            params![
                fixture.store.workspace_id.to_string(),
                digest,
                bytes,
                record.completion_id.to_string(),
                fixture.mission.id.to_string(),
                fixture.mission.project_id.to_string(),
                member.resident_id.to_string(),
                member.admission_configuration_id.to_string(),
                i64::try_from(boundary).unwrap(),
            ],
        )
        .expect("the row itself is storable -- that is the point");

    match Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect_err("the owning module refuses it")
    {
        CollectorRefusal::Unverified { kind, reason } => {
            assert_eq!(kind, "completion");
            assert!(
                reason.contains("ConditionsNotExactlyOnce") || reason.contains("EventCount"),
                "the reader's own refusal is not repeated: {reason}"
            );
        }
        other => panic!("expected the owning module's validation to refuse, got {other}"),
    }
}

/// §7 slice 2. An unmeasured spend never becomes an amount, and the window it was measured over
/// survives the absence of the amount.
///
/// Two shapes, because they are two statements: a completion that cites a record whose measurement
/// is `unmeasured`, and a completion that cites no record at all. Only the first has coverage —
/// coverage is a property of a measurement, not of its absence.
#[test]
fn collector_refuses_unmeasured_spend_and_preserves_declared_coverage() {
    let fixture = admitted("unmeasured-spend", vec!["Gates pass".to_owned()]);
    let spend = fixture.measure_spend(SpendMeasurement::Unmeasured(
        "the ledger for this window was never read".to_owned(),
    ));
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("an unmeasured record is a verified record")
        .spend
        .expect("the completion cited one");

    // The amount is a state and it is `not_evaluated`, carrying the owner's own reason. It is not
    // zero, and there is no arm of `OutcomeState` that could make it zero (R17).
    assert!(
        matches!(&collected.amount, OutcomeState::NotEvaluated(reason)
            if reason == "the ledger for this window was never read"),
        "an unmeasured amount is not `not_evaluated`: {:?}",
        collected.amount
    );
    assert_eq!(collected.measurement, "unmeasured");
    // ...and the declared coverage survived, exactly as the record stated it.
    assert_eq!(
        collected.coverage,
        Some((spend.coverage_from_event_seq, spend.coverage_to_event_seq)),
        "the declared coverage window did not survive the missing amount"
    );

    // The other statement: the completion measured nothing at all. No record, so no coverage --
    // and the reason is the completion's, not a synthesised one.
    let none = admitted("no-spend-record", vec!["Gates pass".to_owned()]);
    let none_boundary = newest_event(&none.store);
    let record = none.completion(
        none_boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger exists for this Mission".to_owned()),
    );
    let none_digest = none.record(&record);
    let none_connection = none.connection();
    let unmeasured = Collector::new(&none_connection, none.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(none_digest),
            ..CollectionRequest::default()
        })
        .expect("a completion that measured nothing is still a completion")
        .spend
        .expect("the state is reported rather than omitted");
    assert_eq!(unmeasured.measurement, "not_measured");
    assert_eq!(unmeasured.coverage, None);
    assert_eq!(unmeasured.digest_value, None);
    assert!(
        matches!(&unmeasured.amount, OutcomeState::NotEvaluated(reason)
        if reason == "no ledger exists for this Mission")
    );
}

/// C49. A completion that DID deliver Artifacts is a typed refusal, never "there were none".
///
/// Fable's mutant on C48: forcing `delivered_artifacts.is_empty()` to `true` left all fifteen
/// collector tests green, because no test ever handed the collector a completion with a delivery.
/// The arm nothing exercised is the one that separates **"we cannot read them yet"** from **"there
/// were none"** — and the second is a claim an Outcome would render as fact.
///
/// The row is inserted directly, and that is the finding underneath the finding: `NoArtifactEntity`
/// is enforced on the WRITE path only (`record_completion`), never in `verified_completion_within`.
/// A stored delivery therefore reads back as a perfectly valid completion, and this arm of the
/// collector is the only thing between it and a rendered document.
#[test]
fn a_completion_that_delivered_artifacts_is_refused_rather_than_reported_as_delivering_none() {
    let fixture = admitted("delivered-artifact", vec!["Gates pass".to_owned()]);
    let boundary = newest_event(&fixture.store);
    let mut record = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    record.delivered_artifacts = vec![[7_u8; 32]];

    // The write path refuses it -- which is why no test had one, and why the read path has to be
    // proved separately rather than assumed to be unreachable.
    let mut connection = fixture.connection();
    record_completion(&mut connection, &record)
        .expect_err("the write path refuses a delivery nothing can verify");

    let bytes = record.canonical_bytes().unwrap();
    let digest = sha256_hex(&bytes);
    let member = &fixture.mission.members[0];
    connection
        .execute(
            "INSERT INTO mission_completions (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, completion_id, mission_id, project_id, accountable_resident_id,
                 accountable_configuration_id, boundary_event_seq, disposition, completed_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission_completion.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, 'concluded', 9000)",
            params![
                fixture.store.workspace_id.to_string(),
                digest,
                bytes,
                record.completion_id.to_string(),
                fixture.mission.id.to_string(),
                fixture.mission.project_id.to_string(),
                member.resident_id.to_string(),
                member.admission_configuration_id.to_string(),
                i64::try_from(boundary).unwrap(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_completion_conditions (
                 workspace_id, completion_digest, mission_id, condition_ordinal, outcome
             ) VALUES (?1, ?2, ?3, 0, 'met')",
            params![
                fixture.store.workspace_id.to_string(),
                digest,
                fixture.mission.id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'mission.completed', ?2, 9000)",
            params![
                fixture.store.workspace_id.to_string(),
                serde_json::json!({
                    "workspaceId": fixture.store.workspace_id.to_string(),
                    "missionId": fixture.mission.id.to_string(),
                    "completionId": record.completion_id.to_string(),
                    "digestValue": digest,
                    "canonicalBytesBase64": data_encoding::BASE64.encode(&bytes),
                })
                .to_string(),
            ],
        )
        .unwrap();

    // The owning reader accepts it: `NoArtifactEntity` is a write-path rule, not a read-path one.
    // Asserted rather than assumed, because if that ever changes this test stops proving the
    // collector's arm and silently becomes a test of the reader instead.
    let read =
        beacond::verified_completion_within(&connection, fixture.store.workspace_id, &digest)
            .expect("the reader has no delivered-Artifact rule");
    assert_eq!(read.delivered_artifacts, vec![[7_u8; 32]]);

    // ...and the collector refuses, by name. Not `Absent`, which would say the Mission delivered
    // nothing -- a sentence about the world rather than about this daemon's capabilities.
    assert_eq!(
        Collector::new(&connection, fixture.store.workspace_id)
            .collect(&CollectionRequest {
                completion_digest: Some(digest),
                ..CollectionRequest::default()
            })
            .expect_err("the cited Artifact is not in the store"),
        CollectorRefusal::Absent {
            kind: "artifact",
            id: HEXLOWER.encode(&[7_u8; 32]),
        }
    );
}

/// C49, the second `Absent(` arm — and it was reporting absence for a state that cannot exist.
///
/// The grep Fable asked for turned up exactly two `Absent(` arms in the collector. This was the
/// other one: a counted measurement with an empty line list became
/// `Absent("counted no priced lines")`. Measured here says the codec cannot even build that record
/// — `MeasuredSpendV1::canonical_bytes` refuses it as `NoLines`, so no verified record reaches the
/// arm. An unreachable arm that answers "there was none" is the same collapse in a quieter place,
/// so it refuses now instead.
#[test]
fn a_counted_measurement_with_no_priced_lines_cannot_be_built_at_all() {
    let fixture = admitted("no-priced-lines", vec!["Gates pass".to_owned()]);
    let mut record = MeasuredSpendV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: newest_event(&fixture.store),
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 0,
            lines: vec![],
        }),
        supersedes: SpendSupersedes::None,
    };
    record
        .canonical_bytes()
        .expect_err("a counted measurement with no lines is not a record");

    // ...and one line is, which is what makes the refusal above a fact about the empty case rather
    // than about this fixture.
    record.measurement = SpendMeasurement::Measured(SpendTotals {
        counted_units: 1,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 1,
            minor_units_exponent: 2,
        }],
    });
    record
        .canonical_bytes()
        .expect("one priced line is a record");
}

/// C49, opus2's F3. The SUCCESS path of Proposal, Ruling and Evaluation.
///
/// The skeleton gave those three sources absence tests and nothing else: no fixture stored one, so
/// every positive branch in `proposal()`, `ruling()` and `evaluation()` was unexecuted. This stores
/// one of each through its own service and collects all three.
///
/// **Measured, and it corrects the brief:** deleting the "cite from the verified value, never the
/// caller's string" guard is NOT red, and no test can make it so. Each reader looks its row up with
/// `WHERE workspace_id = ?1 AND digest_value = ?2` -- an exact match -- so on every reachable input
/// the caller's string and the verified value are the same bytes. The guard is defence in depth
/// against a reader that ever normalises (a `lower()`, a trimmed input), and it is worth keeping for
/// that; it is not evidence, and this test does not pretend to make it so. What these tests DO make
/// falsifiable is the boundary locator for each of the three, which C48 shipped verified by
/// inspection -- see M16, M17 and M18.
#[test]
fn a_stored_proposal_ruling_and_evaluation_each_collect_through_their_owning_reader() {
    let fixture = admitted("positive-path", vec!["Gates pass".to_owned()]);
    fixture.repository_revision();
    fixture.policy_revision();

    let proposal = ProposalService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .submit(&fixture.proposal_record().bytes())
        .expect("the Proposal records");
    let proposal_digest = proposal.digest().value.clone();

    let ruling = RulingService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            "The change is scoped to one test file and is reversible.",
            RulingSupersedes::None,
        )
        .expect("the Ruling records");

    let other = &fixture.mission.members[1];
    let evaluation =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Proposal(proposal_digest.clone()),
                evaluator: Evaluator {
                    resident_id: other.resident_id,
                    configuration_id: other.admission_configuration_id,
                },
                rubric: RubricDeclaration {
                    id: "security-review-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: sha256_hex(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict: Verdict::Uncertain,
                findings: vec![Finding {
                    severity: FindingSeverity::Note,
                    summary: "Read the exact bytes.".to_owned(),
                }],
                conflicts: Conflicts::None,
                rationale: "Independent evaluator reviewed the exact Proposal bytes.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation records");

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            proposal_digests: vec![proposal_digest.clone()],
            ruling_digests: vec![ruling.digest().value.clone()],
            evaluation_digests: vec![evaluation.digest().value.clone()],
            ..CollectionRequest::default()
        })
        .expect("all three verify");

    for (collected, kind, digest) in [
        (
            &collected.proposals[0],
            SourceKind::Proposal,
            &proposal_digest,
        ),
        (
            &collected.rulings[0],
            SourceKind::Ruling,
            &ruling.digest().value,
        ),
        (
            &collected.evaluations[0],
            SourceKind::Evaluation,
            &evaluation.digest().value,
        ),
    ] {
        assert_eq!(collected.kind, kind);
        assert_eq!(collected.workspace_id, fixture.store.workspace_id);
        assert_eq!(&collected.digest_value, digest);
    }
}

/// C49, opus2's F3, second half: the three locator rows C48 could only verify by reading.
///
/// A wrong JSON path refuses every real record as `Unpositioned`, which reads like a store problem
/// and is a typo -- exactly what M5 showed for the completion. With a stored Proposal, Ruling and
/// Evaluation the other three rows become executable, and M16-M18 break each one in turn.
#[test]
fn the_proposal_ruling_and_evaluation_locators_find_their_own_records_events() {
    let fixture = admitted("positive-locators", vec!["Gates pass".to_owned()]);
    fixture.repository_revision();
    fixture.policy_revision();

    let proposal = ProposalService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .submit(&fixture.proposal_record().bytes())
        .expect("the Proposal records");
    let ruling = RulingService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .rule(
            &proposal.digest().value,
            RulingDecision::Approve,
            "The change is scoped to one test file and is reversible.",
            RulingSupersedes::None,
        )
        .expect("the Ruling records");
    let other = &fixture.mission.members[1];
    let evaluation =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Proposal(proposal.digest().value.clone()),
                evaluator: Evaluator {
                    resident_id: other.resident_id,
                    configuration_id: other.admission_configuration_id,
                },
                rubric: RubricDeclaration {
                    id: "security-review-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: sha256_hex(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict: Verdict::Uncertain,
                findings: vec![Finding {
                    severity: FindingSeverity::Note,
                    summary: "Read the exact bytes.".to_owned(),
                }],
                conflicts: Conflicts::None,
                rationale: "Independent evaluator reviewed the exact Proposal bytes.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation records");

    let sources = positionable_sources();
    let connection = fixture.connection();
    for (label, identity) in [
        ("proposal", proposal.digest().value.clone()),
        ("ruling", ruling.digest().value.clone()),
        ("evaluation", evaluation.digest().value.clone()),
    ] {
        let (event_kind, path) = sources[label];
        let found: Option<i64> = connection
            .query_row(
                "SELECT MIN(sequence) FROM events
                 WHERE workspace_id = ?1 AND kind = ?2 AND json_extract(payload_json, ?3) = ?4",
                params![
                    fixture.store.workspace_id.to_string(),
                    event_kind,
                    path,
                    identity
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap();
        assert!(
            found.is_some(),
            "the locator for {label} ({event_kind}, {path}) does not find its own record's Event"
        );
    }
}

/// The appendix bit is the record's, in both directions.
///
/// `collector_reads_each_source_through_its_owning_verified_reader` proves a High classification
/// collects as `true`. This proves an Ordinary one collects as `false` -- the half that makes the
/// bit a read rather than a constant, and the half a hard-coded `true` would pass.
#[test]
fn the_appendix_bit_is_read_from_the_classification_in_both_directions() {
    for (impact, expected) in [(ImpactLevel::High, true), (ImpactLevel::Ordinary, false)] {
        let fixture = admitted(
            &format!("appendix-{}", if expected { "high" } else { "ordinary" }),
            vec!["Gates pass".to_owned()],
        );
        let classification = fixture.classify(impact);
        let connection = fixture.connection();
        let collected = Collector::new(&connection, fixture.store.workspace_id)
            .collect(&CollectionRequest {
                classification_digest: Some(classification.digest().unwrap().value),
                ..CollectionRequest::default()
            })
            .expect("the classification verifies")
            .classification
            .expect("it is collected");
        assert_eq!(
            collected.policy_high_impact, expected,
            "the appendix bit does not follow the record's own impact level"
        );
    }
}

/// §7 slice 2. What the completion delivered and what it left unfinished, reconciled against the
/// completion's own condition set.
///
/// Two reconciliations nobody else makes: a delivered Artifact cannot be cited while no Artifact
/// entity exists to verify it against, and a narrative item may only name a condition this
/// completion actually reports.
#[test]
fn collector_reconciles_delivered_artifacts_and_unfinished_work_with_completion() {
    let fixture = admitted(
        "reconcile",
        vec!["Gates pass".to_owned(), "Evidence filed".to_owned()],
    );
    let boundary = newest_event(&fixture.store);

    // 1. Narrative items that name conditions this completion reports: collected, with the link.
    let mut honest = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met, ConditionOutcome::NotMet],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    honest.unfinished_work = vec![beacond::NarrativeItem {
        ordinal: 0,
        kind: UnfinishedKind::ConditionNotMet,
        condition_ordinal: Some(1),
        summary: "The evidence was not filed before the cut.".to_owned(),
    }];
    honest.uncertainty = vec![beacond::NarrativeItem {
        ordinal: 0,
        kind: UncertaintyKind::UnverifiedClaim,
        condition_ordinal: None,
        summary: "The gate output was not independently reproduced.".to_owned(),
    }];
    let digest = fixture.record(&honest);

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("the completion reconciles")
        .completion
        .expect("it is collected");

    // Delivering nothing is `Absent` -- the domain says there is none -- and never an empty
    // `Verified` list, which says the same thing in a word that means something else.
    assert!(
        matches!(&collected.delivered_artifacts, OutcomeState::Absent(reason)
            if reason == "the completion delivered no Artifacts"),
        "an empty delivery is not `absent`: {:?}",
        collected.delivered_artifacts
    );
    assert_eq!(collected.unfinished_work.len(), 1);
    assert_eq!(collected.unfinished_work[0].kind, "condition_not_met");
    assert_eq!(collected.unfinished_work[0].condition_ordinal, Some(1));
    assert_eq!(collected.uncertainty[0].kind, "unverified_claim");
    assert_eq!(collected.uncertainty[0].condition_ordinal, None);

    // 2. A narrative item naming a condition this completion does not report. The record's decoder
    //    proves the ordinal is a number; that it is THIS completion's ordinal spans two lists and
    //    is nobody's job but the collector's.
    let stray = admitted(
        "reconcile-stray",
        vec!["Gates pass".to_owned(), "Evidence filed".to_owned()],
    );
    let stray_boundary = newest_event(&stray.store);
    let mut record = stray.completion(
        stray_boundary,
        vec![ConditionOutcome::Met, ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    record.unfinished_work = vec![beacond::NarrativeItem {
        ordinal: 0,
        kind: UnfinishedKind::DeferredScope,
        condition_ordinal: Some(7),
        summary: "Deferred, against a condition that does not exist.".to_owned(),
    }];
    let stray_digest = stray.record(&record);
    let stray_connection = stray.connection();
    match Collector::new(&stray_connection, stray.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(stray_digest),
            ..CollectionRequest::default()
        })
        .expect_err("the item names a condition that is not there")
    {
        CollectorRefusal::Irreconcilable { detail } => {
            assert!(detail.contains("condition 7"), "{detail}");
        }
        other => panic!("expected an irreconcilable refusal, got {other}"),
    }

    // 3. A delivered Artifact, while no Artifact entity store exists: refused by the completion's
    //    own reader, which is why the collector never has to decide what to do with one.
    let delivering = admitted("reconcile-artifact", vec!["Gates pass".to_owned()]);
    let delivering_boundary = newest_event(&delivering.store);
    let mut delivered = delivering.completion(
        delivering_boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    delivered.delivered_artifacts = vec![[7_u8; 32]];
    let mut connection = delivering.connection();
    record_completion(&mut connection, &delivered)
        .expect_err("a delivered Artifact needs an Artifact entity to verify it against");
    assert_eq!(
        Collector::new(&connection, delivering.store.workspace_id)
            .artifact(&HEXLOWER.encode(&[7_u8; 32]), delivering.mission.id)
            .expect_err("nothing is stored under that digest"),
        CollectorRefusal::Absent {
            kind: "artifact",
            id: HEXLOWER.encode(&[7_u8; 32]),
        }
    );
}

/// §7 slice 2. Everything is frozen at the completion's **stored** boundary, not at the newest
/// event now.
///
/// The classification is recorded before the cut and the spend after it, so one collects and the
/// other is refused **as late rather than as missing**. That distinction is what proves the locator
/// table actually finds these records: a wrong JSON path would refuse them as `Unpositioned`, which
/// this test would fail on.
#[test]
fn collector_freezes_all_sources_at_completion_event_boundary() {
    let fixture = admitted("boundary", vec!["Gates pass".to_owned()]);
    let classification = fixture.classify(ImpactLevel::High);
    let classification_digest = classification.digest().unwrap().value;

    // The cut: after the classification, before anything else exists.
    let boundary = newest_event(&fixture.store);
    assert!(boundary >= admitted_sequence(&fixture.store, fixture.mission.id));

    // ...and now a spend record, which arrives after it.
    let spend = fixture.measure_spend(SpendMeasurement::Measured(SpendTotals {
        counted_units: 12,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 4_200,
            minor_units_exponent: 2,
        }],
    }));
    assert!(
        newest_event(&fixture.store) > boundary,
        "the spend record produced no Event, so this test would prove nothing"
    );

    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);

    let connection = fixture.connection();
    let collector = Collector::new(&connection, fixture.store.workspace_id);
    match collector
        .collect(&CollectionRequest {
            completion_digest: Some(digest.clone()),
            classification_digest: Some(classification_digest.clone()),
            ..CollectionRequest::default()
        })
        .expect_err("the spend arrived after the cut")
    {
        CollectorRefusal::AfterBoundary {
            kind,
            recorded_at_seq,
            boundary_event_seq,
            ..
        } => {
            assert_eq!(kind, "spend");
            assert_eq!(boundary_event_seq, boundary);
            assert!(
                recorded_at_seq > i64::try_from(boundary).unwrap(),
                "the refusal reports a position that is not after the cut"
            );
        }
        other => panic!("expected a late spend record, got {other}"),
    }

    // The classification was recorded before the cut, so on its own it collects.
    let collected = collector
        .collect(&CollectionRequest {
            classification_digest: Some(classification_digest),
            ..CollectionRequest::default()
        })
        .expect("the classification is at or before the cut");
    assert!(collected.classification.unwrap().policy_high_impact);

    // The third state, and the reason it is not folded into either of the other two: a Receipt row
    // whose run has no Event cannot be placed relative to the cut at all. "The log does not account
    // for it" is not "it arrived late" and not "it is not there", and a freeze that cannot be
    // evaluated must not be assumed to pass.
    let seeded = admitted("boundary-unpositioned", vec!["Gates pass".to_owned()]);
    let run_id = seeded.seeded_receipt();
    let seeded_boundary = newest_event(&seeded.store);
    let seeded_completion = seeded.completion(
        seeded_boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let seeded_digest = seeded.record(&seeded_completion);
    let seeded_connection = seeded.connection();
    let seeded_collector = Collector::new(&seeded_connection, seeded.store.workspace_id);

    // With no completion there is no cut, so the same Receipt collects.
    seeded_collector
        .collect(&CollectionRequest {
            receipt_run_uuids: vec![run_id],
            ..CollectionRequest::default()
        })
        .expect("with no completion there is no cut to place it against");

    // With one, it is refused -- and as its own third state, not as absence or lateness.
    match seeded_collector
        .collect(&CollectionRequest {
            completion_digest: Some(seeded_digest),
            receipt_run_uuids: vec![run_id],
            ..CollectionRequest::default()
        })
        .expect_err("no Event places this Receipt")
    {
        CollectorRefusal::Unpositioned { kind, id } => {
            assert_eq!(kind, "receipt");
            assert_eq!(id, run_id.hyphenated().to_string());
        }
        other => panic!("expected an unpositioned refusal, got {other}"),
    }

    // And the cut is the stored number: a second completion at a later boundary, over the same
    // store, takes the same spend record. Same store, same spend, different answer -- which is only
    // possible if the boundary is read from the record rather than from the clock.
    let later = admitted("boundary-later", vec!["Gates pass".to_owned()]);
    let later_spend = later.measure_spend(SpendMeasurement::Measured(SpendTotals {
        counted_units: 12,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 4_200,
            minor_units_exponent: 2,
        }],
    }));
    let later_boundary = newest_event(&later.store);
    let later_completion = later.completion(
        later_boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: later_spend.spend_id,
            spend_version: later_spend.version,
            spend_sha256: sha256_bytes(&later_spend.canonical_bytes().unwrap()),
        },
    );
    let later_digest = later.record(&later_completion);
    let later_connection = later.connection();
    let collected = Collector::new(&later_connection, later.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(later_digest),
            ..CollectionRequest::default()
        })
        .expect("the spend is at or before this completion's cut");
    assert_eq!(collected.boundary_event_seq, Some(later_boundary));
    assert_eq!(
        collected.spend.unwrap().spend_id,
        Some(later_spend.spend_id)
    );
}

/// §7 slice 2. A record appended after the cut does not change what was collected at it.
///
/// Collect, then write more history, then collect again with the same request: the result is equal.
/// Corrections create a new version; they do not edit an existing one (Kevin's ruling 4).
#[test]
fn late_append_only_record_does_not_mutate_existing_manifest_or_markdown() {
    let fixture = admitted("late-append", vec!["Gates pass".to_owned()]);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let spend = fixture.measure_spend(SpendMeasurement::Measured(SpendTotals {
        counted_units: 3,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 999,
            minor_units_exponent: 2,
        }],
    }));
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);
    let request = CollectionRequest {
        completion_digest: Some(digest),
        classification_digest: Some(classification.digest().unwrap().value),
        ..CollectionRequest::default()
    };

    let connection = fixture.connection();
    let before = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&request)
        .expect("the sources verify");
    drop(connection);

    // Append: a second measured-spend record for the same Mission, with its own Event.
    let appended =
        MeasuredSpendService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id);
    let mut later = spend.clone();
    later.spend_id = Uuid::new_v4();
    later.version = 2;
    later.coverage_to_event_seq = newest_event(&fixture.store);
    appended
        .record(&later.canonical_bytes().unwrap())
        .expect("the later measurement is written");
    assert!(newest_event(&fixture.store) > boundary);

    let connection = fixture.connection();
    let after = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&request)
        .expect("the sources still verify");
    assert_eq!(
        before, after,
        "an append-only record written after the cut changed what was collected at it"
    );
}

/// Every source the collector can position has a locator row, and every row's path resolves.
///
/// The table is the one place a source's Event kind and identity path are written down. A source
/// added to the collector without a row would skip the boundary check silently, and a row whose
/// JSON path is wrong would refuse every real record as `Unpositioned` — which reads like a store
/// problem and is a typo. So: the labels are asserted against the collector's own vocabulary, and
/// each path is executed against a real Event of that kind wherever the fixture has one.
#[test]
fn every_positionable_source_has_a_locator() {
    let sources = positionable_sources();
    for label in [
        "proposal",
        "ruling",
        "evaluation",
        "spend",
        "classification",
        "completion",
    ] {
        assert!(sources.contains_key(label), "no locator row for {label}");
    }

    let fixture = admitted("locators", vec!["Gates pass".to_owned()]);
    let classification = fixture.classify(ImpactLevel::High);
    let spend = fixture.measure_spend(SpendMeasurement::Measured(SpendTotals {
        counted_units: 1,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 1,
            minor_units_exponent: 2,
        }],
    }));
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let completion_digest = fixture.record(&completion);

    let connection = fixture.connection();
    for (label, identity) in [
        ("spend", spend.digest().unwrap().value),
        ("classification", classification.digest().unwrap().value),
        ("completion", completion_digest),
    ] {
        let (event_kind, path) = sources[label];
        let found: Option<i64> = connection
            .query_row(
                "SELECT MIN(sequence) FROM events
                 WHERE workspace_id = ?1 AND kind = ?2 AND json_extract(payload_json, ?3) = ?4",
                params![
                    fixture.store.workspace_id.to_string(),
                    event_kind,
                    path,
                    identity
                ],
                |row| row.get::<_, Option<i64>>(0),
            )
            .unwrap();
        assert!(
            found.is_some(),
            "the locator for {label} ({event_kind}, {path}) does not find its own record's Event"
        );
    }
}

/// Store a completion whose bytes deliver `delivered`, bypassing the write path.
///
/// `record_completion` refuses a non-empty delivery outright, so the only way to exercise the READ
/// path is to write the row the way a restore or an older binary could. Every column is derived
/// from the record's own bytes, so the row is honest about itself -- the question under test is
/// what the collector does with a delivery, not what a corrupt row does.
fn store_completion_with_delivery(
    fixture: &Admitted,
    boundary: u64,
    delivered: Vec<[u8; 32]>,
) -> String {
    let mut record = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    record.delivered_artifacts = delivered;
    let bytes = record.canonical_bytes().unwrap();
    let digest = sha256_hex(&bytes);
    let member = &fixture.mission.members[0];
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO mission_completions (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, completion_id, mission_id, project_id, accountable_resident_id,
                 accountable_configuration_id, boundary_event_seq, disposition, completed_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission_completion.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, 'concluded', 9000)",
            params![
                fixture.store.workspace_id.to_string(),
                digest,
                bytes,
                record.completion_id.to_string(),
                fixture.mission.id.to_string(),
                fixture.mission.project_id.to_string(),
                member.resident_id.to_string(),
                member.admission_configuration_id.to_string(),
                i64::try_from(boundary).unwrap(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_completion_conditions (
                 workspace_id, completion_digest, mission_id, condition_ordinal, outcome
             ) VALUES (?1, ?2, ?3, 0, 'met')",
            params![
                fixture.store.workspace_id.to_string(),
                digest,
                fixture.mission.id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'mission.completed', ?2, 9000)",
            params![
                fixture.store.workspace_id.to_string(),
                serde_json::json!({
                    "workspaceId": fixture.store.workspace_id.to_string(),
                    "missionId": fixture.mission.id.to_string(),
                    "completionId": record.completion_id.to_string(),
                    "digestValue": digest,
                    "canonicalBytesBase64": data_encoding::BASE64.encode(&bytes),
                })
                .to_string(),
            ],
        )
        .unwrap();
    digest
}

/// Scoreboard row 11, closed: a delivered Artifact collects as a VERIFIED digest.
///
/// The collected value is the **exact cited digest**, not the tail of that document's supersession
/// chain: an Outcome cites the bytes the Mission delivered, and resolving to a later version would
/// make the document say something the completion never did. The tail is a read-time surface
/// (`ArtifactService::list_supersession_tail`), asked separately by whoever renders.
#[test]
fn delivered_artifacts_collect_as_verified_digests() {
    let fixture = admitted(
        "delivered-artifacts-verified",
        vec!["Gates pass".to_owned()],
    );
    let first = fixture.record_mission_artifact(b"# Outcome\n\nThe first delivered document.\n");
    let second = fixture.record_mission_artifact(b"# Outcome\n\nThe second delivered document.\n");
    let boundary = newest_event(&fixture.store);
    let mut cited = [[0_u8; 32]; 2];
    cited[0].copy_from_slice(&HEXLOWER.decode(first.digest().value.as_bytes()).unwrap());
    cited[1].copy_from_slice(&HEXLOWER.decode(second.digest().value.as_bytes()).unwrap());
    let mut delivered = cited.to_vec();
    delivered.sort_unstable();
    let digest = store_completion_with_delivery(&fixture, boundary, delivered);

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("both delivered Artifacts read through their owning module");
    let completion = collected.completion.expect("the completion collected");
    match &completion.delivered_artifacts {
        OutcomeState::Verified(digests) => {
            assert_eq!(digests.len(), 2);
            assert!(digests.contains(&first.digest().value));
            assert!(digests.contains(&second.digest().value));
        }
        other => panic!("the delivery did not collect as verified: {other:?}"),
    }
}
/// The decoy: the completion's bytes name a digest nothing stored, while a correct Artifact of the
/// same Mission sits beside it.
///
/// The collection ABORTS. It does not drop the unresolvable entry and render the rest, which would
/// be a document nobody could tell was incomplete, and it does not mark it `broken`, which would be
/// honest-looking evidence about a document that does not exist.
#[test]
fn a_delivered_digest_that_names_nothing_refuses_the_whole_collection() {
    let fixture = admitted("delivered-artifact-decoy", vec!["Gates pass".to_owned()]);
    let real = fixture.record_mission_artifact(b"# Outcome\n\nA document that does exist.\n");
    let boundary = newest_event(&fixture.store);
    let mut present = [0_u8; 32];
    present.copy_from_slice(&HEXLOWER.decode(real.digest().value.as_bytes()).unwrap());
    let mut delivered = vec![present, [0x5c_u8; 32]];
    delivered.sort_unstable();
    let digest = store_completion_with_delivery(&fixture, boundary, delivered);

    let connection = fixture.connection();
    assert_eq!(
        Collector::new(&connection, fixture.store.workspace_id)
            .collect(&CollectionRequest {
                completion_digest: Some(digest),
                ..CollectionRequest::default()
            })
            .expect_err("one unresolvable delivery refuses the whole collection"),
        CollectorRefusal::Absent {
            kind: "artifact",
            id: HEXLOWER.encode(&[0x5c_u8; 32]),
        }
    );
}

/// An Artifact that exists, verifies, and belongs to a DIFFERENT Mission is refused by name.
///
/// The owning reader cannot catch this: the record is perfectly valid, and only the Outcome knows
/// which Mission it is about. `context = none` is kept as a different sentence from "a different
/// Mission", because "nothing places it here" and "it is placed elsewhere" are different facts.
#[test]
fn a_delivered_artifact_scoped_to_another_mission_is_refused() {
    let fixture = admitted("delivered-artifact-scope", vec!["Gates pass".to_owned()]);
    let member = &fixture.mission.members[0];
    let artifacts = ArtifactService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id);
    let unscoped = artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nA Workspace-scoped document.\n",
            ArtifactContext::None,
            member.resident_id,
            member.admission_configuration_id,
            None,
        )
        .expect("a Workspace-scoped Artifact is a valid Artifact");

    let connection = fixture.connection();
    let collector = Collector::new(&connection, fixture.store.workspace_id);
    assert_eq!(
        collector
            .artifact(&unscoped.digest().value, fixture.mission.id)
            .expect_err("nothing places it in this Mission"),
        CollectorRefusal::ArtifactOutOfScope {
            id: unscoped.digest().value.clone(),
            declared_mission: None,
            expected_mission: fixture.mission.id,
        }
    );

    // ...and one scoped to a real, different Mission names that Mission in the refusal.
    let elsewhere = Uuid::new_v4();
    let scoped = fixture.record_mission_artifact(b"# Outcome\n\nThis Mission's document.\n");
    assert_eq!(
        collector
            .artifact(&scoped.digest().value, elsewhere)
            .expect_err("it belongs to another Mission"),
        CollectorRefusal::ArtifactOutOfScope {
            id: scoped.digest().value.clone(),
            declared_mission: Some(fixture.mission.id),
            expected_mission: elsewhere,
        }
    );
    // The control: the same Artifact, asked for by its own Mission, collects.
    assert_eq!(
        collector
            .artifact(&scoped.digest().value, fixture.mission.id)
            .expect("its own Mission")
            .digest_value,
        scoped.digest().value
    );
}

/// An empty delivery stays `Absent`: a statement about the Mission, not about the daemon.
#[test]
fn an_empty_delivery_is_absent_and_never_an_empty_verified_list() {
    let fixture = admitted("delivered-artifacts-empty", vec!["Gates pass".to_owned()]);
    let boundary = newest_event(&fixture.store);
    let digest = store_completion_with_delivery(&fixture, boundary, vec![]);

    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("a completion that delivered nothing collects");
    let completion = collected.completion.expect("the completion collected");
    match &completion.delivered_artifacts {
        OutcomeState::Absent(reason) => assert!(reason.contains("no Artifacts")),
        other => panic!("delivering nothing is not an absence: {other:?}"),
    }
}

/// R19.2's premise, measured: a row filed under THIS Workspace's predicate whose canonical bytes
/// name ANOTHER Workspace never reaches the collector's own arm.
///
/// Measured three ways before deciding. With the row's digest moved to match the foreign bytes, the
/// owner still refuses; with the owner's `columns_match` Workspace guard disabled it STILL refuses
/// (the `artifact.recorded` Event pairing catches it); with the collector's check removed as well
/// it still refuses. So the collector's arm is defence in depth behind three of the owner's guards,
/// kept because it is one line, and proved as a pure function in the module's own tests
/// (acceptance row 0) rather than through a row that would have to forge an Event to reach it.
#[test]
fn an_artifact_whose_bytes_name_another_workspace_is_refused_by_its_owning_reader() {
    let fixture = admitted("artifact-foreign-bytes", vec!["Gates pass".to_owned()]);
    let honest = fixture.record_mission_artifact(b"# Outcome\n\nFiled here, named elsewhere.\n");

    // The same record with another Workspace in its bytes, spliced under this Workspace's row.
    let mut foreign = honest.payload().clone();
    foreign.workspace_id = Uuid::new_v4();
    let foreign_bytes = foreign.canonical_bytes().expect("the grammar allows it");
    let connection = fixture.connection();
    connection
        .execute_batch("DROP TRIGGER artifacts_no_update;")
        .unwrap();
    // The digest column moves WITH the bytes, so the only thing wrong with the row is whose it is:
    // a splice that left the old digest would be refused for the digest first and prove nothing
    // about the Workspace guard.
    let foreign_digest = sha256_hex(&foreign_bytes);
    connection
        .execute(
            "UPDATE artifacts SET canonical_bytes = ?1, digest_value = ?2 WHERE digest_value = ?3",
            params![foreign_bytes, foreign_digest, honest.digest().value],
        )
        .expect("the store admits the splice: SQL cannot decode");

    // The OWNER refuses -- InvalidStore -- and the collector reports exactly that as `Unverified`.
    // The collector's own arm (`ArtifactInAnotherWorkspace`) is never reached on a real row.
    assert_eq!(
        Collector::new(&connection, fixture.store.workspace_id)
            .artifact(&foreign_digest, fixture.mission.id)
            .expect_err("a row whose bytes name another Workspace does not read"),
        CollectorRefusal::Unverified {
            kind: "artifact",
            reason: beacond::ArtifactError::InvalidStore.to_string(),
        }
    );
}

#[test]
fn collector_refuses_a_classification_for_a_different_mission_than_the_completion() {
    let fixture = admitted(
        "classification-cross-mission",
        vec!["Gates pass".to_owned()],
    );
    let policy_revision = fixture.policy_revision();

    let other_mission = Uuid::new_v4();
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO missions SELECT ?1, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json, prohibitions_json,
                    lifecycle, admitted_by, admitted_at_ms
               FROM missions WHERE id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_admission_members
               SELECT workspace_id, ?1, team_id, position, resident_id, configuration_id
               FROM mission_admission_members WHERE mission_id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    drop(connection);

    let classification = MissionImpactClassificationV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: other_mission,
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: ImpactLevel::High,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project.id,
            policy_revision,
            charter_revision: fixture.project.charter.revision,
        },
        rule_id: None,
        reason: "A different Mission was classified.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::None,
    };
    MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .classify(&classification.canonical_bytes().unwrap())
        .unwrap();

    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let completion_digest = fixture.record(&completion);
    let connection = fixture.connection();
    let result =
        Collector::new(&connection, fixture.store.workspace_id).collect(&CollectionRequest {
            completion_digest: Some(completion_digest),
            classification_digest: Some(classification.digest().unwrap().value),
            ..CollectionRequest::default()
        });
    assert!(
        result.is_err(),
        "collector joined Mission A's completion to Mission B's classification: {result:?}",
    );
}

#[test]
fn collector_refuses_a_spend_record_for_a_different_mission_than_the_completion() {
    let fixture = admitted("spend-cross-mission", vec!["Gates pass".to_owned()]);
    let other_mission = Uuid::new_v4();
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO missions SELECT ?1, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json, prohibitions_json,
                    lifecycle, admitted_by, admitted_at_ms
               FROM missions WHERE id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_admission_members
               SELECT workspace_id, ?1, team_id, position, resident_id, configuration_id
               FROM mission_admission_members WHERE mission_id = ?2",
            params![other_mission.to_string(), fixture.mission.id.to_string()],
        )
        .unwrap();
    drop(connection);

    let spend = MeasuredSpendV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: other_mission,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: 1,
        coverage_to_event_seq: newest_event(&fixture.store),
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                unit: "USD".to_owned(),
                amount_minor: 100,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    };
    MeasuredSpendService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .record(&spend.canonical_bytes().unwrap())
        .unwrap();

    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let completion_digest = fixture.record(&completion);
    let connection = fixture.connection();
    let result =
        Collector::new(&connection, fixture.store.workspace_id).collect(&CollectionRequest {
            completion_digest: Some(completion_digest),
            ..CollectionRequest::default()
        });
    assert!(
        result.is_err(),
        "collector joined Mission A's completion to Mission B's spend: {result:?}",
    );
}

#[test]
fn a_forged_early_event_cannot_pull_a_post_boundary_classification_into_the_outcome() {
    let fixture = admitted(
        "classification-forged-boundary",
        vec!["Gates pass".to_owned()],
    );
    let policy_revision = fixture.policy_revision();
    let classification = MissionImpactClassificationV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: ImpactLevel::High,
        basis: ClassificationBasis::OperatorDeclared,
        policy: PolicyRevisionRef {
            project_id: fixture.project.id,
            policy_revision,
            charter_revision: fixture.project.charter.revision,
        },
        rule_id: None,
        reason: "This classification is recorded after the completion boundary.".to_owned(),
        classified_by: ClassificationActor::Operator,
        classified_at_ms: 1_788_400_000_000,
        supersedes: ClassificationSupersedes::None,
    };
    let classification_digest = classification.digest().unwrap().value;
    fixture
        .connection()
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'mission.impact_classified', ?2, 0)",
            params![
                fixture.store.workspace_id.to_string(),
                json!({"digest": {"value": classification_digest}}).to_string(),
            ],
        )
        .unwrap();

    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let completion_digest = fixture.record(&completion);
    MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .classify(&classification.canonical_bytes().unwrap())
        .unwrap();

    let connection = fixture.connection();
    let result =
        Collector::new(&connection, fixture.store.workspace_id).collect(&CollectionRequest {
            completion_digest: Some(completion_digest),
            classification_digest: Some(classification_digest),
            ..CollectionRequest::default()
        });
    assert!(
        matches!(
            result,
            Err(CollectorRefusal::AfterBoundary {
                kind: "classification",
                ..
            })
        ),
        "a forged early event made a post-boundary classification visible: {result:?}",
    );
}
#[test]
fn collector_refuses_measured_spend_whose_coverage_starts_after_mission_admission() {
    let fixture = admitted("late-spend-coverage", vec!["Gates pass".to_owned()]);
    let admission = admitted_sequence(&fixture.store, fixture.mission.id);
    fixture.classify(ImpactLevel::Ordinary);
    let late_start = newest_event(&fixture.store);
    assert!(
        late_start > admission,
        "the probe needs an event after admission"
    );

    let spend = MeasuredSpendV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: late_start,
        coverage_to_event_seq: late_start,
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                unit: "USD".to_owned(),
                amount_minor: 100,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    };
    MeasuredSpendService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .record(&spend.canonical_bytes().unwrap())
        .expect("the spend store delegates Mission-span reconciliation to the collector");

    let completion = fixture.completion(
        newest_event(&fixture.store),
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    // sol1's probe ended "collect succeeds, and the window starts at or before admission" -- a
    // shape that could only go GREEN by rewriting the record's declared window. The fix refuses
    // by name instead, so the probe's tail says so; the relation it measures is the same.
    let refusal = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect_err("a window that starts after the Mission was admitted does not span it");
    assert_eq!(
        refusal,
        CollectorRefusal::SpendCoverageStartsAfterMission {
            id: HEXLOWER.encode(&sha256_bytes(&spend.canonical_bytes().unwrap())),
            coverage_from_event_seq: late_start,
            mission_admitted_at_seq: admission,
        },
        "verified actual spend starts at event {late_start}, after Mission admission {admission}"
    );
}

/// The boundary of #11, from the other side: a window that begins AT the admission Event spans
/// the Mission from its first moment and collects as Verified. `<` would refuse it; the check is
/// `>`.
#[test]
fn a_spend_window_that_starts_at_admission_spans_the_mission() {
    let fixture = admitted("spend-coverage-at-admission", vec!["Gates pass".to_owned()]);
    let admission = admitted_sequence(&fixture.store, fixture.mission.id);
    let spend = MeasuredSpendV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        spend_id: Uuid::new_v4(),
        version: 1,
        origin: SpendOrigin::DaemonLedger,
        measured_at_ms: 1_788_400_000_000,
        coverage_from_event_seq: admission,
        coverage_to_event_seq: newest_event(&fixture.store),
        measurement: SpendMeasurement::Measured(SpendTotals {
            counted_units: 1,
            lines: vec![SpendLine {
                unit: "USD".to_owned(),
                amount_minor: 100,
                minor_units_exponent: 2,
            }],
        }),
        supersedes: SpendSupersedes::None,
    };
    MeasuredSpendService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .record(&spend.canonical_bytes().unwrap())
        .expect("the spend record is written");
    let completion = fixture.completion(
        newest_event(&fixture.store),
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("a window from the admission Event spans the Mission");
    let spend = collected.spend.expect("the completion cited spend");
    assert_eq!(spend.coverage, Some((admission, spend.coverage.unwrap().1)));
    assert!(
        matches!(spend.amount, OutcomeState::Verified(_)),
        "{:?}",
        spend.amount
    );
}

#[test]
fn collector_preserves_where_a_partial_spend_measurement_is_incomplete() {
    let fixture = admitted("partial-spend-gap", vec!["Gates pass".to_owned()]);
    let spend = fixture.measure_spend(SpendMeasurement::PartiallyMeasured(
        SpendTotals {
            counted_units: 11,
            lines: vec![SpendLine {
                unit: "USD".to_owned(),
                amount_minor: 4_200,
                minor_units_exponent: 2,
            }],
        },
        vec![UncountedEntry {
            kind: UncountedKind::ProviderDidNotPrice,
            detail: "provider omitted one invocation".to_owned(),
        }],
    ));
    let boundary = newest_event(&fixture.store);
    let completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("the partial measurement is a valid verified spend");
    let spend = collected.spend.expect("the completion cited spend");

    assert_eq!(spend.measurement, "partially_measured");
    let exposed = format!("{spend:?}");
    assert!(
        exposed.contains("provider_did_not_price")
            && exposed.contains("provider omitted one invocation"),
        "the collected partial measurement lost the named gap: {exposed}"
    );
}

/// The three states of #14 are three values: a whole count is `Complete` (a positive claim), a
/// partial count carries its gaps exactly as recorded, and an unmeasured record -- or none -- has
/// `NoCount`. None of them is another's default.
#[test]
fn a_whole_count_a_partial_count_and_no_count_are_three_distinct_states() {
    let totals = || SpendTotals {
        counted_units: 3,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 900,
            minor_units_exponent: 2,
        }],
    };
    for (label, measurement, expected) in [
        (
            "measured",
            SpendMeasurement::Measured(totals()),
            SpendCompleteness::Complete,
        ),
        (
            "partially_measured",
            SpendMeasurement::PartiallyMeasured(
                totals(),
                vec![
                    UncountedEntry {
                        kind: UncountedKind::OutsideRetention,
                        detail: "the ledger keeps thirty days".to_owned(),
                    },
                    UncountedEntry {
                        kind: UncountedKind::UnitNotConvertible,
                        detail: "one provider bills in tokens".to_owned(),
                    },
                ],
            ),
            SpendCompleteness::Partial(vec![
                CollectedUncounted {
                    kind: "outside_retention".to_owned(),
                    detail: "the ledger keeps thirty days".to_owned(),
                },
                CollectedUncounted {
                    kind: "unit_not_convertible".to_owned(),
                    detail: "one provider bills in tokens".to_owned(),
                },
            ]),
        ),
        (
            "unmeasured",
            SpendMeasurement::Unmeasured("no ledger was read".to_owned()),
            SpendCompleteness::NoCount,
        ),
    ] {
        let fixture = admitted(
            &format!("completeness-{label}"),
            vec!["Gates pass".to_owned()],
        );
        let spend = fixture.measure_spend(measurement);
        let completion = fixture.completion(
            newest_event(&fixture.store),
            vec![ConditionOutcome::Met],
            SpendReference::Recorded {
                spend_id: spend.spend_id,
                spend_version: spend.version,
                spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
            },
        );
        let digest = fixture.record(&completion);
        let connection = fixture.connection();
        let collected = Collector::new(&connection, fixture.store.workspace_id)
            .collect(&CollectionRequest {
                completion_digest: Some(digest),
                ..CollectionRequest::default()
            })
            .expect("every measurement state collects");
        let spend = collected.spend.expect("the completion cited spend");
        assert_eq!(spend.measurement, label);
        assert_eq!(spend.completeness, expected, "{label}");
    }

    // And no record at all is NoCount too -- not Complete, which is a count with nothing missing.
    let fixture = admitted("completeness-not-measured", vec!["Gates pass".to_owned()]);
    let completion = fixture.completion(
        newest_event(&fixture.store),
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("nobody counted".to_owned()),
    );
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("a completion that recorded no spend collects");
    let spend = collected.spend.expect("the spend slot is present");
    assert_eq!(spend.measurement, "not_measured");
    assert_eq!(spend.completeness, SpendCompleteness::NoCount);
}

/// kimi's P1 on #14: the distinctness test covered measured, partial, unmeasured and no record, and
/// not `broken` -- the one state where reporting `Complete` would be the exact collapse the enum
/// forbids (someone tried, and the ledger is unusable). A broken measurement has no count to be
/// complete or incomplete: `NoCount`, beside an amount that says `Broken` in the record's words.
#[test]
fn a_broken_measurement_has_no_count_to_be_complete() {
    let fixture = admitted("completeness-broken", vec!["Gates pass".to_owned()]);
    let spend = fixture.measure_spend(SpendMeasurement::Broken(
        "the ledger for this window is unreadable".to_owned(),
    ));
    let completion = fixture.completion(
        newest_event(&fixture.store),
        vec![ConditionOutcome::Met],
        SpendReference::Recorded {
            spend_id: spend.spend_id,
            spend_version: spend.version,
            spend_sha256: sha256_bytes(&spend.canonical_bytes().unwrap()),
        },
    );
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("a broken measurement is a valid verified record");
    let spend = collected.spend.expect("the completion cited spend");
    assert_eq!(spend.measurement, "broken");
    assert_eq!(spend.completeness, SpendCompleteness::NoCount);
    assert_eq!(
        spend.amount,
        OutcomeState::Broken("the ledger for this window is unreadable".to_owned())
    );
}

fn astra_artifact_evaluation(
    fixture: &Admitted,
    artifact: &beacond::VerifiedArtifactRecord,
) -> beacond::Evaluation {
    let evaluator = &fixture.mission.members[1];
    EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .record(EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(artifact.digest().value.clone()),
            evaluator: Evaluator {
                resident_id: evaluator.resident_id,
                configuration_id: evaluator.admission_configuration_id,
            },
            rubric: RubricDeclaration {
                id: "astra-seam-v1".to_owned(),
                version: 1,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: HEXLOWER.encode(&[0x31; 32]),
            },
            cited_runs: vec![],
            verdict: Verdict::Meets,
            findings: vec![],
            conflicts: Conflicts::None,
            rationale: "The exact candidate bytes meet this rubric.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        })
        .unwrap()
}

#[test]
fn astra_s5_delivered_artifact_must_exist_at_the_completions_boundary() {
    for deliver_late in [false, true] {
        let f = admitted("astra-late-delivery", vec!["Gates pass".to_owned()]);
        let early = f.record_mission_artifact(b"# Before the boundary\n");
        let boundary = newest_event(&f.store);
        assert_eq!(early.creation_event_seq(), boundary);
        let late = f.record_mission_artifact(b"# After the boundary\n");
        assert!(late.creation_event_seq() > boundary);
        let delivered = if deliver_late { &late } else { &early };
        let digest: [u8; 32] = HEXLOWER
            .decode(delivered.digest().value.as_bytes())
            .unwrap()
            .try_into()
            .unwrap();
        let completion = store_completion_with_delivery(&f, boundary, vec![digest]);
        let conn = f.connection();
        let result = Collector::new(&conn, f.store.workspace_id).collect(&CollectionRequest {
            completion_digest: Some(completion),
            ..CollectionRequest::default()
        });
        eprintln!(
            "S5 late={deliver_late} cut={boundary} delivered_cursor={} result={result:?}",
            delivered.creation_event_seq()
        );
        if deliver_late {
            assert!(
                result.is_err(),
                "a post-boundary Artifact became a frozen Verified delivery"
            );
        } else {
            assert_eq!(
                result.unwrap().completion.unwrap().delivered_artifacts,
                OutcomeState::Verified(vec![early.digest().value.clone()])
            );
        }
    }
}

#[test]
fn astra_s6_unverified_evaluation_event_cannot_move_a_late_citation_before_the_cut() {
    let f = admitted("astra-evaluation-locator", vec!["Gates pass".to_owned()]);
    let artifact = f.record_mission_artifact(b"# Cited Artifact\n");
    let boundary = newest_event(&f.store);
    let completion = f.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    let completion_digest = f.record(&completion);
    let evaluation = astra_artifact_evaluation(&f, &artifact);
    let conn = f.connection();
    let collector = Collector::new(&conn, f.store.workspace_id);
    let request = CollectionRequest {
        completion_digest: Some(completion_digest),
        evaluation_digests: vec![evaluation.digest().value.clone()],
        ..CollectionRequest::default()
    };
    let before = collector.collect(&request).unwrap_err();
    assert!(matches!(
        before,
        CollectorRefusal::AfterBoundary {
            kind: "evaluation",
            ..
        }
    ));
    // Re-type an earlier Event as a locator-only decoy; it carries no Evaluation bytes at all.
    // The valid, later evaluation.recorded Event remains byte-identical in the ledger.
    let decoy_seq: u64 = conn
        .query_row(
            "SELECT MIN(sequence) FROM events WHERE kind='workspace.created'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|s| u64::try_from(s).unwrap())
        .unwrap();
    assert!(decoy_seq < boundary);
    conn.execute(
        "UPDATE events SET kind = 'evaluation.recorded', payload_json = ?1 WHERE sequence = ?2",
        params![
            json!({"evaluation": {"digest": {"value": evaluation.digest().value}}}).to_string(),
            i64::try_from(decoy_seq).unwrap()
        ],
    )
    .unwrap();
    let after = collector.collect(&request);
    eprintln!("S6 decoy_seq={decoy_seq} cut={boundary} before={before:?} after={after:?}");
    assert!(
        after.is_err(),
        "an unverified locator decoy laundered a late Evaluation into the frozen collection"
    );
}

/// R23.2: a real Evaluation cited under a condition collects as the same `CollectedDigest` the
/// top-level citations carry -- kind, Workspace, digest -- read through its owning reader at the
/// completion's boundary and scoped to the Mission through its subject's own bytes. The count is
/// the list's length, not a second field.
#[test]
fn a_real_evaluation_cited_under_a_condition_collects_with_its_envelope() {
    let fixture = admitted("support-evaluation", vec!["Gates pass".to_owned()]);
    fixture.repository_revision();
    fixture.policy_revision();
    let proposal = ProposalService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .submit(&fixture.proposal_record().bytes())
        .expect("the Proposal records");
    let evaluator = &fixture.mission.members[1];
    let evaluation =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Proposal(proposal.digest().value.clone()),
                evaluator: Evaluator {
                    resident_id: evaluator.resident_id,
                    configuration_id: evaluator.admission_configuration_id,
                },
                rubric: RubricDeclaration {
                    id: "security-review-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: sha256_hex(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict: Verdict::Meets,
                findings: vec![],
                conflicts: Conflicts::None,
                rationale: "The gate passed on the exact Proposal bytes.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation records");
    let mut cited = [0_u8; 32];
    cited.copy_from_slice(
        &HEXLOWER
            .decode(evaluation.digest().value.as_bytes())
            .unwrap(),
    );

    let mut completion = fixture.completion(
        newest_event(&fixture.store),
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    completion.condition_results[0].support = vec![SupportRef::Evaluation(cited)];
    let digest = fixture.record(&completion);
    let connection = fixture.connection();
    let collected = Collector::new(&connection, fixture.store.workspace_id)
        .collect(&CollectionRequest {
            completion_digest: Some(digest),
            ..CollectionRequest::default()
        })
        .expect("a verified Evaluation under a condition collects");
    let condition = &collected.completion.unwrap().condition_results[0];
    assert_eq!(
        condition.support,
        vec![CollectedSupport::Evaluation(CollectedDigest {
            kind: SourceKind::Evaluation,
            workspace_id: fixture.store.workspace_id,
            digest_value: evaluation.digest().value.clone(),
        })]
    );
    assert_eq!(condition.support_count(), condition.support.len());
}

/// The digest bytes a `SupportRef` carries, from a verified record's hex digest.
fn digest_bytes(hex: &str) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&HEXLOWER.decode(hex.as_bytes()).unwrap());
    bytes
}

/// A Receipt a completion can cite as support: `seeded_receipt`'s run and receipt rows, plus the
/// `run.executed` Event that positions the run in the log -- the seeded rows alone have no Event,
/// and a Receipt with no Event is `Unpositioned`, which is the wrong refusal to test against.
/// `bound` says the fixture already bound repository revision 1 through `repository_revision()`,
/// so only the run and receipt rows are inserted.
fn receipt_for_support(fixture: &Admitted, bound: bool) -> (Uuid, String) {
    let run_id = Uuid::new_v4();
    let connection = fixture.connection();
    if !bound {
        connection
            .execute(
                "INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind, source,
                     base_revision, declared_checks_json, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, ?3, 'local_path', '/private/beacon/repo',
                           '0123456789012345678901234567890123456789',
                           '[{\"name\":\"unit\"}]', 'operator', 0)",
                params![
                    fixture.store.workspace_id.to_string(),
                    fixture.project.id.to_string(),
                    fixture.project.charter.revision
                ],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'check_preflight', 1, ?4, 'verified', 0, 10, 11)",
            params![
                fixture.store.workspace_id.to_string(),
                run_id.to_string(),
                fixture.project.id.to_string(),
                fixture.project.charter.revision
            ],
        )
        .unwrap();
    let receipt_json = receipt_body(fixture.store.workspace_id, fixture.project.id, run_id);
    connection
        .execute(
            "INSERT INTO receipts (
                 workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
             ) VALUES (?1, ?2, 1, 'verified', ?3, 11)",
            params![
                fixture.store.workspace_id.to_string(),
                run_id.to_string(),
                receipt_json,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'run.executed', ?2, 11)",
            params![
                fixture.store.workspace_id.to_string(),
                json!({ "runId": run_id.to_string() }).to_string(),
            ],
        )
        .unwrap();
    (run_id, receipt_json)
}

/// A completion for this Mission at `boundary` whose one condition cites `support`, recorded.
/// One per fixture: the store admits one completion per Mission.
fn record_completion_with_support(
    fixture: &Admitted,
    boundary: u64,
    support: Vec<SupportRef>,
) -> String {
    let mut completion = fixture.completion(
        boundary,
        vec![ConditionOutcome::Met],
        SpendReference::NotMeasured("no ledger was read".to_owned()),
    );
    completion.condition_results[0].support = support;
    fixture.record(&completion)
}

fn collect_completion(
    fixture: &Admitted,
    digest: String,
) -> Result<beacond::CollectedSources, CollectorRefusal> {
    let connection = fixture.connection();
    Collector::new(&connection, fixture.store.workspace_id).collect(&CollectionRequest {
        completion_digest: Some(digest),
        ..CollectionRequest::default()
    })
}

/// A Proposal of this Mission and an Evaluation of it by the second member, recorded now.
fn proposal_and_evaluation(fixture: &Admitted) -> (beacond::Proposal, beacond::Evaluation) {
    let proposal = ProposalService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .submit(&fixture.proposal_record().bytes())
        .expect("the Proposal records");
    let evaluator = &fixture.mission.members[1];
    let evaluation =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Proposal(proposal.digest().value.clone()),
                evaluator: Evaluator {
                    resident_id: evaluator.resident_id,
                    configuration_id: evaluator.admission_configuration_id,
                },
                rubric: RubricDeclaration {
                    id: "security-review-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: sha256_hex(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict: Verdict::Meets,
                findings: vec![],
                conflicts: Conflicts::None,
                rationale: "The gate passed on the exact Proposal bytes.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation records");
    (proposal, evaluation)
}

/// Every kind the owner's vocabulary has, under one condition, each collected as the value its
/// own reader returned: the three digest kinds as `CollectedDigest`, the Receipt as its own
/// `(run, version, checksum)` identity. The record's order is the set's order (kind, then key).
#[test]
fn every_support_kind_collects_through_its_owner_under_one_condition() {
    let fixture = admitted("support-every-kind", vec!["Gates pass".to_owned()]);
    fixture.repository_revision();
    fixture.policy_revision();
    let artifact = fixture.record_mission_artifact(b"# The delivered document\n");
    let (run_id, receipt_json) = receipt_for_support(&fixture, true);
    let (proposal, evaluation) = proposal_and_evaluation(&fixture);
    let ruling = RulingService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .rule(
            &proposal.digest().value,
            RulingDecision::Approve,
            "The change is scoped to one test file and is reversible.",
            RulingSupersedes::None,
        )
        .expect("the Ruling records");

    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![
            SupportRef::Artifact(digest_bytes(&artifact.digest().value)),
            SupportRef::Receipt {
                run_id,
                receipt_version: 1,
                receipt_bytes_sha256: sha256_bytes(receipt_json.as_bytes()),
            },
            SupportRef::Evaluation(digest_bytes(&evaluation.digest().value)),
            SupportRef::Ruling(digest_bytes(&ruling.digest().value)),
        ],
    );
    let collected = collect_completion(&fixture, digest).expect("every kind verifies");
    let condition = &collected.completion.unwrap().condition_results[0];
    let cited = |kind: SourceKind, digest_value: &str| CollectedDigest {
        kind,
        workspace_id: fixture.store.workspace_id,
        digest_value: digest_value.to_owned(),
    };
    assert_eq!(
        condition.support,
        vec![
            CollectedSupport::Artifact(cited(SourceKind::Artifact, &artifact.digest().value)),
            CollectedSupport::Receipt(beacond::ReceiptRef {
                workspace_id: fixture.store.workspace_id,
                run_uuid: run_id,
                receipt_version: 1,
                outcome_checksum: sha256_hex(receipt_json.as_bytes()),
            }),
            CollectedSupport::Evaluation(cited(SourceKind::Evaluation, &evaluation.digest().value)),
            CollectedSupport::Ruling(cited(SourceKind::Ruling, &ruling.digest().value)),
        ]
    );
    assert_eq!(condition.support_count(), 4);
}

/// The decoy: the completion's word for its support disagrees with the owner's record while the
/// record itself is correct. A cited Receipt whose checksum is over bytes the store never held, or
/// whose version the store does not hold, is refused by name; an Evaluation digest nothing stored
/// is absent, not blank. One store per case: the store admits one completion per Mission.
#[test]
fn a_lie_in_the_completions_support_is_refused_while_the_owned_record_is_correct() {
    let fixture = admitted("support-decoy-checksum", vec!["Gates pass".to_owned()]);
    let (run_id, _) = receipt_for_support(&fixture, false);
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Receipt {
            run_id,
            receipt_version: 1,
            receipt_bytes_sha256: sha256_bytes(b"{\"outcome\":\"verified\",\"forged\":true}"),
        }],
    );
    assert_eq!(
        collect_completion(&fixture, digest).expect_err("the completion lies about the Receipt"),
        CollectorRefusal::CitedIdentityMismatch {
            kind: "receipt",
            id: run_id.hyphenated().to_string(),
        }
    );

    let fixture = admitted("support-decoy-version", vec!["Gates pass".to_owned()]);
    let (run_id, receipt_json) = receipt_for_support(&fixture, false);
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Receipt {
            run_id,
            receipt_version: 2,
            receipt_bytes_sha256: sha256_bytes(receipt_json.as_bytes()),
        }],
    );
    assert_eq!(
        collect_completion(&fixture, digest).expect_err("version 2 is not what the store holds"),
        CollectorRefusal::CitedIdentityMismatch {
            kind: "receipt",
            id: run_id.hyphenated().to_string(),
        }
    );

    let fixture = admitted("support-decoy-absent", vec!["Gates pass".to_owned()]);
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Evaluation([0x5a; 32])],
    );
    assert_eq!(
        collect_completion(&fixture, digest).expect_err("nothing stored has that digest"),
        CollectorRefusal::Absent {
            kind: "evaluation",
            id: HEXLOWER.encode(&[0x5a; 32]),
        }
    );
}

/// Support recorded AFTER the completion's boundary is not part of this Outcome: the record is
/// real, verified and this Mission's, and it is refused as after the boundary -- an Evaluation by
/// the Event that carries its bytes, an Artifact by the creation cursor its owner re-derives --
/// never as absent.
#[test]
fn support_recorded_after_the_boundary_is_refused_by_name() {
    let fixture = admitted(
        "support-after-boundary-evaluation",
        vec!["Gates pass".to_owned()],
    );
    fixture.repository_revision();
    fixture.policy_revision();
    let boundary = newest_event(&fixture.store);
    let (_, evaluation) = proposal_and_evaluation(&fixture);
    let digest = record_completion_with_support(
        &fixture,
        boundary,
        vec![SupportRef::Evaluation(digest_bytes(
            &evaluation.digest().value,
        ))],
    );
    match collect_completion(&fixture, digest).expect_err("after the boundary") {
        CollectorRefusal::AfterBoundary {
            kind: "evaluation",
            id,
            boundary_event_seq,
            ..
        } => {
            assert_eq!(id, evaluation.digest().value);
            assert_eq!(boundary_event_seq, boundary);
        }
        other => panic!("{other:?}"),
    }

    let fixture = admitted(
        "support-after-boundary-artifact",
        vec!["Gates pass".to_owned()],
    );
    let boundary = newest_event(&fixture.store);
    let late = fixture.record_mission_artifact(b"# Delivered after the cut\n");
    let digest = record_completion_with_support(
        &fixture,
        boundary,
        vec![SupportRef::Artifact(digest_bytes(&late.digest().value))],
    );
    match collect_completion(&fixture, digest).expect_err("after the boundary") {
        CollectorRefusal::AfterBoundary {
            kind: "artifact",
            id,
            boundary_event_seq,
            ..
        } => {
            assert_eq!(id, late.digest().value);
            assert_eq!(boundary_event_seq, boundary);
        }
        other => panic!("{other:?}"),
    }
}

/// Support scoped elsewhere is refused with the scope the record's own bytes declare: an Artifact
/// with no Mission context cannot support this Mission's condition, and the refusal says so.
#[test]
fn support_about_another_mission_is_refused_by_its_own_bytes() {
    let fixture = admitted("support-cross-mission", vec!["Gates pass".to_owned()]);
    let member = &fixture.mission.members[0];
    let unscoped = ArtifactService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .record(
            ArtifactKind::OutcomeDocument,
            b"# A Workspace-scoped document\n",
            ArtifactContext::None,
            member.resident_id,
            member.admission_configuration_id,
            None,
        )
        .expect("a Workspace-scoped Artifact is a valid Artifact");
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Artifact(digest_bytes(&unscoped.digest().value))],
    );
    assert_eq!(
        collect_completion(&fixture, digest).expect_err("nothing places it in this Mission"),
        CollectorRefusal::ArtifactOutOfScope {
            id: unscoped.digest().value.clone(),
            declared_mission: None,
            expected_mission: fixture.mission.id,
        }
    );
}

/// A second admitted Mission in this Workspace, cloned from the fixture's so that only its
/// identity differs -- the same Project, Charter, Team and roster -- so every foreign key a
/// record under it needs already holds. Its roster clones freely: the roster trigger closes a
/// Mission's members only once its admission Event exists, and the clone has none.
fn second_mission(fixture: &Admitted) -> Uuid {
    let id = Uuid::new_v4();
    let connection = fixture.connection();
    connection
        .execute(
            "INSERT INTO missions (
                 id, workspace_id, project_id, charter_revision, team_id, desired_outcome,
                 success_conditions_json, constraints_json, prohibitions_json, lifecycle,
                 admitted_by, admitted_at_ms
             ) SELECT ?1, workspace_id, project_id, charter_revision, team_id, desired_outcome,
                      success_conditions_json, constraints_json, prohibitions_json, lifecycle,
                      admitted_by, admitted_at_ms
               FROM missions WHERE workspace_id = ?2 AND id = ?3",
            params![
                id.to_string(),
                fixture.store.workspace_id.to_string(),
                fixture.mission.id.to_string(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO mission_admission_members (
                 workspace_id, mission_id, team_id, position, resident_id, configuration_id
             ) SELECT workspace_id, ?1, team_id, position, resident_id, configuration_id
               FROM mission_admission_members WHERE workspace_id = ?2 AND mission_id = ?3",
            params![
                id.to_string(),
                fixture.store.workspace_id.to_string(),
                fixture.mission.id.to_string(),
            ],
        )
        .unwrap();
    id
}

/// The Mission arms for the two kinds whose Mission is their Proposal's: an Evaluation of another
/// Mission's Proposal, and a Ruling on it, each cited under this Mission's condition, each refused
/// with the Mission the Proposal's own verified scope declares. Without this witness the
/// comparison for these two kinds could be deleted and every other test would stay green.
#[test]
fn support_by_an_evaluation_or_ruling_of_another_missions_proposal_is_refused_by_name() {
    let proposal_of_another_mission = |fixture: &Admitted| -> (Uuid, beacond::Proposal) {
        fixture.repository_revision();
        fixture.policy_revision();
        let other = second_mission(fixture);
        let mut record = fixture.proposal_record();
        record.mission_id = other;
        let proposal =
            ProposalService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
                .submit(&record.bytes())
                .expect("a Proposal for the other Mission records");
        (other, proposal)
    };

    let fixture = admitted(
        "support-other-mission-ruling",
        vec!["Gates pass".to_owned()],
    );
    let (other, proposal) = proposal_of_another_mission(&fixture);
    let ruling = RulingService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .rule(
            &proposal.digest().value,
            RulingDecision::Approve,
            "Approved for the other Mission.",
            RulingSupersedes::None,
        )
        .expect("the Ruling records");
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Ruling(digest_bytes(&ruling.digest().value))],
    );
    assert_eq!(
        collect_completion(&fixture, digest).expect_err("a Ruling on another Mission's Proposal"),
        CollectorRefusal::RecordForAnotherMission {
            kind: "ruling",
            id: ruling.digest().value.clone(),
            declared_mission: other,
            expected_mission: fixture.mission.id,
        }
    );

    let fixture = admitted(
        "support-other-mission-evaluation",
        vec!["Gates pass".to_owned()],
    );
    let (other, proposal) = proposal_of_another_mission(&fixture);
    let evaluator = &fixture.mission.members[1];
    let evaluation =
        EvaluationService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .record(EvaluationDraft {
                subject: EvaluationSubjectDraft::Proposal(proposal.digest().value.clone()),
                evaluator: Evaluator {
                    resident_id: evaluator.resident_id,
                    configuration_id: evaluator.admission_configuration_id,
                },
                rubric: RubricDeclaration {
                    id: "security-review-v1".to_owned(),
                    version: 1,
                    canonicalizer: "beacon.rubric.v1".to_owned(),
                    sha256: sha256_hex(&[0x20_u8; 32]),
                },
                cited_runs: Vec::new(),
                verdict: Verdict::Meets,
                findings: vec![],
                conflicts: Conflicts::None,
                rationale: "Evaluated for the other Mission.".to_owned(),
                supersedes: EvaluationSupersedes::None,
            })
            .expect("the Evaluation of the other Mission's Proposal records");
    let digest = record_completion_with_support(
        &fixture,
        newest_event(&fixture.store),
        vec![SupportRef::Evaluation(digest_bytes(
            &evaluation.digest().value,
        ))],
    );
    assert_eq!(
        collect_completion(&fixture, digest)
            .expect_err("an Evaluation of another Mission's Proposal"),
        CollectorRefusal::RecordForAnotherMission {
            kind: "evaluation",
            id: evaluation.digest().value.clone(),
            declared_mission: other,
            expected_mission: fixture.mission.id,
        }
    );
}
