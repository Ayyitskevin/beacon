//! Proposal v1 through the real service (ADR 0034).
//!
//! Every record here is assembled by [`Record`], a byte-level builder that follows the published
//! grammar rather than calling Beacon's encoder. That independence is the point: a canonicalizer
//! that quietly changed its layout would agree with itself and disagree with this file.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, ContextComparison, ContextMismatch, ContextMismatchReason,
    DeclaredCheck, InconsistentProposal, LocalModelActivationService, LocalModelEndpoint,
    MalformedProposal, MissionDraft, MissionMemberDraft, MissionService, OperatorAccountError,
    OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft, ProjectDraft,
    ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService, ProjectService,
    ProposalActor, ProposalError, ProposalService, ProviderFeature, ReadableContext,
    RepositoryDraft, RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService,
    SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    Supervisor, SupervisorError, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement, UnmetPrecondition,
};
use data_encoding::{BASE64, HEXLOWER};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A fabricated Repository Binding. Neither the path nor the commit id names anything that exists,
/// and the patterned revision is meant to be unmistakably synthetic.
const REPOSITORY_SOURCE: &str = "/home/operator/projects/sample-tracker";
const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

const MAGIC: &[u8] = b"beacon.proposal.v1\0";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let output = digest.finalize();
    let mut value = [0_u8; 32];
    value.copy_from_slice(&output[..]);
    value
}

// ---------------------------------------------------------------------------------------------
// An independent encoder for the published grammar
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Context {
    NotDeclared,
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
    Bound {
        kind: String,
        version: u32,
        canonicalizer: String,
        sha256: [u8; 32],
    },
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
            Context::NotDeclared => out.push(0x01),
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
                Constraint::Bound {
                    kind,
                    version,
                    canonicalizer,
                    sha256,
                } => {
                    out.push(0x02);
                    push_string(&mut out, kind);
                    out.extend_from_slice(&version.to_be_bytes());
                    push_string(&mut out, canonicalizer);
                    out.extend_from_slice(sha256);
                }
            }
        }
        for bound in self.bounds {
            out.extend_from_slice(&bound.to_be_bytes());
        }
        out.push(self.budget_tag);
        out.push(self.rollback_tag);
        out
    }

    fn digest(&self) -> String {
        HEXLOWER.encode(&sha256(&self.bytes()))
    }
}

// ---------------------------------------------------------------------------------------------
// One admitted Mission, built the way an Operator would build it
// ---------------------------------------------------------------------------------------------

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-proposal-{label}-{}.sqlite3",
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
    proposals: ProposalService,
}

fn tags_response() -> String {
    format!(
        "{{\"models\":[{{\"name\":\"{MODEL_NAME}\",\"model\":\"{MODEL_NAME}\",\
         \"modified_at\":\"2026-08-14T14:38:20.13373148-04:00\",\"size\":17741872154,\
         \"digest\":\"{MODEL_DIGEST}\",\"details\":{{\"format\":\"gguf\",\"family\":\"qwen35\",\
         \"families\":[\"qwen35\"],\"parameter_size\":\"27.3B\",\
         \"quantization_level\":\"Q4_K_M\"}}}}]}}"
    )
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

fn declared_checks() -> Vec<DeclaredCheck> {
    vec![
        DeclaredCheck {
            name: "cargo-test".to_owned(),
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--workspace".to_owned(),
            ],
            working_directory: ".".to_owned(),
        },
        DeclaredCheck {
            name: "ruff".to_owned(),
            argv: vec!["ruff".to_owned(), "check".to_owned(), ".".to_owned()],
            working_directory: ".".to_owned(),
        },
    ]
}

fn policy_draft() -> ProjectPolicyDraft {
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
    }
}

/// Builds a Workspace with one Project, one Policy revision, one Repository revision, and one
/// admitted Mission whose roster holds two Residents at their exact admission Configurations.
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
    ProjectPolicyService::new(Arc::clone(&accounts), workspace_id)
        .publish(project.id, project.charter.revision, None, policy_draft())
        .unwrap();
    ProjectRepositoryService::new(Arc::clone(&accounts), workspace_id)
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: REPOSITORY_SOURCE.to_owned(),
                base_revision: BASE_REVISION.to_owned(),
                declared_checks: declared_checks(),
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
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
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
            success_conditions: vec!["Relevant gates pass".to_owned()],
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

    Fixture {
        store,
        proposals: ProposalService::new(Arc::clone(&accounts), workspace_id),
        accounts,
        workspace_id,
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configurations[0].resident_id,
        configuration_id: configurations[0].id,
        other_resident_id: configurations[1].resident_id,
        other_configuration_id: configurations[1].id,
    }
}

