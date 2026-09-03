//! Dispatch gates through the real service (ADR 0039), against real Git repositories.
//!
//! **Nothing here can apply a change**: this build has no landing path, so the best outcome a
//! dispatch reaches is `Admissible`. That is what makes the file worth having. BLUEPRINT `:907`
//! says "a denied or stale Proposal causes zero repository mutation", and a negative criterion is
//! proved most honestly against a version that cannot mutate anything at all.
//!
//! The row-34 tests are gpt1's four probe skeletons, and they assert on **repository bytes** rather
//! than on Beacon's own records: [`raw_repository_snapshot`] walks the whole repository root
//! including `.git`, runs no Git, and follows no symlink. Its own control proves it notices a single
//! changed byte of the index, of a loose object, and of worktree metadata -- without which every
//! zero-mutation assertion here would be weaker than it reads.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, CandidateImages, CandidateService, DeclaredCheck,
    DispatchOutcome, DispatchService, LocalModelActivationService, LocalModelEndpoint,
    MissionDraft, MissionMemberDraft, MissionService, MutationEffect, OperatorAccountService,
    OperatorSessionPhase, PauseService, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProjectPolicyService, ProjectRepositoryService, ProjectService, ProposalService,
    ProviderFeature, RepositoryDraft, RepositorySourceKind, ResidentConfigurationService,
    ResidentLifecycleService, RulingDecision, RulingService, RulingSupersedes, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    SupervisorError, SuppliedContent, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
use data_encoding::HEXLOWER;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A fabricated Repository Binding. Neither the path nor the commit id names anything that exists.
const REPOSITORY_SOURCE: &str = "/home/operator/projects/sample-tracker";
const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

const PROPOSAL_MAGIC: &[u8] = b"beacon.proposal.v1\0";
// The mutation tags of both grammars, written out here rather than imported: a test that took them
// from the code under test could not notice the code renumbering them.
const MUTATION_CREATE: u8 = 0x01;
const MUTATION_MODIFY: u8 = 0x02;
const MUTATION_DELETE: u8 = 0x03;

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let output = digest.finalize();
    let mut value = [0_u8; 32];
    value.copy_from_slice(&output[..]);
    value
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn push_count(out: &mut Vec<u8>, count: usize) {
    out.extend_from_slice(&(count as u32).to_be_bytes());
}

// ---------------------------------------------------------------------------------------------
// Independent encoders for the two published grammars
// ---------------------------------------------------------------------------------------------

/// One truthful `beacon.proposal.v1` record, with the two knobs this file needs: the summary,
/// which is what makes a second Proposal in the same Mission a different Proposal, and the
/// declared mutations a Candidate must mirror one for one.
struct ProposalRecord {
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    summary: String,
    /// The Repository Binding revision and pinned base this Proposal names. Parameterised here
    /// because a dispatch fixture binds a real repository at a real commit, unlike the synthetic
    /// binding the other files use.
    repository_revision: u32,
    charter_revision: u32,
    policy_revision: u32,
    base_revision: String,
    /// The SHA-256 the declared readable context claims `src/lib.rs` holds at the pinned base.
    /// Real here, unlike the synthetic fixtures elsewhere: a dispatch gate refuses a Proposal whose
    /// readable context does not hold, so a fixture with a patterned digest never reaches the live
    /// repository gates at all.
    context_digest: [u8; 32],
    /// `(kind tag, target)` in the caller's intended order. The grammar stores target paths
    /// ascending and the mutation list in this order, so a record whose two orders differ is what
    /// proves a Candidate follows the mutations rather than the sorted paths.
    mutations: Vec<(u8, String)>,
}

/// The three-kind default: create, modify, delete, declared in an order the sorted path list does
/// not reproduce.
fn default_mutations() -> Vec<(u8, String)> {
    vec![
        (MUTATION_MODIFY, "src/lib.rs".to_owned()),
        (MUTATION_DELETE, "src/old.rs".to_owned()),
        (MUTATION_CREATE, "docs/new.md".to_owned()),
    ]
}