impl Fixture {
    /// The truthful record for this Mission: every reference resolves, every list is canonical.
    fn record(&self) -> Record {
        Record {
            magic: MAGIC.to_vec(),
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            resident_id: self.resident_id,
            configuration_id: self.configuration_id,
            bootstrap_tag: 0x01,
            bootstrap_reason: "The Owner selected this Resident to request this one Proposal."
                .to_owned(),
            charter_revision: 1,
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

    fn count(&self, sql: &str) -> i64 {
        self.store
            .connect()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .unwrap()
    }

    fn proposal_rows(&self) -> i64 {
        self.count("SELECT count(*) FROM proposals")
    }

    fn proposal_events(&self) -> i64 {
        self.count("SELECT count(*) FROM events WHERE kind = 'proposal.submitted'")
    }

    /// Bind the Project's Repository to a new revision, returning that revision number.
    fn rebind_repository(
        &self,
        prior: u32,
        source_kind: RepositorySourceKind,
        source: &str,
        base_revision: &str,
    ) -> u32 {
        ProjectRepositoryService::new(Arc::clone(&self.accounts), self.workspace_id)
            .bind(
                self.project_id,
                1,
                Some(prior),
                RepositoryDraft {
                    source_kind,
                    source: source.to_owned(),
                    base_revision: base_revision.to_owned(),
                    declared_checks: declared_checks(),
                },
            )
            .unwrap()
            .revision
    }

    /// A second Project in this SAME Workspace and store, with its own Charter, Policy,
    /// Repository, Team and admitted Mission, and the same Resident on its roster.
    ///
    /// Building it here rather than through `admitted_mission` is the whole point: that helper
    /// creates an independent store, so a Mission id taken from it is merely unknown to this one
    /// and never reaches the Mission-to-Project comparison. Sharing the Resident means every
    /// other referent check passes, leaving that comparison as the only thing that can refuse a
    /// Proposal naming this Project's Mission under the other Project's scope.
    fn second_project_mission(&self) -> (Uuid, Uuid) {
        let project = ProjectService::new(Arc::clone(&self.accounts), self.workspace_id)
            .create(ProjectDraft {
                name: "Beacon Alpha II".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Hold a second Mission in the same Workspace.".to_owned(),
                    desired_outcomes: vec!["Be a distinct Project".to_owned()],
                    constraints: vec!["Keep authority explicit".to_owned()],
                    prohibitions: vec!["Do not deploy".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
        ProjectPolicyService::new(Arc::clone(&self.accounts), self.workspace_id)
            .publish(project.id, project.charter.revision, None, policy_draft())
            .unwrap();
        ProjectRepositoryService::new(Arc::clone(&self.accounts), self.workspace_id)
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source: REPOSITORY_SOURCE.to_owned(),
                    base_revision: BASE_REVISION.to_owned(),
                    declared_checks: declared_checks(),
                },
            )
            .unwrap();
        let team = TeamService::new(Arc::clone(&self.accounts), self.workspace_id)
            .form(TeamDraft {
                project_id: project.id,
                name: "Second Mission Team".to_owned(),
                members: vec![TeamMemberDraft {
                    resident_id: self.resident_id,
                    expected_configuration_id: self.configuration_id,
                }],
            })
            .unwrap();
        let mission = MissionService::new(Arc::clone(&self.accounts), self.workspace_id)
            .admit(MissionDraft {
                project_id: project.id,
                expected_charter_revision: project.charter.revision,
                team_id: team.id,
                desired_outcome: "Ship a second reviewed Beacon feature.".to_owned(),
                success_conditions: vec!["Relevant gates pass".to_owned()],
                constraints: vec!["Keep the change reversible".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                members: vec![MissionMemberDraft {
                    resident_id: self.resident_id,
                    expected_configuration_id: self.configuration_id,
                }],
            })
            .unwrap();
        (project.id, mission.id)
    }

    /// Reopens the store through the real startup path, which is where reconciliation runs.
    fn restart(&self) -> Result<(), SupervisorError> {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .map(|_| ())
    }

    /// Plants state the service refuses to write, then puts the immutability triggers back.
    ///
    /// Restoring them is not tidiness: startup verifies the schema signature before it verifies
    /// any state, so a store left with two triggers missing refuses startup for the wrong reason
    /// and a reconciliation test passes without ever reaching reconciliation.
    fn tamper(&self, statements: &str) {
        let connection = self.store.connect();
        // Recreate from the store's own recorded text, not a copy of it: the schema signature
        // compares that text exactly, so a re-indented copy is a different schema.
        let mut statement = connection
            .prepare(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger' AND tbl_name = 'proposals' ORDER BY name",
            )
            .unwrap();
        let triggers = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(statement);
        assert_eq!(triggers.len(), 2);

        connection
            .execute_batch(
                "DROP TRIGGER proposals_no_update;
                 DROP TRIGGER proposals_no_delete;",
            )
            .unwrap();
        connection.execute_batch(statements).unwrap();
        for trigger in triggers {
            connection.execute_batch(&trigger).unwrap();
        }
    }
}

/// A real Git repository on disk, so the readable-context comparison inspects a real object
/// store rather than a fabricated path. Tests may run `git`; the code under test may not.
struct TempRepository {
    path: PathBuf,
}

impl TempRepository {
    fn new(label: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("beacond-proposal-repo-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn git(&self, arguments: &[&str]) {
        let status = Command::new("git")
            .args(arguments)
            .current_dir(&self.path)
            .env("GIT_AUTHOR_NAME", "Beacon Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Beacon Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .status()
            .expect("git is available in the test environment");
        assert!(status.success(), "git {arguments:?} failed");
    }

    /// One commit whose `link.rs` is a symlink, so a declared entry can name a path the walk
    /// resolves to something that is not a regular file.
    fn with_symlink(label: &str) -> (Self, String) {
        let repository = Self::new(label);
        repository.git(&["init", "--quiet"]);
        fs::create_dir_all(repository.path.join("src")).unwrap();
        fs::write(repository.path.join("src/lib.rs"), "fn main() {}\n").unwrap();
        // Sorts before `link.rs`, so a declaration can put a real mismatch ahead of the entry the
        // walk cannot read.
        fs::write(repository.path.join("a.txt"), "first\n").unwrap();
        std::os::unix::fs::symlink("src/lib.rs", repository.path.join("link.rs")).unwrap();
        repository.git(&["add", "-A"]);
        repository.git(&["commit", "--quiet", "-m", "one"]);
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repository.path)
            .output()
            .unwrap();
        let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        (repository, revision)
    }

    /// Initialise the repository with one commit containing `src/lib.rs`, and return its revision.
    fn with_one_commit(label: &str) -> (Self, String) {
        let repository = Self::new(label);
        repository.git(&["init", "--quiet"]);
        fs::create_dir_all(repository.path.join("src")).unwrap();
        fs::write(repository.path.join("src/lib.rs"), "fn main() {}\n").unwrap();
        repository.git(&["add", "."]);
        repository.git(&["commit", "--quiet", "-m", "one"]);
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repository.path)
            .output()
            .unwrap();
        let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        (repository, revision)
    }
}

impl Drop for TempRepository {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn malformed(error: &ProposalError) -> Option<MalformedProposal> {
    match error {
        ProposalError::Malformed(reason) => Some(*reason),
        _ => None,
    }
}

fn inconsistent(error: &ProposalError) -> Option<InconsistentProposal> {
    match error {
        ProposalError::Inconsistent(reason) => Some(*reason),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Submission
// ---------------------------------------------------------------------------------------------

#[test]
fn an_owner_submits_one_proposal_bound_to_the_exact_bytes_it_named() {
    let fixture = admitted_mission("submit");
    let record = fixture.record();
    let bytes = record.bytes();

    let proposal = fixture.proposals.submit(&bytes).unwrap();

    // The independently built bytes are what the service hashed, stored, and returned.
    assert_eq!(proposal.canonical_bytes(), bytes.as_slice());
    assert_eq!(proposal.digest().algorithm, "sha256");
    assert_eq!(proposal.digest().canonicalizer, "beacon.proposal.v1");
    assert_eq!(proposal.digest().value, record.digest());
    assert_eq!(proposal.actor(), ProposalActor::Owner);

    let payload = proposal.payload();
    assert_eq!(payload.scope.workspace_id, fixture.workspace_id);
    assert_eq!(payload.scope.project_id, fixture.project_id);
    assert_eq!(payload.scope.mission_id, fixture.mission_id);
    assert_eq!(payload.requester.resident_id, fixture.resident_id);
    assert_eq!(
        payload.requester.resident_configuration_id,
        fixture.configuration_id
    );
    assert_eq!(payload.bindings.pinned_base_revision, BASE_REVISION);
    assert_eq!(payload.intent.target_paths.len(), 2);
    assert_eq!(
        payload.intent.declared_checks,
        vec![declared_checks()[0].clone()]
    );

    assert_eq!(fixture.proposal_rows(), 1);
    assert_eq!(fixture.proposal_events(), 1);
    assert_eq!(fixture.proposals.get(&record.digest()).unwrap(), proposal);
    assert_eq!(
        fixture
            .proposals
            .list_for_mission(fixture.mission_id)
            .unwrap(),
        vec![proposal]
    );
}

#[test]
fn the_audit_event_carries_the_exact_bytes_as_padded_base64_and_the_owner_actor() {
    let fixture = admitted_mission("event");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    let payload: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'proposal.submitted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let encoded = value["proposal"]["canonicalBytesBase64"].as_str().unwrap();
    assert_eq!(encoded, BASE64.encode(&record.bytes()));
    assert_eq!(BASE64.decode(encoded.as_bytes()).unwrap(), record.bytes());
    assert_eq!(value["proposal"]["actor"], "owner");
    assert_eq!(value["proposal"]["digest"]["value"], record.digest());
    assert_eq!(
        value["proposal"]["digest"]["canonicalizer"],
        "beacon.proposal.v1"
    );
}

#[test]
fn submission_creates_no_run_receipt_worktree_or_external_effect() {
    let fixture = admitted_mission("inert");
    let before_runs = fixture.count("SELECT count(*) FROM runs");
    let before_receipts = fixture.count("SELECT count(*) FROM receipts");
    let before_security = fixture.count("SELECT count(*) FROM security_events");
    let before_events = fixture.count("SELECT count(*) FROM events");

    // The service was constructed with no model endpoint of any kind, so there is nothing for it
    // to contact even if it wanted to.
    fixture.proposals.submit(&fixture.record().bytes()).unwrap();

    assert_eq!(fixture.count("SELECT count(*) FROM runs"), before_runs);
    assert_eq!(
        fixture.count("SELECT count(*) FROM receipts"),
        before_receipts
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM security_events"),
        before_security
    );
    // Exactly one new Event, and it is the Proposal's own.
    assert_eq!(
        fixture.count("SELECT count(*) FROM events"),
        before_events + 1
    );
    assert_eq!(fixture.proposal_events(), 1);
    // Submitting a Proposal records no decision about it. Migration 0019 gives Rulings a table, so
    // the honest assertion is now that the table stays EMPTY rather than that it is absent -- which
    // is the stronger claim, because an absent table cannot be written to by accident and a present
    // one can.
    assert_eq!(fixture.count("SELECT count(*) FROM rulings"), 0);
    // Model Invocation and capability state still have no tables at all. There is nothing to count
    // because nothing was built, which is the honest form of this assertion until they exist.
    for table in ["invocations", "capabilities"] {
        let exists: i64 = fixture
            .store
            .connect()
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            exists, 0,
            "{table} exists; this slice must not have built it"
        );
    }
}

#[test]
fn identical_resubmission_returns_the_original_record_and_writes_no_second_event() {
    let fixture = admitted_mission("idempotent");
    let bytes = fixture.record().bytes();

    let first = fixture.proposals.submit(&bytes).unwrap();
    let second = fixture.proposals.submit(&bytes).unwrap();

    assert_eq!(first, second);
    // The original submission time survives: a resubmission that refreshed it would be a second
    // recording wearing the first one's digest.
    assert_eq!(first.submitted_at_ms(), second.submitted_at_ms());
    assert_eq!(fixture.proposal_rows(), 1);
    assert_eq!(fixture.proposal_events(), 1);
}

#[test]
fn a_stored_digest_that_does_not_name_its_bytes_is_refused_not_matched() {
    let fixture = admitted_mission("collision");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    // Force the collision the store is supposed to make impossible: this digest now names
    // different bytes. Idempotence keyed on the digest alone would hand the caller these.
    let mut other = record.clone();
    other.summary = "A different Proposal wearing another Proposal's digest.".to_owned();
    fixture.tamper(&format!(
        "UPDATE proposals SET canonical_bytes = x'{}' WHERE digest_value = '{}'",
        HEXLOWER.encode(&other.bytes()),
        record.digest()
    ));

    let error = fixture.proposals.submit(&record.bytes()).unwrap_err();
    assert!(matches!(error, ProposalError::DigestCollision), "{error:?}");
    assert!(matches!(
        fixture.proposals.get(&record.digest()).unwrap_err(),
        ProposalError::InvalidStore
    ));
    let error = fixture.restart().unwrap_err();
    assert!(
        matches!(
            error,
            SupervisorError::ProposalEventMismatch(_)
                | SupervisorError::ProposalStateMismatch { .. }
                // R28.1 tier 2: the owner-account check is about the STORE, not one row,
                // so it is now `Totals` rather than a row refusal that names nothing.
                | SupervisorError::ProposalTotalsMismatch(_)
        ),
        "{error:?} / {error}"
    );
}

#[test]
fn unauthenticated_submission_of_oversize_malformed_bytes_returns_only_the_account_refusal() {
    let fixture = admitted_mission("locked");
    fixture.accounts.lock().unwrap();

    // Both oversize and malformed. An implementation that checked the size cap or ran the decoder
    // before authenticating would answer Oversize or Malformed and hand an unauthenticated caller
    // a parser oracle.
    let error = fixture
        .proposals
        .submit(&vec![0x00; 70 * 1024])
        .unwrap_err();

    assert!(
        matches!(
            error,
            ProposalError::Account(OperatorAccountError::NotAuthenticated)
        ),
        "{error:?}"
    );
    assert!(malformed(&error).is_none());
    assert!(!matches!(error, ProposalError::Oversize));
    assert_eq!(fixture.proposal_rows(), 0);
    assert_eq!(fixture.proposal_events(), 0);
}

#[test]
fn an_oversize_slice_is_refused_before_it_is_parsed() {
    let fixture = admitted_mission("oversize");
    let mut record = fixture.record();
    record.summary = "x".repeat(70 * 1024);

    let error = fixture.proposals.submit(&record.bytes()).unwrap_err();

    assert!(matches!(error, ProposalError::Oversize), "{error:?}");
    assert_eq!(fixture.proposal_rows(), 0);
    assert_eq!(fixture.proposal_events(), 0);

    // The boundary itself: 64 KiB exactly is not oversize, so the refusal is the size rule rather
    // than an off-by-one that would also refuse legitimate records.
    let mut at_limit = fixture.record();
    let overhead = at_limit.bytes().len() - at_limit.summary.len();
    at_limit.summary = "x".repeat(64 * 1024 - overhead);
    assert_eq!(at_limit.bytes().len(), 64 * 1024);
    assert!(!matches!(
        fixture.proposals.submit(&at_limit.bytes()).unwrap_err(),
        ProposalError::Oversize
    ));
}

#[test]
fn malformed_bytes_are_refused_with_neither_row_nor_event() {
    let fixture = admitted_mission("malformed");
    let base = fixture.record();

    let mut wrong_magic = base.clone();
    wrong_magic.magic = b"beacon.proposal.v2\0".to_vec();
    let mut trailing = base.bytes();
    trailing.push(0x00);
    let mut unsorted = base.clone();
    unsorted.targets.reverse();
    unsorted.mutations.reverse();
    let mut bad_manifest = base.clone();
    bad_manifest.context = Context::Declared {
        entries: vec![("src/lib.rs".to_owned(), [0x66; 32])],
        manifest: Some([0x00; 32]),
    };
    let mut bound = base.clone();
    bound.requirements[1] = Constraint::Bound {
        kind: "landlock".to_owned(),
        version: 1,
        canonicalizer: "beacon.sandbox.v1".to_owned(),
        sha256: [0x88; 32],
    };
    let mut escaping = base.clone();
    escaping.targets = vec!["../outside.rs".to_owned()];
    escaping.mutations = vec![(0x02, "../outside.rs".to_owned())];
    let mut budget = base.bytes();
    let last = budget.len() - 2;
    budget[last] = 0x02;

    let cases: Vec<(&str, Vec<u8>, MalformedProposal)> = vec![
        ("magic", wrong_magic.bytes(), MalformedProposal::Magic),
        ("trailing byte", trailing, MalformedProposal::TrailingBytes),
        (
            "unsorted targets",
            unsorted.bytes(),
            MalformedProposal::NonCanonicalOrder,
        ),
        (
            "mismatched manifest",
            bad_manifest.bytes(),
            MalformedProposal::ManifestMismatch,
        ),
        (
            "bound descriptor",
            bound.bytes(),
            MalformedProposal::UnsupportedBoundDescriptor,
        ),
        (
            "escaping path",
            escaping.bytes(),
            MalformedProposal::TargetPath,
        ),
        ("budget tag", budget, MalformedProposal::UnknownTag),
    ];
    for (name, bytes, expected) in cases {
        let error = fixture.proposals.submit(&bytes).unwrap_err();
        assert_eq!(malformed(&error), Some(expected), "{name}: {error:?}");
    }
    assert_eq!(fixture.proposal_rows(), 0);
    assert_eq!(fixture.proposal_events(), 0);
}

#[test]
fn not_declared_and_declared_empty_are_two_proposals_not_one() {
    let fixture = admitted_mission("context-states");
    let mut absent = fixture.record();
    absent.context = Context::NotDeclared;
    let mut empty = fixture.record();
    empty.context = Context::Declared {
        entries: Vec::new(),
        manifest: None,
    };
    assert_ne!(absent.digest(), empty.digest());

    let first = fixture.proposals.submit(&absent.bytes()).unwrap();
    let second = fixture.proposals.submit(&empty.bytes()).unwrap();

    assert_eq!(first.digest().value, absent.digest());
    assert_eq!(second.digest().value, empty.digest());
    assert!(matches!(
        first.payload().intent.readable_context,
        ReadableContext::NotDeclared
    ));
    match &second.payload().intent.readable_context {
        ReadableContext::Declared { entries, .. } => assert!(entries.is_empty()),
        other => panic!("expected a positive empty declaration, got {other:?}"),
    }
    assert_eq!(fixture.proposal_rows(), 2);
    assert_eq!(fixture.proposal_events(), 2);
}

// ---------------------------------------------------------------------------------------------
// Referent existence and scope
// ---------------------------------------------------------------------------------------------

#[test]
fn structurally_inconsistent_references_write_neither_row_nor_event() {
    let fixture = admitted_mission("references");
    let base = fixture.record();

    let mut foreign_workspace = base.clone();
    foreign_workspace.workspace_id = Uuid::new_v4();

    // The Mission exists and the Project exists; only their relationship is a lie. A validator
    // that confirmed each identity separately would accept this substitution.
    let mut foreign_project = base.clone();
    foreign_project.project_id = Uuid::new_v4();

    let mut absent_mission = base.clone();
    absent_mission.mission_id = Uuid::new_v4();

    let mut stranger = base.clone();
    stranger.resident_id = Uuid::new_v4();

    // A real admitted member, but paired with the other member's Configuration.
    let mut wrong_configuration = base.clone();
    wrong_configuration.configuration_id = fixture.other_configuration_id;

    let mut wrong_policy = base.clone();
    wrong_policy.policy_revision = 2;

    let mut wrong_repository = base.clone();
    wrong_repository.repository_revision = 2;

    let mut wrong_base = base.clone();
    wrong_base.base_revision = "b1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_owned();

    // A check whose name is bound but whose argument vector is not.
    let mut wrong_check = base.clone();
    wrong_check.checks = vec![(
        "cargo-test".to_owned(),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".".to_owned(),
    )];

    let cases: Vec<(&str, Record, InconsistentProposal)> = vec![
        (
            "foreign workspace",
            foreign_workspace,
            InconsistentProposal::WorkspaceScope,
        ),
        (
            "absent project",
            foreign_project,
            InconsistentProposal::ProjectNotFound,
        ),
        (
            "absent mission",
            absent_mission,
            InconsistentProposal::MissionNotInProject,
        ),
        (
            "unadmitted resident",
            stranger,
            InconsistentProposal::ResidentNotAdmitted,
        ),
        (
            "another member's configuration",
            wrong_configuration,
            InconsistentProposal::ConfigurationNotAdmission,
        ),
        (
            "absent policy revision",
            wrong_policy,
            InconsistentProposal::PolicyNotForCharter,
        ),
        (
            "absent repository revision",
            wrong_repository,
            InconsistentProposal::RepositoryNotForCharter,
        ),
        (
            "wrong pinned base",
            wrong_base,
            InconsistentProposal::PinnedBaseRevisionMismatch,
        ),
        (
            "check that is not bound",
            wrong_check,
            InconsistentProposal::DeclaredChecksNotBound,
        ),
    ];
    for (name, record, expected) in cases {
        let error = fixture.proposals.submit(&record.bytes()).unwrap_err();
        assert_eq!(inconsistent(&error), Some(expected), "{name}: {error:?}");
        assert_eq!(fixture.proposal_rows(), 0, "{name} wrote a row");
        assert_eq!(fixture.proposal_events(), 0, "{name} wrote an Event");
    }
}

#[test]
fn a_mission_from_another_project_in_the_same_workspace_is_refused() {
    // ADR 0034 carries Project and Mission redundantly so "a cross-Workspace or cross-Project
    // substitution is independently detectable". Detecting it needs a Mission that genuinely
    // EXISTS in this Workspace and belongs to a DIFFERENT Project -- a Mission id from a separate
    // store is merely unknown, and takes the absent-Mission branch without ever comparing the two
    // Projects.
    let fixture = admitted_mission("cross-project");
    let (second_project, second_mission) = fixture.second_project_mission();
    assert_ne!(second_project, fixture.project_id);

    // Scope names Project A; the Mission belongs to Project B. Everything else resolves: the
    // Mission exists, the requesting Resident is on its admission roster at that exact
    // Configuration, and Project A's own Policy and Repository bindings are the ones named.
    let mut record = fixture.record();
    record.mission_id = second_mission;
    let error = fixture.proposals.submit(&record.bytes()).unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentProposal::MissionNotInProject),
        "{error:?}"
    );
    assert_eq!(fixture.proposal_rows(), 0);
    assert_eq!(fixture.proposal_events(), 0);

    // The mirror case: naming Project B's scope with Project B's Mission is consistent, so the
    // refusal above is the Project comparison and not something incidental to the second Project.
    let mut consistent = fixture.record();
    consistent.project_id = second_project;
    consistent.mission_id = second_mission;
    fixture.proposals.submit(&consistent.bytes()).unwrap();
    assert_eq!(fixture.proposal_rows(), 1);
}

#[test]
fn a_second_admitted_member_may_request_its_own_proposal() {
    let fixture = admitted_mission("second-member");
    let mut record = fixture.record();
    record.resident_id = fixture.other_resident_id;
    record.configuration_id = fixture.other_configuration_id;

    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert_eq!(
        proposal.payload().requester.resident_id,
        fixture.other_resident_id
    );
    assert_ne!(proposal.digest().value, fixture.record().digest());
}

#[test]
fn a_declared_check_argument_longer_than_an_invented_cap_is_accepted_when_it_matches_a_real_binding()
 {
    // S4 (gpt2 non-author review). Repository Binding (project_repository.rs:398-423) accepts
    // any non-empty, NUL-free argument with no length limit -- only the check name is capped, at
    // 64 bytes, matching this grammar's own MAX_CHECK_NAME_BYTES. A field limit invented here
    // that Repository Binding does not share would silently narrow the exact-byte match Proposal
    // v1 must be able to make: a bound Declared Check with a long argument could never appear in
    // ANY Proposal, even one comfortably inside the ADR's 64 KiB envelope.
    let fixture = admitted_mission("long-declared-check-argument");
    // 4,097 bytes: one byte past the removed invented cap, and nowhere near 64 KiB.
    let long_argument = "-".repeat(4_097);
    let oversized_check = DeclaredCheck {
        name: "cargo-test".to_owned(),
        argv: vec!["cargo".to_owned(), "test".to_owned(), long_argument.clone()],
        working_directory: ".".to_owned(),
    };
    ProjectRepositoryService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .bind(
            fixture.project_id,
            1,
            Some(1),
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: REPOSITORY_SOURCE.to_owned(),
                base_revision: BASE_REVISION.to_owned(),
                declared_checks: vec![oversized_check],
            },
        )
        .unwrap();

    let mut record = fixture.record();
    record.repository_revision = 2;
    record.checks = vec![(
        "cargo-test".to_owned(),
        vec!["cargo".to_owned(), "test".to_owned(), long_argument],
        ".".to_owned(),
    )];
    let bytes = record.bytes();
    assert!(
        bytes.len() < 64 * 1024,
        "fixture stayed well inside the envelope"
    );

    let proposal = fixture.proposals.submit(&bytes).unwrap();

    assert_eq!(proposal.payload().intent.declared_checks[0].argv.len(), 3);
    assert_eq!(
        proposal.payload().intent.declared_checks[0].argv[2].len(),
        4_097
    );
}

// ---------------------------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------------------------

#[test]
fn get_refuses_a_digest_that_names_nothing_and_list_is_scoped_to_its_mission() {
    let fixture = admitted_mission("reads");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(matches!(
        fixture.proposals.get(&"0".repeat(64)).unwrap_err(),
        ProposalError::NotFound
    ));
    assert!(
        fixture
            .proposals
            .list_for_mission(Uuid::new_v4())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .proposals
            .list_for_mission(fixture.mission_id)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_tampered_context_manifest_is_rejected_by_get_list_and_startup() {
    let fixture = admitted_mission("tampered-manifest");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    // The same nested mismatch submit refuses, planted after the fact. Validating only on the way
    // in would let this read back as a clean Proposal.
    let mut tampered = record.clone();
    tampered.context = Context::Declared {
        entries: vec![("src/lib.rs".to_owned(), [0x66; 32])],
        manifest: Some([0x01; 32]),
    };
    fixture.tamper(&format!(
        "UPDATE proposals SET canonical_bytes = x'{}', digest_value = '{}' \
         WHERE digest_value = '{}'",
        HEXLOWER.encode(&tampered.bytes()),
        tampered.digest(),
        record.digest()
    ));

    assert!(matches!(
        fixture.proposals.get(&tampered.digest()).unwrap_err(),
        ProposalError::InvalidStore
    ));
    assert!(matches!(
        fixture
            .proposals
            .list_for_mission(fixture.mission_id)
            .unwrap_err(),
        ProposalError::InvalidStore
    ));
    let error = fixture.restart().unwrap_err();
    assert!(
        matches!(
            error,
            SupervisorError::ProposalEventMismatch(_)
                | SupervisorError::ProposalStateMismatch { .. }
                // R28.1 tier 2: the owner-account check is about the STORE, not one row,
                // so it is now `Totals` rather than a row refusal that names nothing.
                | SupervisorError::ProposalTotalsMismatch(_)
        ),
        "{error:?}"
    );
}

#[test]
fn proposal_rows_refuse_update_and_delete() {
    let fixture = admitted_mission("immutable");
    fixture.proposals.submit(&fixture.record().bytes()).unwrap();
    let connection = fixture.store.connect();

    assert!(
        connection
            .execute("UPDATE proposals SET bootstrap_reason = 'other'", [])
            .is_err()
    );
    assert!(connection.execute("DELETE FROM proposals", []).is_err());
    assert_eq!(fixture.proposal_rows(), 1);
}

// ---------------------------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------------------------

#[test]
fn a_fresh_v1_proposal_reports_exactly_its_undeclared_preconditions() {
    let fixture = admitted_mission("readiness");
    let declared = fixture.proposals.submit(&fixture.record().bytes()).unwrap();

    // All seven constraint families can only be not_declared in v1, so every v1 Proposal is
    // unready by construction. Nothing here is a boolean, and nothing may read as permission.
    //
    // The eighth item is the readable-context comparison ADR 0034:166-170 requires. This fixture
    // declares src/lib.rs against a fabricated repository path, so the comparison could not be
    // performed and says so. An earlier revision of this test asserted a list with no context
    // status at all, which is what let a declared context read as though nothing needed saying.
    assert_eq!(
        declared.readiness().unmet(),
        [
            UnmetPrecondition::ExecutionPolicyNotDeclared,
            UnmetPrecondition::SandboxNotDeclared,
            UnmetPrecondition::NetworkNotDeclared,
            UnmetPrecondition::ModelProviderNotDeclared,
            UnmetPrecondition::CommandAllowlistNotDeclared,
            UnmetPrecondition::ToolAllowlistNotDeclared,
            UnmetPrecondition::EvaluationsNotDeclared,
            UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::RepositoryUnreadable
            },
        ]
    );

    let mut absent = fixture.record();
    absent.context = Context::NotDeclared;
    let undeclared = fixture.proposals.submit(&absent.bytes()).unwrap();
    assert_eq!(
        undeclared.readiness().unmet(),
        [
            UnmetPrecondition::ExecutionPolicyNotDeclared,
            UnmetPrecondition::SandboxNotDeclared,
            UnmetPrecondition::NetworkNotDeclared,
            UnmetPrecondition::ModelProviderNotDeclared,
            UnmetPrecondition::CommandAllowlistNotDeclared,
            UnmetPrecondition::ToolAllowlistNotDeclared,
            UnmetPrecondition::EvaluationsNotDeclared,
            UnmetPrecondition::ReadableContextNotDeclared,
        ]
    );
}

#[test]
fn a_declared_context_over_an_unopenable_store_is_not_verified_as_unreadable() {
    // ADR 0034:166-170 -- "Internal manifest consistency does not prove that a path/digest pair
    // existed at the pinned base revision... No surface may label the context verified from the
    // manifest's self-consistency alone." The wrong answer this kills is SILENCE: a Proposal that
    // declares src/lib.rs, submits, and reports a readiness list carrying no comparison status at
    // all -- which is what shipped before, and reads as "nothing to say about the context".
    let fixture = admitted_mission("context-not-verified");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(matches!(
        proposal.payload().intent.readable_context,
        ReadableContext::Declared { .. }
    ));
    // The fixture's bound source is a fabricated path, so the object store cannot be opened.
    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::RepositoryUnreadable
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
    // And it is never reported as merely undeclared: those are different facts.
    assert!(
        !proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotDeclared)
    );
}

#[test]
fn a_git_url_repository_reports_that_there_is_nothing_local_to_compare() {
    // The wrong answer: treating a Git URL as "unavailable" (which invites a retry) or, worse,
    // fetching it. Neither. It is a distinct reason -- there is no local revision to read.
    let fixture = admitted_mission("context-git-url");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::GitUrl,
        "https://example.invalid/beacon.git",
        BASE_REVISION,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::RepositorySourceNotLocal
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_pinned_revision_absent_from_a_real_object_store_is_reported_as_absent_not_unavailable() {
    // A REAL repository that opens cleanly, pinned to a commit it does not contain. The wrong
    // answer is collapsing this into RepositoryUnavailable: the store was read successfully, and
    // what is missing is the revision the entries would have been compared against.
    let fixture = admitted_mission("context-revision-absent");
    let (repository, _real_revision) = TempRepository::with_one_commit("absent");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        // A syntactically valid commit id that this repository does not hold.
        "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00",
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00".to_owned();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::PinnedRevisionAbsent
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_declared_entry_that_does_not_hold_at_the_pinned_revision_is_a_mismatch_naming_it() {
    // This test used to assert the opposite: that a present pinned revision still reported the
    // content as unreadable, because `beacond` had no decompressor. That was true and is not any
    // more, and the assertion is replaced rather than deleted -- it is the guard that says this
    // slice built the thing the last one promised it would not.
    //
    // The record declares `src/lib.rs` with a fabricated digest, and the repository really holds
    // that path at the pinned commit with different bytes. So the comparison runs, and the wrong
    // answers it kills are both of the ones that matter: silence, which would label the context
    // verified from the manifest's self-consistency alone, and a bare "mismatch" that does not say
    // which entry.
    let fixture = admitted_mission("context-mismatch");
    let (repository, real_revision) = TempRepository::with_one_commit("present");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextMismatch {
                entries: vec![ContextMismatch {
                    path: "src/lib.rs".to_owned(),
                    reason: ContextMismatchReason::DigestDiffers,
                }],
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
    // And it is a mismatch, not a not-verified: the difference between a fact about the
    // Repository and a fact about Beacon's reach is the whole point of the two items.
    assert!(
        !proposal
            .readiness()
            .unmet()
            .iter()
            .any(|item| matches!(item, UnmetPrecondition::ReadableContextNotVerified { .. })),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_declared_entry_that_holds_at_the_pinned_revision_adds_no_readiness_item() {
    // The control, and the one outcome that makes the whole slice worth shipping: when the
    // declared digest is the real SHA-256 of the file's bytes at the pinned revision, the
    // comparison agrees and says nothing. Without this, a comparison that reported every entry as
    // mismatched or unverified would pass every other test in this file.
    let fixture = admitted_mission("context-agrees");
    let (repository, real_revision) = TempRepository::with_one_commit("agreeing");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    // `with_one_commit` writes exactly these bytes to `src/lib.rs`.
    record.context = Context::Declared {
        entries: vec![("src/lib.rs".to_owned(), sha256(b"fn main() {}\n"))],
        manifest: None,
    };
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        !proposal.readiness().unmet().iter().any(|item| matches!(
            item,
            UnmetPrecondition::ReadableContextMismatch { .. }
                | UnmetPrecondition::ReadableContextNotVerified { .. }
        )),
        "a comparison that agreed still reported an item: {:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_declared_path_absent_at_the_pinned_revision_is_a_mismatch_not_an_inspection_failure() {
    // The entry claims a file that was not there. Beacon read every tree on the way and proved it,
    // so this is a fact about the Repository -- reporting it as a Beacon-side failure would hide a
    // false declaration behind an excuse.
    let fixture = admitted_mission("context-absent");
    let (repository, real_revision) = TempRepository::with_one_commit("absent");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    record.context = Context::Declared {
        entries: vec![("docs/never-existed.md".to_owned(), [0x77; 32])],
        manifest: None,
    };
    record.targets = vec!["docs/never-existed.md".to_owned()];
    record.mutations = vec![(0x01, "docs/never-existed.md".to_owned())];
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextMismatch {
                entries: vec![ContextMismatch {
                    path: "docs/never-existed.md".to_owned(),
                    reason: ContextMismatchReason::PathAbsent,
                }],
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_declaration_that_fits_the_aggregate_budget_is_read_and_reported_as_agreement() {
    // The Operator-surface half of opus2's F1: a declaration that fits the real 64 MiB allowance
    // must reach agreement, so exhaustion is never reported for an ordinary submission.
    //
    // This test does NOT drive exhaustion, and its earlier name said it did. Submission runs the
    // comparison with the constant, so nothing reachable from here can spend the allowance without
    // a 64 MiB fixture -- gpt2 remapped the exhausted state to `ObjectTooLargeToInspect` and this
    // test stayed green. The reason an Operator reads is guarded one layer down, where the budget
    // is a parameter, by `an_exhausted_aggregate_budget_is_the_reason_an_operator_reads` in
    // `src/proposal.rs`. The two together are the control: this one proves the budget does not
    // refuse what fits, that one proves what does not fit is named correctly.
    let fixture = admitted_mission("context-budget");
    let (repository, real_revision) = TempRepository::with_one_commit("budget");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );
    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    record.context = Context::Declared {
        entries: vec![("src/lib.rs".to_owned(), sha256(b"fn main() {}\n"))],
        manifest: None,
    };
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    // With room, the declared entry agrees and nothing is reported.
    assert!(
        !proposal.readiness().unmet().iter().any(|item| matches!(
            item,
            UnmetPrecondition::ReadableContextMismatch { .. }
                | UnmetPrecondition::ReadableContextNotVerified { .. }
        )),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_packed_repository_is_not_verified_rather_than_mismatched() {
    // After a repack every object on the path is packed, and this reader reads only loose objects.
    // The wrong answer is reporting the declared entry as false because Beacon could not read the
    // object that would have judged it -- the exact shape of defect this whole taxonomy exists to
    // prevent. It is also proof that `PackedObjectUnreadable` is earned rather than assumed: the
    // authenticated pack lookup found the object, so "packed" is a measurement.
    let fixture = admitted_mission("context-packed");
    let (repository, real_revision) = TempRepository::with_one_commit("packed");
    repository.git(&["gc", "--quiet", "--prune=now"]);
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::PackedObjectUnreadable
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn an_object_in_no_storage_at_all_is_an_inspection_failure_not_a_packed_object() {
    // gpt1's S1: "packed" may be claimed only after the authenticated pack lookup says the object
    // is present. Here the blob is referenced by an authenticated tree and lives in neither loose
    // nor pack storage, so the store is broken -- and reporting that as "packed" would excuse a
    // defect as a limitation of Beacon.
    //
    // Without this test the pack lookup can be deleted entirely and every other test still passes,
    // because in all of them the object really is packed.
    let fixture = admitted_mission("context-missing-object");
    let (repository, real_revision) = TempRepository::with_one_commit("missing-object");
    let blob = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD:src/lib.rs"])
            .current_dir(&repository.path)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_owned();
    fs::remove_file(
        repository
            .path
            .join(".git/objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::InspectionFailed
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_mismatch_found_before_an_unreadable_entry_is_discarded_rather_than_published() {
    // gpt1's F3, and the rule that costs the most to get wrong. Entry one genuinely disagrees;
    // entry two cannot be read, because its path is a symlink. The buffered mismatch must be
    // DISCARDED and the whole comparison reported not-verified.
    //
    // The wrong answer is publishing the mismatch anyway. A mismatch is an accusation about the
    // Operator's declaration, and a walk that did not finish has not earned one -- the entry it
    // could not reach might have been the one that explained the other. A not-verified says less
    // and is always true.
    let fixture = admitted_mission("context-buffered");
    let (repository, real_revision) = TempRepository::with_symlink("buffered");
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        &real_revision,
    );

    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = real_revision;
    // Ascending byte order, which the grammar requires: `a.txt` then `link.rs`. The mismatching
    // entry is FIRST, so a real disagreement is buffered before the walk reaches the entry it
    // cannot read. An earlier version of this test put the unreadable entry first, which meant the
    // buffer was empty when the walk stopped -- and a mutant that published buffered mismatches
    // early passed it. The order is the test.
    record.context = Context::Declared {
        entries: vec![
            ("a.txt".to_owned(), [0x66; 32]),
            ("link.rs".to_owned(), [0x66; 32]),
        ],
        manifest: None,
    };
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::EntryIsNotARegularFile
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
    assert!(
        !proposal
            .readiness()
            .unmet()
            .iter()
            .any(|item| matches!(item, UnmetPrecondition::ReadableContextMismatch { .. })),
        "a buffered mismatch was published from an unfinished walk: {:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn an_object_store_inspection_that_errors_is_reported_as_failed_not_absent() {
    // AGENTS.md:152 -- "Terminal, absent, broken, and not evaluated are distinct states." A pack
    // index that cannot be parsed is an inspection that BROKE. The wrong answer is reporting it as
    // PinnedRevisionAbsent: that is a negative answer, and it would tell an Operator the revision
    // is not in the repository when what actually happened is that Beacon could not finish looking.
    let fixture = admitted_mission("context-inspection-failed");
    let (repository, _) = TempRepository::with_one_commit("broken-index");
    // A pack index that is not a version 2 index. The queried revision is not a loose object, so
    // the lookup falls through to the pack scan and has to parse this.
    let pack = repository.path.join(".git/objects/pack");
    fs::create_dir_all(&pack).unwrap();
    fs::write(pack.join("pack-beacon.idx"), b"not a pack index at all").unwrap();

    let absent = "c0ffee00c0ffee00c0ffee00c0ffee00c0ffee00";
    let revision = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        repository.path.to_str().unwrap(),
        absent,
    );
    let mut record = fixture.record();
    record.repository_revision = revision;
    record.base_revision = absent.to_owned();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::InspectionFailed
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
    // Specifically not the negative answer, and not the broken-store reason: the store opened.
    assert!(!proposal.readiness().unmet().contains(
        &UnmetPrecondition::ReadableContextNotVerified {
            reason: ContextComparison::PinnedRevisionAbsent
        }
    ));
    assert!(!proposal.readiness().unmet().contains(
        &UnmetPrecondition::ReadableContextNotVerified {
            reason: ContextComparison::RepositoryUnreadable
        }
    ));
}

#[test]
fn a_git_indirection_chain_is_reported_as_unreadable_not_as_a_failed_inspection() {
    // Guards a documentation claim rather than a behaviour change. Round four found the enum
    // comment offering a `.git` indirection chain as an example of InspectionFailed; it cannot be
    // one, because IndirectionTooDeep comes only from ObjectStore::open, and an open failure is
    // reported before any inspection starts. The wrong answer is the doc's: describing a state the
    // production path cannot reach, which sends a reader debugging the wrong layer.
    let fixture = admitted_mission("context-indirection-chain");
    let (target, revision) = TempRepository::with_one_commit("indirection-target");

    // A source whose `.git` is a FILE pointing at a directory that itself has a `.git` -- one
    // level more indirection than the object store follows.
    let source = std::env::temp_dir().join(format!("beacond-indirect-{}", Uuid::new_v4()));
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join(".git"),
        format!("gitdir: {}\n", target.path.display()),
    )
    .unwrap();

    let bound = fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        source.to_str().unwrap(),
        &revision,
    );
    let mut record = fixture.record();
    record.repository_revision = bound;
    record.base_revision = revision;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotVerified {
                reason: ContextComparison::RepositoryUnreadable
            }),
        "{:?}",
        proposal.readiness().unmet()
    );
    assert!(!proposal.readiness().unmet().contains(
        &UnmetPrecondition::ReadableContextNotVerified {
            reason: ContextComparison::InspectionFailed
        }
    ));
    fs::remove_dir_all(&source).unwrap();
}

#[test]
fn a_not_declared_context_reports_only_that_it_is_undeclared() {
    // The comparison applies to a DECLARED context. An undeclared one has no entries to compare,
    // so it must keep reporting exactly what it reported before -- the wrong answer here is a
    // NotVerified item appearing on a Proposal that declared nothing, which would read as a failed
    // check rather than an absent declaration.
    let fixture = admitted_mission("context-not-declared");
    let mut record = fixture.record();
    record.context = Context::NotDeclared;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    assert!(
        proposal
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::ReadableContextNotDeclared)
    );
    assert!(
        !proposal
            .readiness()
            .unmet()
            .iter()
            .any(|item| matches!(item, UnmetPrecondition::ReadableContextNotVerified { .. })),
        "{:?}",
        proposal.readiness().unmet()
    );
}

#[test]
fn a_new_repository_revision_with_the_same_base_reports_only_the_revision_as_moved() {
    // ADR 0034 treats "Repository" and "base" as separate facts in its readiness list
    // (":220-228"), so a Repository rebind that keeps the same pinned commit must not also claim
    // the base moved -- that is a second, false diagnosis riding on the first true one.
    let fixture = admitted_mission("repository-moved-same-base");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    ProjectRepositoryService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .bind(
            fixture.project_id,
            1,
            Some(1),
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                // A different fabricated source, same pinned commit -- only the revision moved.
                source: "/home/operator/projects/sample-tracker-relocated".to_owned(),
                base_revision: BASE_REVISION.to_owned(),
                declared_checks: declared_checks(),
            },
        )
        .unwrap();

    let after = fixture.proposals.get(&record.digest()).unwrap();
    assert!(
        after
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::RepositoryRevisionMoved),
        "{:?}",
        after.readiness().unmet()
    );
    assert!(
        !after
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::BaseRevisionMoved),
        "same base falsely reported as moved: {:?}",
        after.readiness().unmet()
    );
}

#[test]
fn a_new_charter_revision_makes_get_report_a_moved_binding_and_startup_still_succeeds() {
    let fixture = admitted_mission("moved-charter");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();

    ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .revise_charter(
            fixture.project_id,
            1,
            ProjectCharterDraft {
                purpose: "Prove the local agent office, revised.".to_owned(),
                desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        )
        .unwrap();

    let after = fixture.proposals.get(&record.digest()).unwrap();
    assert!(
        after
            .readiness()
            .unmet()
            .contains(&UnmetPrecondition::CharterRevisionMoved)
    );
    // The record itself did not change: staleness is derived, never stored.
    assert_eq!(after.canonical_bytes(), record.bytes().as_slice());
    assert_eq!(after.digest().value, record.digest());
    // And a historical Proposal does not become corrupt merely because a current revision moved.
    fixture.restart().unwrap();
}

// ---------------------------------------------------------------------------------------------
// Startup reconciliation
//
// "Refuses startup" is concrete here: `Supervisor::bootstrap` returns an error instead of a
// `StartupReport`, so no Workspace identity and no service is produced against that store.
// ---------------------------------------------------------------------------------------------

fn submitted(label: &str) -> (Fixture, Record) {
    let fixture = admitted_mission(label);
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();
    (fixture, record)
}

fn assert_refuses_startup(fixture: &Fixture, case: &str) {
    let error = fixture.restart().unwrap_err();
    assert!(
        matches!(
            error,
            SupervisorError::ProposalEventMismatch(_)
                | SupervisorError::ProposalStateMismatch { .. }
                // R28.1 tier 2: the owner-account check is about the STORE, not one row,
                // so it is now `Totals` rather than a row refusal that names nothing.
                | SupervisorError::ProposalTotalsMismatch(_)
        ),
        "{case}: {error:?} / {error}"
    );
}

#[test]
fn a_truthful_store_starts_and_reconciles() {
    // The control the refusals are measured against: without it, a reconciliation that refused
    // every store would pass every test below.
    let (fixture, record) = submitted("startup-clean");

    fixture.restart().unwrap();

    assert_eq!(
        fixture
            .proposals
            .get(&record.digest())
            .unwrap()
            .digest()
            .value,
        record.digest()
    );
}

#[test]
fn startup_refuses_a_proposal_without_its_event() {
    let (fixture, _) = submitted("startup-missing-event");
    fixture
        .store
        .connect()
        .execute("DELETE FROM events WHERE kind = 'proposal.submitted'", [])
        .unwrap();

    assert_refuses_startup(&fixture, "missing event");
}

#[test]
fn startup_refuses_a_duplicate_proposal_event() {
    let (fixture, _) = submitted("startup-duplicate-event");
    // A byte-identical second Event. Reconciliation that only asked "is there an Event?" would
    // accept two recordings of one submission.
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, payload_json, occurred_at_ms
             FROM events WHERE kind = 'proposal.submitted'",
            [],
        )
        .unwrap();

    assert_refuses_startup(&fixture, "duplicate event");
}

#[test]
fn startup_refuses_an_orphan_proposal_event() {
    let (fixture, _) = submitted("startup-orphan-event");
    fixture.tamper("DELETE FROM proposals");

    assert_refuses_startup(&fixture, "orphan event");
}

#[test]
fn startup_refuses_altered_proposal_bytes() {
    let (fixture, record) = submitted("startup-altered-bytes");
    let mut altered = record.clone();
    altered.summary = "A summary the recorded digest never covered.".to_owned();
    fixture.tamper(&format!(
        "UPDATE proposals SET canonical_bytes = x'{}'",
        HEXLOWER.encode(&altered.bytes())
    ));

    assert_refuses_startup(&fixture, "altered bytes");
}

#[test]
fn startup_refuses_an_indexed_column_that_disagrees_with_the_bytes() {
    let (fixture, _) = submitted("startup-column-drift");
    // The bytes are untouched and the digest still names them. Only the column lies. A
    // reconciliation that trusted the columns, or that never re-decoded the bytes to compare,
    // would start cleanly on a store whose index disagrees with its own record.
    fixture.tamper("UPDATE proposals SET bootstrap_reason = 'A reason nobody submitted.'");

    assert_refuses_startup(&fixture, "column drift");
}

#[test]
fn startup_refuses_a_differently_encoded_event_payload() {
    let (fixture, record) = submitted("startup-unpadded-base64");
    let payload = format!(
        "{{\"proposal\":{{\"digest\":{{\"algorithm\":\"sha256\",\
         \"canonicalizer\":\"beacon.proposal.v1\",\"value\":\"{}\"}},\
         \"canonicalBytesBase64\":\"{}\",\"actor\":\"owner\",\"submittedAtMs\":{}}}}}",
        record.digest(),
        // The same bytes, encoded without padding: canonical RFC 4648 padded base64 is the
        // recorded form, and evidence written another way is different evidence.
        data_encoding::BASE64_NOPAD.encode(&record.bytes()),
        fixture
            .store
            .connect()
            .query_row(
                "SELECT occurred_at_ms FROM events WHERE kind = 'proposal.submitted'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    );
    fixture
        .store
        .connect()
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE kind = 'proposal.submitted'",
            [payload],
        )
        .unwrap();

    assert_refuses_startup(&fixture, "unpadded base64");
}

#[test]
fn startup_refuses_an_event_whose_bytes_are_not_the_stored_bytes() {
    let (fixture, record) = submitted("startup-event-bytes");
    let mut other = record.clone();
    other.summary = "Evidence for a Proposal that was never stored.".to_owned();
    let payload = format!(
        "{{\"proposal\":{{\"digest\":{{\"algorithm\":\"sha256\",\
         \"canonicalizer\":\"beacon.proposal.v1\",\"value\":\"{}\"}},\
         \"canonicalBytesBase64\":\"{}\",\"actor\":\"owner\",\"submittedAtMs\":{}}}}}",
        record.digest(),
        BASE64.encode(&other.bytes()),
        fixture
            .store
            .connect()
            .query_row(
                "SELECT occurred_at_ms FROM events WHERE kind = 'proposal.submitted'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap()
    );
    fixture
        .store
        .connect()
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE kind = 'proposal.submitted'",
            [payload],
        )
        .unwrap();

    assert_refuses_startup(&fixture, "event bytes are not the stored bytes");
}

#[test]
fn startup_and_get_refuse_a_stored_bound_constraint() {
    // ADR 0034: "Until those content and owning-module surfaces exist, the only accepted value
    // is not_declared." Submission enforces that; this proves the common verified-read path used
    // by `get`, `list_for_mission`, and startup enforces it too, on a row that is otherwise
    // perfectly coherent -- correct digest, correct bytes, correct Event, correct columns. Only
    // the constraint tag is something submission would have refused.
    let (fixture, record) = submitted("startup-bound-constraint");
    let mut bound = record.clone();
    bound.requirements = (0..7)
        .map(|index| Constraint::Bound {
            kind: format!("descriptor-{index}"),
            version: 1,
            canonicalizer: "beacon.sandbox.v1".to_owned(),
            sha256: [0x99; 32],
        })
        .collect();
    let bytes = bound.bytes();
    let digest = bound.digest();
    assert_ne!(digest, record.digest());

    // The row and its Event are rewritten together so every OTHER check still passes: the new
    // digest names the new bytes, the columns (none of which encode requirements) still match,
    // and the Event's digest and base64 payload match the new bytes exactly.
    fixture.tamper(&format!(
        "UPDATE proposals SET canonical_bytes = x'{}', digest_value = '{}' \
         WHERE digest_value = '{}'",
        HEXLOWER.encode(&bytes),
        digest,
        record.digest()
    ));
    let occurred_at_ms: i64 = fixture
        .store
        .connect()
        .query_row(
            "SELECT occurred_at_ms FROM events WHERE kind = 'proposal.submitted'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let payload = format!(
        "{{\"proposal\":{{\"digest\":{{\"algorithm\":\"sha256\",\
         \"canonicalizer\":\"beacon.proposal.v1\",\"value\":\"{digest}\"}},\
         \"canonicalBytesBase64\":\"{}\",\"actor\":\"owner\",\"submittedAtMs\":{occurred_at_ms}}}}}",
        BASE64.encode(&bytes)
    );
    fixture
        .store
        .connect()
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE kind = 'proposal.submitted'",
            [payload],
        )
        .unwrap();

    // Prove the plant is otherwise coherent: bytes hash to the new digest, and it decodes.
    assert_eq!(HEXLOWER.encode(&sha256(&bytes)), digest);

    let get_error = fixture.proposals.get(&digest).unwrap_err();
    assert!(
        matches!(get_error, ProposalError::InvalidStore),
        "{get_error:?}"
    );

    let list_error = fixture
        .proposals
        .list_for_mission(fixture.mission_id)
        .unwrap_err();
    assert!(
        matches!(list_error, ProposalError::InvalidStore),
        "{list_error:?}"
    );

    assert_refuses_startup(&fixture, "stored bound constraint");
}

#[test]
fn startup_refuses_a_missing_event_on_one_proposal_balanced_by_a_duplicate_on_another() {
    // T1 (gpt2 non-author review). The per-Proposal event_count check and the total-count check
    // are redundant with only one Proposal in the store -- either alone catches a missing or
    // duplicate Event. With TWO Proposals, deleting A's Event and duplicating B's leaves the
    // total count unchanged (1 removed, 1 added) while A has zero matching Events and B has two.
    // Only the per-Proposal check can tell those two states apart from a healthy store.
    let fixture = admitted_mission("balanced-missing-and-duplicate");
    let a = fixture.record();
    let mut b = fixture.record();
    b.resident_id = fixture.other_resident_id;
    b.configuration_id = fixture.other_configuration_id;
    fixture.proposals.submit(&a.bytes()).unwrap();
    fixture.proposals.submit(&b.bytes()).unwrap();
    assert_eq!(fixture.proposal_rows(), 2);
    assert_eq!(fixture.proposal_events(), 2);

    fixture.tamper(&format!(
        "DELETE FROM events WHERE kind = 'proposal.submitted'
           AND json_extract(payload_json, '$.proposal.digest.value') = '{}';
         INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         SELECT workspace_id, kind, payload_json, occurred_at_ms
         FROM events
         WHERE kind = 'proposal.submitted'
           AND json_extract(payload_json, '$.proposal.digest.value') = '{}';",
        a.digest(),
        b.digest()
    ));
    // The corruption is balanced: still 2 rows, still 2 Events in total.
    assert_eq!(fixture.proposal_rows(), 2);
    assert_eq!(fixture.proposal_events(), 2);

    assert_refuses_startup(&fixture, "balanced missing-and-duplicate Events");
}

#[test]
fn startup_refuses_when_the_owner_account_is_gone() {
    let (fixture, _) = submitted("startup-no-owner");
    // A record whose authorization basis is `owner` is only truthful while that Owner exists.
    fixture
        .store
        .connect()
        .execute("DELETE FROM operator_accounts", [])
        .unwrap();

    assert_refuses_startup(&fixture, "no owner account");
}

/// R28 tier 2 pass 3 of 14: a Proposal row the log does not account for is refused at startup, and
/// the refusal names WHICH Proposal and carries the reader's own fault.
///
/// Pass 1's plant shape: the event replay catches every column tamper first, so the plant is a row
/// with no Event of its own. The planted digest is all `f`, so it sorts last in the pass's own
/// `ORDER BY` by construction (R39.3), and the column list comes from `pragma_table_info` so the
/// plant does not go stale when a column is added.
#[test]
fn startup_refuses_an_unaccounted_proposal_row_and_names_the_row() {
    // R44: BOTH ENDS. A planted digest of all `f` sorts LAST in this pass's own
    // ORDER BY and all `0` sorts FIRST -- the extremes of the hex space, so the
    // placement holds against any honest digest. Tampering only one end makes
    // "names the failing row" and "names that end" the same assertion.
    for (end, fill) in [("last", "f"), ("first", "0")] {
        let fixture = admitted_mission(&format!("r28t2-proposal-startup-{end}"));
        fixture.proposals.submit(&fixture.record().bytes()).unwrap();

        let connection = fixture.store.connect();
        let existing: String = connection
            .query_row(
                "SELECT digest_value FROM proposals ORDER BY digest_value LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the fixture recorded a Proposal");
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('proposals')")
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
                    "INSERT INTO proposals ({names}) SELECT {values} FROM proposals
                 WHERE digest_value = ?2"
                ),
                rusqlite::params![planted, existing],
            )
            .expect("the planted row inserts");
        // R44.4 (astra): the endpoint is CONSTRUCTED, not read back, so assert it -- one ordered
        // read per iteration, the same query the pass runs. Fixed-width lowercase hex makes all
        // `f`/all `0` the extremes today; a migration that changed the digest encoding or the
        // column collation would silently stop bracketing and this bar would decay to one end
        // without failing.
        {
            let order = if end == "last" { "DESC" } else { "ASC" };
            let placed: String = connection
                .query_row(
                    &format!("SELECT digest_value FROM proposals ORDER BY workspace_id {order}, digest_value {order} LIMIT 1"),
                    [],
                    |row| row.get(0),
                )
                .expect("the plant is in the table");
            assert_eq!(
                placed, planted,
                "{end}: the planted row is not at the {end} end of this pass's own ORDER BY"
            );
        }

        drop(connection);

        // The belt refuses it ON READ, so the planted fault is real and has a name of its own.
        let read = fixture
            .proposals
            .get(&planted)
            .expect_err("the reader trusted a row whose bytes do not hash to its digest");
        assert!(
            matches!(
                read,
                ProposalError::InvalidStore | ProposalError::Inconsistent(_)
            ),
            "the planted fault is not the one this test is about: {read:?}"
        );

        let refusal = fixture
            .restart()
            .expect_err("a store holding an unaccounted Proposal row reopened Ready");
        assert!(
            matches!(
                &refusal,
                SupervisorError::ProposalStateMismatch {
                    workspace_id,
                    digest_value,
                    ..
                } if *workspace_id == fixture.workspace_id && digest_value == &planted
            ),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );
        // R28.4: the key and the fault's NAME. `name()` is `pub(crate)`, so this bar reads the name
        // out of the line itself and checks `Debug` agrees -- which is the property that matters: the
        // one-line message names a fault the Debug channel can be opened on.
        let line = refusal.to_string();
        assert!(
            line.contains(&planted) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        let named = line
            .rsplit(": ")
            .next()
            .expect("the line ends with the fault's name")
            .to_owned();
        assert!(
            !named.is_empty() && !named.contains(' ') && !named.contains('{'),
            "the tail of the line is a rendering, not a name: {line}"
        );
        assert!(
            format!("{refusal:?}").contains(&named),
            "the named fault is not reachable through Debug: {refusal:?}"
        );
        // kimi's P3-2: pin the fault, and assert the carried TEXT is absent -- the form the
        // Evaluation and Artifact bars use. Reading the name out of the line proves it is
        // name-SHAPED; this proves it is the RIGHT name and that no reader sentence rode in with it.
        assert_eq!(
            named, "InvalidStore",
            "the planted row's fault is not the one this bar pins: {line}"
        );
        assert!(
            !line.contains("the stored digest is not the hash"),
            "a reader's own sentence reached the one-line message: {line}"
        );
    }
}