#[allow(
    dead_code,
    reason = "the harness carries the whole builder; this file uses part of it"
)]
impl ProposalRecord {
    fn bytes(&self) -> Vec<u8> {
        let mut out = PROPOSAL_MAGIC.to_vec();
        for id in [
            self.workspace_id,
            self.project_id,
            self.mission_id,
            self.resident_id,
            self.configuration_id,
        ] {
            out.extend_from_slice(id.as_bytes());
        }
        out.push(0x01);
        push_string(
            &mut out,
            "The Owner selected this Resident to request this one Proposal.",
        );
        for revision in [
            self.charter_revision,
            self.policy_revision,
            self.repository_revision,
        ] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, &self.base_revision);
        push_string(&mut out, &self.summary);

        // Readable context: declared, one entry, manifest over the encoded entry list.
        out.push(0x02);
        let mut entries = Vec::new();
        push_count(&mut entries, 1);
        push_string(&mut entries, "src/lib.rs");
        entries.extend_from_slice(&self.context_digest);
        out.extend_from_slice(&entries);
        out.extend_from_slice(&sha256(&entries));

        // Target paths: ascending and unique, as the grammar requires. Derived from the mutation
        // list so the two cannot fall out of agreement in this builder.
        let mut targets: Vec<&str> = self
            .mutations
            .iter()
            .map(|(_, path)| path.as_str())
            .collect();
        targets.sort_unstable();
        targets.dedup();
        push_count(&mut out, targets.len());
        for target in &targets {
            push_string(&mut out, target);
        }
        push_count(&mut out, self.mutations.len());
        for (kind, target) in &self.mutations {
            out.push(*kind);
            push_string(&mut out, target);
        }
        push_count(&mut out, 1);
        push_string(&mut out, "cargo-test");
        push_count(&mut out, 1);
        push_string(&mut out, "cargo-test");
        push_count(&mut out, 3);
        for argument in ["cargo", "test", "--workspace"] {
            push_string(&mut out, argument);
        }
        push_string(&mut out, ".");

        // Seven constraint families, each `not_declared`.
        out.extend_from_slice(&[0x01; 7]);
        for bound in [300_000_u64, 240, 2 << 30, 256 << 20, 1 << 20, 0] {
            out.extend_from_slice(&bound.to_be_bytes());
        }
        out.push(0x01);
        out.push(0x01);
        out
    }

    fn digest(&self) -> String {
        HEXLOWER.encode(&sha256(&self.bytes()))
    }
}

struct TestStore {
    path: PathBuf,
}

#[allow(
    dead_code,
    reason = "the harness carries the whole builder; this file uses part of it"
)]
impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-candidate-{label}-{}.sqlite3",
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
    proposals: ProposalService,
    candidates: CandidateService,
    rulings: RulingService,
    dispatches: DispatchService,
    pause: PauseService,
    copies_root: PathBuf,
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
    vec![DeclaredCheck {
        name: "cargo-test".to_owned(),
        argv: vec![
            "cargo".to_owned(),
            "test".to_owned(),
            "--workspace".to_owned(),
        ],
        working_directory: ".".to_owned(),
    }]
}

fn policy_draft(minimum_count: u32) -> ProjectPolicyDraft {
    ProjectPolicyDraft {
        team_lead_selection: TeamLeadSelectionPolicy {
            required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
            availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
            track_record_requirements: vec![TrackRecordRequirement {
                evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                minimum_count,
            }],
            conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
            ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
        },
    }
}

fn charter_draft(purpose: &str) -> ProjectCharterDraft {
    ProjectCharterDraft {
        purpose: purpose.to_owned(),
        desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
        constraints: vec!["Keep authority explicit".to_owned()],
        prohibitions: vec!["Do not deploy".to_owned()],
        autonomy_profile: AutonomyProfile::Supervised,
    }
}

/// A Workspace with one Project, one Policy revision, one Repository revision, one admitted
/// Mission, and no Candidate yet.
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
            charter: charter_draft("Prove the local agent office."),
        })
        .unwrap();
    ProjectPolicyService::new(Arc::clone(&accounts), workspace_id)
        .publish(project.id, project.charter.revision, None, policy_draft(1))
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
            vec![StarterResidentDraft {
                role: StarterRole::Lead,
                display_name: None,
            }],
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

    let configuration = ResidentConfigurationService::new(Arc::clone(&accounts), workspace_id)
        .configure(
            residents[0].id,
            None,
            activation.id,
            vec![
                BeaconCapability::WorkCreateMission,
                BeaconCapability::WorkCoordinateMission,
            ],
        )
        .unwrap();
    let team = TeamService::new(Arc::clone(&accounts), workspace_id)
        .form(TeamDraft {
            project_id: project.id,
            name: "Mission Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id: configuration.resident_id,
                expected_configuration_id: configuration.id,
            }],
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
            members: vec![MissionMemberDraft {
                resident_id: configuration.resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();

    // Beacon's own repository-copy root, beside the store the way `SampleProjectService` places
    // its managed tree. v1 dispatch reaches nothing outside it.
    let copies_root = store
        .path
        .parent()
        .unwrap()
        .join(format!("beacon-copies-{}", Uuid::new_v4()));
    fs::create_dir_all(&copies_root).unwrap();

    Fixture {
        proposals: ProposalService::new(Arc::clone(&accounts), workspace_id),
        candidates: CandidateService::new(Arc::clone(&accounts), workspace_id),
        rulings: RulingService::new(Arc::clone(&accounts), workspace_id),
        dispatches: DispatchService::new(Arc::clone(&accounts), workspace_id, &copies_root),
        pause: PauseService::new(Arc::clone(&accounts)),
        copies_root,
        store,
        accounts,
        workspace_id,
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configuration.resident_id,
        configuration_id: configuration.id,
    }
}

#[allow(
    dead_code,
    reason = "the harness carries the whole fixture; this file uses part of it"
)]
impl Fixture {
    fn proposal(&self, summary: &str) -> ProposalRecord {
        ProposalRecord {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            resident_id: self.resident_id,
            configuration_id: self.configuration_id,
            summary: summary.to_owned(),
            repository_revision: 1,
            charter_revision: 1,
            policy_revision: 1,
            base_revision: BASE_REVISION.to_owned(),
            context_digest: [0x66; 32],
            mutations: default_mutations(),
        }
    }

    /// Submit one Proposal and return the digest that names its exact bytes.
    fn submit(&self, summary: &str) -> String {
        let record = self.proposal(summary);
        let proposal = self.proposals.submit(&record.bytes()).unwrap();
        assert_eq!(proposal.digest().value, record.digest());
        record.digest()
    }

    fn count(&self, sql: &str) -> i64 {
        self.store
            .connect()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .unwrap()
    }

    fn ruling_rows(&self) -> i64 {
        self.count("SELECT count(*) FROM rulings")
    }

    fn ruling_events(&self) -> i64 {
        self.count("SELECT count(*) FROM events WHERE kind = 'ruling.recorded'")
    }

    fn candidate_bindings(&self) -> i64 {
        self.count("SELECT count(*) FROM ruling_candidates")
    }

    fn restart(&self) -> Result<(), SupervisorError> {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .map(|_| ())
    }

    /// Plants state the service refuses to write, then puts the immutability triggers back.
    fn tamper(&self, statements: &str) {
        self.without_immutability_triggers(|connection| {
            connection.execute_batch(statements).unwrap();
        });
    }

    /// Run `f` with the Candidate immutability triggers dropped, then put them back.
    ///
    /// Restoring them is not tidiness: startup verifies the schema signature before it verifies
    /// any state, so a store left short a trigger refuses startup for the wrong reason and a
    /// reconciliation test passes without ever reaching reconciliation.
    ///
    /// `f` returns its own result rather than unwrapping inside the drop window, which an earlier
    /// version of this helper got wrong. A statement the store refused panicked before the restore
    /// ran, the triggers stayed off, and the NEXT call's setup then panicked for an unrelated
    /// reason -- which a test looking for a refusal happily counted as one. Two guards read green
    /// that way.
    fn without_immutability_triggers<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let connection = self.store.connect();
        let mut statement = connection
            .prepare(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND tbl_name IN ('rulings', 'ruling_supersessions', 'ruling_candidates')
                 ORDER BY name",
            )
            .unwrap();
        let triggers = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(statement);
        assert_eq!(triggers.len(), 6);

        connection
            .execute_batch(
                "DROP TRIGGER rulings_no_update;
                 DROP TRIGGER rulings_no_delete;
                 DROP TRIGGER ruling_supersessions_no_update;
                 DROP TRIGGER ruling_supersessions_no_delete;
                 DROP TRIGGER ruling_candidates_no_update;
                 DROP TRIGGER ruling_candidates_no_delete;",
            )
            .unwrap();
        let outcome = f(&connection);
        for trigger in triggers {
            connection.execute_batch(&trigger).unwrap();
        }
        outcome
    }
}

// ---------------------------------------------------------------------------------------------
// Measuring "zero repository mutation" over raw bytes
// ---------------------------------------------------------------------------------------------

/// Every byte of a repository, as one digest.
///
/// gpt1's helper, and the reason it replaced a five-part fingerprint: that one hashed object
/// *names* and left out `.git/index`, reflogs, pseudorefs, config, worktree metadata, and untracked
/// content -- and the head-and-cleanliness gate was measured changing the index while it stayed
/// equal. This walks the whole root, **runs no Git**, follows no symlink, sorts by raw `OsStr`
/// bytes, and hashes each entry's relative name, file type, mode, symlink target, length and full
/// contents.
///
/// It establishes final byte identity. It is not evidence that no file was transiently created.
fn raw_repository_snapshot(root: &Path) -> String {
    fn walk(root: &Path, current: &Path, entries: &mut Vec<(Vec<u8>, PathBuf)>) {
        let mut children: Vec<_> = fs::read_dir(current)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        children.sort_by(|left, right| {
            left.as_os_str()
                .as_bytes()
                .cmp(right.as_os_str().as_bytes())
        });
        for child in children {
            let relative = child.strip_prefix(root).unwrap().to_owned();
            entries.push((relative.as_os_str().as_bytes().to_vec(), child.clone()));
            let metadata = fs::symlink_metadata(&child).unwrap();
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                walk(root, &child, entries);
            }
        }
    }

    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries.sort_by(|left, right| left.0.cmp(&right.0));

    let mut digest = Sha256::new();
    for (name, path) in entries {
        let metadata = fs::symlink_metadata(&path).unwrap();
        digest.update((name.len() as u64).to_be_bytes());
        digest.update(&name);
        let kind: u8 = if metadata.file_type().is_symlink() {
            2
        } else if metadata.is_dir() {
            1
        } else {
            0
        };
        digest.update([kind]);
        digest.update(metadata.mode().to_be_bytes());
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&path).unwrap();
            let target = target.as_os_str().as_bytes();
            digest.update((target.len() as u64).to_be_bytes());
            digest.update(target);
        } else if metadata.is_file() {
            let bytes = fs::read(&path).unwrap();
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(&bytes);
        }
    }
    HEXLOWER.encode(&digest.finalize()[..])
}

/// A Git repository Beacon made, with one commit, inside the copies root.
struct CopyRepository {
    path: PathBuf,
    revision: String,
}

impl CopyRepository {
    fn git(path: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(path)
            .env("GIT_AUTHOR_NAME", "Beacon Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Beacon Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()
            .expect("git is available in the test environment");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// One commit holding the file every Candidate in this file modifies.
    fn create(root: &Path, label: &str) -> Self {
        let path = root.join(format!("copy-{label}"));
        fs::create_dir_all(path.join("src")).unwrap();
        fs::write(path.join("src/lib.rs"), "fn main() {}\n").unwrap();
        fs::write(path.join("src/old.rs"), "fn old() {}\n").unwrap();
        Self::git(&path, &["init", "--quiet", "--initial-branch=main"]);
        Self::git(&path, &["add", "-A"]);
        Self::git(&path, &["commit", "--quiet", "-m", "one"]);
        let revision = Self::git(&path, &["rev-parse", "HEAD"]);
        Self { path, revision }
    }

    /// Add a second commit and leave the branch on it: the head has moved on.
    fn commit_again(&self) -> String {
        fs::write(self.path.join("src/lib.rs"), "fn main() { /* moved */ }\n").unwrap();
        Self::git(&self.path, &["add", "-A"]);
        Self::git(&self.path, &["commit", "--quiet", "-m", "two"]);
        Self::git(&self.path, &["rev-parse", "HEAD"])
    }

    fn snapshot(&self) -> String {
        raw_repository_snapshot(&self.path)
    }
}

// ---------------------------------------------------------------------------------------------
// Arranging a dispatchable decision
// ---------------------------------------------------------------------------------------------

const APPROVE_REASON: &str = "The declared effects stay inside the Repository and spend nothing.";
/// The one mutation every fixture here declares: modify `src/lib.rs`.
const TARGET: &str = "src/lib.rs";
const NEW_BYTES: &[u8] = b"fn main() { /* dispatched */ }\n";
/// What `src/lib.rs` holds at a fresh copy's first commit, and after its second.
const BASE_LIB_BYTES: &[u8] = b"fn main() {}\n";
const MOVED_LIB_BYTES: &[u8] = b"fn main() { /* moved */ }\n";

fn images_for(pre: &[u8], post: &[u8]) -> Vec<CandidateImages> {
    vec![CandidateImages {
        pre_image_sha256: Some(sha256(pre)),
        post_image_sha256: Some(sha256(post)),
    }]
}

fn content_for(bytes: &[u8]) -> Vec<SuppliedContent> {
    vec![SuppliedContent {
        path: TARGET.to_owned(),
        bytes: Some(bytes.to_vec()),
    }]
}

impl Fixture {
    /// A Proposal bound to `repository` at its current head, declaring one modification of
    /// `src/lib.rs`, with its Candidate recorded.
    fn candidate_for(
        &self,
        repository: &CopyRepository,
        summary: &str,
        base_bytes: &[u8],
    ) -> (String, String) {
        let revision = self.rebind_repository(repository);
        let mut record = self.proposal(summary);
        record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
        record.repository_revision = revision;
        record.base_revision = repository.revision.clone();
        record.context_digest = sha256(base_bytes);
        let proposal = self.proposals.submit(&record.bytes()).unwrap();
        let candidate = self
            .candidates
            .record(&proposal.digest().value, &images_for(base_bytes, NEW_BYTES))
            .unwrap();
        (
            proposal.digest().value.clone(),
            candidate.digest().value.clone(),
        )
    }

    /// [`Fixture::candidate_for`] at a Charter revision other than the first.
    fn candidate_at_charter(
        &self,
        repository: &CopyRepository,
        summary: &str,
        base_bytes: &[u8],
        charter_revision: u32,
    ) -> (String, String) {
        let revision = self.rebind_repository_at(repository, charter_revision);
        let mut record = self.proposal(summary);
        record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
        record.repository_revision = revision;
        record.charter_revision = charter_revision;
        record.policy_revision = charter_revision;
        record.base_revision = repository.revision.clone();
        record.context_digest = sha256(base_bytes);
        let proposal = self.proposals.submit(&record.bytes()).unwrap();
        let candidate = self
            .candidates
            .record(&proposal.digest().value, &images_for(base_bytes, NEW_BYTES))
            .unwrap();
        (
            proposal.digest().value.clone(),
            candidate.digest().value.clone(),
        )
    }

    /// Bind the Project to this repository at its current head, returning the new revision.
    fn rebind_repository(&self, repository: &CopyRepository) -> u32 {
        self.rebind_repository_at(repository, 1)
    }

    fn rebind_repository_at(&self, repository: &CopyRepository, charter_revision: u32) -> u32 {
        let repositories =
            ProjectRepositoryService::new(Arc::clone(&self.accounts), self.workspace_id);
        let previous = repositories
            .history(self.project_id)
            .unwrap()
            .last()
            .map(|binding| binding.revision);
        repositories
            .bind(
                self.project_id,
                charter_revision,
                previous,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source: repository.path.to_string_lossy().into_owned(),
                    base_revision: repository.revision.clone(),
                    declared_checks: declared_checks(),
                },
            )
            .unwrap()
            .revision
    }

    /// The whole arrangement a dispatch needs: a Beacon-made copy, a Proposal, its Candidate, and
    /// an approving v2 Ruling that names it.
    fn approved(&self, label: &str) -> (CopyRepository, String) {
        let repository = CopyRepository::create(&self.copies_root, label);
        let (proposal_digest, _candidate) = self.candidate_for(
            &repository,
            "Apply the dispatched change to the library root.",
            BASE_LIB_BYTES,
        );
        let ruling = self
            .rulings
            .rule_on_candidate(
                &self.candidates.present(&proposal_digest).unwrap(),
                RulingDecision::Approve,
                APPROVE_REASON,
                RulingSupersedes::None,
            )
            .unwrap();
        (repository, ruling.digest().value.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// The helper's own control
// ---------------------------------------------------------------------------------------------

#[test]
fn the_snapshot_notices_one_changed_byte_of_index_object_or_worktree_metadata() {
    // Without this, every zero-mutation assertion below is weaker than it reads. The fingerprint
    // this replaced hashed object NAMES and missed `.git/index` entirely, so the gate that rewrote
    // the index looked like it had changed nothing.
    let root = std::env::temp_dir().join(format!("beacon-snapshot-control-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let repository = CopyRepository::create(&root, "control");
    let before = repository.snapshot();
    assert_eq!(repository.snapshot(), before, "the snapshot is not stable");

    // One byte of the index.
    let index = repository.path.join(".git/index");
    let mut bytes = fs::read(&index).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    fs::write(&index, &bytes).unwrap();
    assert_ne!(
        repository.snapshot(),
        before,
        "a changed index byte was invisible"
    );
    bytes[last] ^= 0x01;
    fs::write(&index, &bytes).unwrap();
    assert_eq!(repository.snapshot(), before);

    // One loose object.
    let objects = repository.path.join(".git/objects");
    let loose = fs::read_dir(&objects)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.is_dir() && path.file_name().unwrap().len() == 2)
        .expect("a fresh repository has loose objects");
    let object = fs::read_dir(&loose)
        .unwrap()
        .filter_map(Result::ok)
        .next()
        .unwrap()
        .path();
    // Git writes loose objects read-only, so the control restores the mode as well as the bytes:
    // the snapshot hashes the mode too, and leaving it changed would pass for the wrong reason.
    let original_mode = fs::metadata(&object).unwrap().permissions().mode();
    let mut bytes = fs::read(&object).unwrap();
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600)).unwrap();
    bytes.push(0x00);
    fs::write(&object, &bytes).unwrap();
    assert_ne!(
        repository.snapshot(),
        before,
        "a changed object byte was invisible"
    );
    bytes.pop();
    fs::write(&object, &bytes).unwrap();
    fs::set_permissions(&object, fs::Permissions::from_mode(original_mode)).unwrap();
    assert_eq!(repository.snapshot(), before);

    // One byte of worktree metadata: the file every dispatch would rewrite.
    let tracked = repository.path.join("src/lib.rs");
    fs::write(&tracked, "fn main() { }\n").unwrap();
    assert_ne!(
        repository.snapshot(),
        before,
        "a changed worktree byte was invisible"
    );

    fs::remove_dir_all(&root).unwrap();
}

// ---------------------------------------------------------------------------------------------
// Row 34: a denied or stale Proposal causes zero repository mutation
// ---------------------------------------------------------------------------------------------

#[test]
fn row_34_denied_ruling_leaves_every_repository_byte_identical() {
    let fixture = admitted_mission("row34-denied");
    let repository = CopyRepository::create(&fixture.copies_root, "denied");
    let (proposal_digest, _candidate) = fixture.candidate_for(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Refuse,
            "Not this one: the change is not what the Mission asked for.",
            RulingSupersedes::None,
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RulingRefused);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn row_34_stale_head_leaves_every_repository_byte_identical() {
    // The one arrangement that kills two wrong answers at once: the branch is moved BACK to an
    // ancestor, so a deleted head gate would let it through AND a fast-forward merge would accept
    // it. Every stored record still agrees -- the Binding, the Ruling's copied revisions, the
    // Candidate -- so no other gate here can refuse it.
    let fixture = admitted_mission("row34-stale");
    let repository = CopyRepository::create(&fixture.copies_root, "stale");
    let parent = repository.revision.clone();
    let moved = repository.commit_again();
    assert_ne!(parent, moved);

    // Bind and rule at the LATER commit, then move the branch back to its parent.
    let later = CopyRepository {
        path: repository.path.clone(),
        revision: moved.clone(),
    };
    let (proposal_digest, _candidate) = fixture.candidate_for(
        &later,
        "Apply the dispatched change to the library root.",
        MOVED_LIB_BYTES,
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();
    CopyRepository::git(&repository.path, &["reset", "--hard", "--quiet", &parent]);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        parent
    );

    let before = raw_repository_snapshot(&repository.path);
    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RepositoryHeadMoved);
    assert_eq!(receipt.observed_head.as_deref(), Some(parent.as_str()));
    assert_eq!(
        receipt.pinned_base_revision.as_deref(),
        Some(moved.as_str())
    );
    assert_eq!(raw_repository_snapshot(&repository.path), before);
}

#[test]
fn row_34_drifted_ruling_leaves_every_repository_byte_identical() {
    let fixture = admitted_mission("row34-drifted");
    let (repository, ruling_digest) = fixture.approved("drifted");

    // Publish a new Policy revision: the Ruling's copied policy revision is no longer current.
    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .publish(fixture.project_id, 1, Some(1), policy_draft(2))
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RulingDrifted);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn row_34_superseded_ruling_leaves_every_repository_byte_identical() {
    let fixture = admitted_mission("row34-superseded");
    let (repository, first) = fixture.approved("superseded");
    let ruling = fixture.rulings.get(&first).unwrap();
    fixture
        .rulings
        .rule(
            &ruling.payload().proposal.value,
            RulingDecision::Refuse,
            "On reflection, not this change.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&first, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RulingSuperseded);
    assert_eq!(repository.snapshot(), before);
}

// ---------------------------------------------------------------------------------------------
// The rest of the gates
// ---------------------------------------------------------------------------------------------

#[test]
fn every_gate_holding_is_admissible_and_still_applies_nothing() {
    // The positive control without which every refusal above passes against a function that always
    // refuses -- and the honest statement of what this build is: the gates hold, and nothing
    // happens, because the act does not exist yet.
    let fixture = admitted_mission("admissible");
    let (repository, ruling_digest) = fixture.approved("admissible");

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Admissible);
    assert_eq!(
        repository.snapshot(),
        before,
        "an admissible dispatch changed the repository"
    );

    // The Receipt is a measurement of what held, and of what was not established.
    assert_eq!(receipt.version, 3);
    assert_eq!(receipt.autonomy_profile, Some(AutonomyProfile::Supervised));
    assert_eq!(
        receipt.observed_head.as_deref(),
        Some(repository.revision.as_str())
    );
    assert_eq!(receipt.per_mutation.len(), 1);
    assert_eq!(receipt.per_mutation[0].path, TARGET);
    assert_eq!(receipt.per_mutation[0].effect, MutationEffect::NotAttempted);
    assert!(receipt.candidate_digest.is_some());
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry.contains("no landing path")),
        "the Receipt does not say the change was not applied: {:?}",
        receipt.not_verified
    );
    // The seven undeclared constraint families are named on every Receipt, so no record of a
    // dispatch can be read as having checked them.
    assert_eq!(
        receipt
            .not_verified
            .iter()
            .filter(|entry| entry.contains("not declarable in Proposal v1"))
            .count(),
        7
    );
    fixture
        .restart()
        .expect("a recorded dispatch survives a restart");
}

#[test]
fn a_v1_ruling_is_not_dispatchable_because_it_names_no_bytes() {
    // R8's rule, at the gate. Not "because it is old": because nothing in it says which bytes the
    // Operator saw.
    let fixture = admitted_mission("v1-ruling");
    let repository = CopyRepository::create(&fixture.copies_root, "v1-ruling");
    let (proposal_digest, _) = fixture.candidate_for(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
    );
    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::CandidateAbsent);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn dispatch_refuses_while_the_installation_is_paused() {
    // BLUEPRINT:377-379 and AGENTS.md:128 -- a paused Installation refuses new dispatches. A gate,
    // never a line in `not_verified`.
    let fixture = admitted_mission("observe-and-pause");
    let (repository, ruling_digest) = fixture.approved("observe-and-pause");
    let before = repository.snapshot();

    fixture.pause.set_paused(true).unwrap();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::InstallationPaused);
    assert_eq!(repository.snapshot(), before);

    fixture.pause.set_paused(false).unwrap();
    assert_eq!(
        fixture
            .dispatches
            .dispatch(&ruling_digest, &content_for(NEW_BYTES))
            .unwrap()
            .outcome,
        DispatchOutcome::Admissible,
        "resuming did not restore the previous answer"
    );
}

#[test]
fn dispatch_refuses_under_observe() {
    // BLUEPRINT:405-406 -- Observe permits no external mutation. The Charter is revised BEFORE the
    // decision, because revising it afterwards moves a revision the Ruling copied and the drift
    // gate would refuse first: this test would then pass without the autonomy gate existing at all.
    let fixture = admitted_mission("observe");
    ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .revise_charter(
            fixture.project_id,
            1,
            ProjectCharterDraft {
                purpose: "Prove the local agent office, observing only.".to_owned(),
                desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Observe,
            },
        )
        .unwrap();

    // A Policy revision belongs to a Charter revision, so the new Charter needs its own.
    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .publish(fixture.project_id, 2, Some(1), policy_draft(1))
        .unwrap();

    let repository = CopyRepository::create(&fixture.copies_root, "observe");
    let (proposal_digest, _candidate) = fixture.candidate_at_charter(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
        2,
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(
        receipt.autonomy_profile,
        Some(AutonomyProfile::Observe),
        "the profile is recorded because it is a gate"
    );
    assert_eq!(receipt.outcome, DispatchOutcome::AutonomyProfileRefuses);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn a_repository_outside_beacons_own_copies_is_refused() {
    // v1 reaches only repositories Beacon made. The blast radius is bounded by construction, not by
    // intent: a Candidate that authorizes an effect is a Material Artifact and v1 has no non-author
    // Evaluation of one (R8).
    let fixture = admitted_mission("provenance");
    let elsewhere = std::env::temp_dir().join(format!("beacon-not-a-copy-{}", Uuid::new_v4()));
    fs::create_dir_all(&elsewhere).unwrap();
    let repository = CopyRepository::create(&elsewhere, "outside");
    let (proposal_digest, _candidate) = fixture.candidate_for(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::SourceNotBeaconMade);
    assert_eq!(repository.snapshot(), before);
    fs::remove_dir_all(&elsewhere).unwrap();
}

#[test]
fn content_that_does_not_hash_to_the_candidate_is_refused_before_anything_is_read() {
    let fixture = admitted_mission("content");
    let (repository, ruling_digest) = fixture.approved("content");
    let before = repository.snapshot();

    // One byte different from what the Candidate declares.
    let mut wrong = NEW_BYTES.to_vec();
    wrong[0] = b'F';
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(&wrong))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::ContentDigestMismatch);

    // A second file nobody declared.
    let mut extra = content_for(NEW_BYTES);
    extra.push(SuppliedContent {
        path: "src/other.rs".to_owned(),
        bytes: Some(b"fn other() {}\n".to_vec()),
    });
    let receipt = fixture.dispatches.dispatch(&ruling_digest, &extra).unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::ContentDigestMismatch);

    // Nothing supplied at all.
    let receipt = fixture.dispatches.dispatch(&ruling_digest, &[]).unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::ContentDigestMismatch);

    assert_eq!(repository.snapshot(), before);
}

#[test]
fn a_dirty_or_detached_source_is_refused() {
    let fixture = admitted_mission("dirty");
    let (repository, ruling_digest) = fixture.approved("dirty");

    // An untracked file is enough: a dispatch would publish a commit whose parent nobody can
    // reproduce from what is on disk.
    fs::write(repository.path.join("scratch.txt"), "notes\n").unwrap();
    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::SourceDirty);
    assert_eq!(repository.snapshot(), before);
    fs::remove_file(repository.path.join("scratch.txt")).unwrap();

    // A detached head has no ref to move.
    CopyRepository::git(
        &repository.path,
        &["checkout", "--quiet", "--detach", &repository.revision],
    );
    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::SourceDetached);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn a_gate_read_does_not_write_to_the_repository_it_measures() {
    // gpt1's probe, as a committed control: `git status --porcelain` refreshes and rewrites
    // `.git/index` unless GIT_OPTIONAL_LOCKS=0 is set. Touching a tracked file changes its stat
    // information, which is exactly what makes Git want to rewrite the index -- so this is the
    // arrangement where a gate that writes would be caught.
    let fixture = admitted_mission("read-only");
    let (repository, ruling_digest) = fixture.approved("read-only");

    // The arrangement matters: Git rewrites the index when a tracked file's stat information no
    // longer matches what the index recorded. Writing the same bytes back is not enough -- the
    // timestamp can land in the same tick -- so the modification time is moved an hour into the
    // past, which is exactly the "changed mtime, identical content" case gpt1 probed. The snapshot
    // hashes contents and modes and not times, so this arrangement changes nothing it measures.
    let tracked = repository.path.join("src/lib.rs");
    let bytes = fs::read(&tracked).unwrap();
    fs::write(&tracked, &bytes).unwrap();
    let file = fs::File::options().write(true).open(&tracked).unwrap();
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    file.set_times(
        fs::FileTimes::new()
            .set_accessed(earlier)
            .set_modified(earlier),
    )
    .unwrap();
    drop(file);

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Admissible);
    assert_eq!(
        repository.snapshot(),
        before,
        "a gate read wrote to the repository it was measuring"
    );
}
