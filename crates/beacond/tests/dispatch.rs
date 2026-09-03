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
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use beacond::{
    AccessOrigin, AutonomyProfile, BeaconCapability, CandidateImages, CandidateService,
    DeclaredCheck, DispatchFailpoints, DispatchOutcome, DispatchService,
    LocalModelActivationService, LocalModelEndpoint, MissionDraft, MissionMemberDraft,
    MissionService, MutationEffect, OperatorAccountService, OperatorSessionPhase, PauseService,
    ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService,
    ProjectRepositoryService, ProjectService, ProposalService, ProviderFeature, RepositoryDraft,
    RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService, RulingDecision,
    RulingService, RulingSupersedes, SampleProjectService, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    SupervisorError, SuppliedContent, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension};
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
    /// The path the declared readable context names, which is the path the fixtures modify.
    context_path: String,
    /// The Proposal's own declared bounds, which G12 applies to supplied content.
    file_size_bytes: u64,
    output_bytes: u64,
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
        push_string(&mut entries, &self.context_path);
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
        for bound in [
            300_000_u64,
            240,
            2 << 30,
            self.file_size_bytes,
            self.output_bytes,
            0,
        ] {
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
        dispatches: DispatchService::new(Arc::clone(&accounts), workspace_id),
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
            context_path: "src/lib.rs".to_owned(),
            file_size_bytes: 256 << 20,
            output_bytes: 1 << 20,
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

    /// Run one batch with the Run, Receipt and preparation immutability triggers dropped, then put
    /// them back -- because startup verifies the schema signature before any state, and a store
    /// left short a trigger refuses startup for the wrong reason.
    fn without_run_triggers(&self, batch: &str) {
        let connection = self.store.connect();
        let mut statement = connection
            .prepare(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger'
                   AND tbl_name IN ('receipts', 'runs', 'dispatch_preparations')
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
                "DROP TRIGGER receipts_no_update;
                 DROP TRIGGER receipts_no_delete;
                 DROP TRIGGER runs_no_delete;
                 DROP TRIGGER runs_single_terminal_transition;
                 DROP TRIGGER dispatch_preparations_no_update;
                 DROP TRIGGER dispatch_preparations_no_delete;",
            )
            .unwrap();
        connection.execute_batch(batch).unwrap();
        for trigger in triggers {
            connection.execute_batch(&trigger).unwrap();
        }
    }

    /// Run one dispatch in a CHILD PROCESS that dies at `boundary`, without unwinding.
    ///
    /// The first version of this caught a panic in-process and called that a crash. gpt1 was right
    /// to reject it: unwinding runs destructors -- the scratch index is removed, the SQLite
    /// transaction is dropped in an orderly way -- and the same process then opened the store it
    /// had just "crashed" with. `abort()` in a child runs no destructor, releases the database
    /// lock the way a kill does, and leaves the parent to recover a store it did not have open.
    fn dispatch_until_death(&self, ruling_digest: &str, boundary: &str) {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "dispatch_child_that_dies_at_its_boundary",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_BOUNDARY, boundary)
            .env(CHILD_STORE, &self.store.path)
            .env(CHILD_WORKSPACE, self.workspace_id.to_string())
            .env(CHILD_RULING, ruling_digest)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("the test binary can run itself");
        // SIGABRT, not an exit code: a process that ran its cleanup would exit, and this must not.
        assert_eq!(
            status.signal(),
            Some(6),
            "the child did not die at {boundary} (status {status:?})"
        );
    }

    fn preparation(&self) -> Option<(String, String)> {
        self.store
            .connect()
            .query_row(
                "SELECT prepared_commit, branch FROM dispatch_preparations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .unwrap()
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
    ///
    /// Provenance is NOT recorded here: `Fixture::beacon_made` is what records it, so a test that
    /// forgets is a test whose dispatch refuses -- and one test forgets on purpose.
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
        self.beacon_made(repository);
        self.candidate_for_unrecorded(repository, summary, base_bytes)
    }

    /// [`Fixture::candidate_for`] without recording provenance, for the test that proves a
    /// repository Beacon did not make is refused.
    fn candidate_for_unrecorded(
        &self,
        repository: &CopyRepository,
        summary: &str,
        base_bytes: &[u8],
    ) -> (String, String) {
        let revision = self.rebind_repository_at(repository, 1);
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

    /// [`Fixture::candidate_for`] whose Candidate declares a pre-image the base does not hold.
    fn candidate_for_with_pre_image(
        &self,
        repository: &CopyRepository,
        summary: &str,
        declared_pre_image: &[u8],
    ) -> (String, String) {
        self.beacon_made(repository);
        let revision = self.rebind_repository_at(repository, 1);
        let mut record = self.proposal(summary);
        record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
        record.repository_revision = revision;
        record.base_revision = repository.revision.clone();
        record.context_digest = sha256(BASE_LIB_BYTES);
        let proposal = self.proposals.submit(&record.bytes()).unwrap();
        let candidate = self
            .candidates
            .record(
                &proposal.digest().value,
                &images_for(declared_pre_image, NEW_BYTES),
            )
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
        self.beacon_made(repository);
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

    /// Record that Beacon made this repository, the way `copy_for_dispatch` does.
    ///
    /// Written through the store directly because the only production act that creates a copy is
    /// the Sample Project's, and these fixtures need arbitrary repositories -- but the gate they
    /// pass is the real one, reading the real record by device and inode.
    fn beacon_made(&self, repository: &CopyRepository) {
        // What `copy_for_dispatch` does: a token generated by Beacon, written into the copy's own
        // configuration, and recorded against the identity of the directory that holds it. A
        // repository Beacon really made already carries one, and overwriting it here would leave
        // the row and the repository disagreeing -- which is exactly the state the gate refuses.
        let existing = Command::new("git")
            .args(["config", "--get", "beacon.provenanceToken"])
            .current_dir(&repository.path)
            .output()
            .unwrap();
        let token = if existing.status.success() {
            String::from_utf8(existing.stdout)
                .unwrap()
                .trim()
                .to_owned()
        } else {
            let token = Uuid::new_v4().to_string();
            CopyRepository::git(
                &repository.path,
                &["config", "beacon.provenanceToken", &token],
            );
            token
        };
        let metadata = fs::metadata(&repository.path).unwrap();
        self.store
            .connect()
            .execute(
                "INSERT OR IGNORE INTO beacon_repositories (
                     workspace_id, path, device, inode, origin, provenance_token, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, 'sample_project_copy', ?5, 1)",
                rusqlite::params![
                    self.workspace_id.to_string(),
                    fs::canonicalize(&repository.path)
                        .unwrap()
                        .to_string_lossy(),
                    metadata.dev() as i64,
                    metadata.ino() as i64,
                    token,
                ],
            )
            .unwrap();
    }

    /// Bind the Project to this repository at its current head, returning the new revision.
    fn rebind_repository(&self, repository: &CopyRepository) -> u32 {
        self.beacon_made(repository);
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
        let (repository, _proposal, ruling) = self.approved_proposal(label);
        (repository, ruling)
    }

    /// [`Fixture::approved`], with the Proposal digest a second writer needs to rule again.
    fn approved_proposal(&self, label: &str) -> (CopyRepository, String, String) {
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
        (repository, proposal_digest, ruling.digest().value.clone())
    }

    /// A second session on its own connection, the way another window or process would arrive.
    fn other_session(path: &Path) -> Arc<OperatorAccountService> {
        let accounts = Arc::new(OperatorAccountService::open(path).unwrap());
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .unwrap();
        accounts
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
fn every_gate_holding_lands_the_declared_change_and_nothing_else() {
    // The positive control without which every refusal is satisfied by a function that always
    // refuses -- and the first end-to-end proof that a Ruling can move a branch.
    let fixture = admitted_mission("applied");
    let (repository, ruling_digest) = fixture.approved("applied");
    let base = repository.revision.clone();

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let landed = receipt
        .resulting_revision
        .clone()
        .expect("an applied dispatch names the revision it produced");
    assert_eq!(receipt.prepared_commit.as_deref(), Some(landed.as_str()));
    assert_ne!(landed, base);

    // The branch names the prepared commit, and its parent is the base the Ruling pinned.
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        landed
    );
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", &format!("{landed}^")]),
        base
    );

    // The change is in the OBJECTS: the blob the commit names holds exactly the declared bytes.
    let blob = CopyRepository::git(
        &repository.path,
        &["cat-file", "blob", &format!("{landed}:{TARGET}")],
    );
    assert_eq!(blob.as_bytes(), NEW_BYTES.trim_ascii_end());
    // ...and the commit touches exactly the declared set.
    assert_eq!(
        CopyRepository::git(&repository.path, &["diff", "--name-status", &base, &landed]),
        format!("M\t{TARGET}")
    );

    // Beacon owns this checkout, so it is not left stale.
    assert!(receipt.checkout_updated);
    assert_eq!(fs::read(repository.path.join(TARGET)).unwrap(), NEW_BYTES);
    assert!(
        CopyRepository::git(&repository.path, &["status", "--porcelain"]).is_empty(),
        "the copy's working tree is not clean after a landing"
    );

    // The Receipt is the record: actor, both identities, the mutation's effect, and what was not
    // established -- including the seven families ADR 0034 leaves undeclarable.
    assert_eq!(receipt.version, 3);
    assert_eq!(receipt.per_mutation[0].effect, MutationEffect::Written);
    assert_eq!(receipt.autonomy_profile, Some(AutonomyProfile::Supervised));
    // The seven undeclared constraint families are named on every Receipt, in the closed
    // vocabulary, so no record of a dispatch can be read as having checked them.
    for family in [
        beacond::NotVerified::ExecutionPolicyConstraint,
        beacond::NotVerified::SandboxConstraint,
        beacond::NotVerified::NetworkConstraint,
        beacond::NotVerified::ModelProviderConstraint,
        beacond::NotVerified::CommandAllowlistConstraint,
        beacond::NotVerified::ToolAllowlistConstraint,
        beacond::NotVerified::EvaluationsConstraint,
    ] {
        assert!(receipt.not_verified.contains(&family), "{family:?}");
    }
    assert!(
        receipt
            .not_verified
            .contains(&beacond::NotVerified::NonAuthorEvaluationOfCandidate)
    );

    // The commit message carries the authority chain into Git itself, and carries nothing else.
    let message = CopyRepository::git(&repository.path, &["log", "-1", "--format=%B", &landed]);
    assert!(
        message.contains(&format!("Beacon-Ruling: {ruling_digest}")),
        "{message}"
    );
    assert!(message.contains("Beacon-Proposal: "), "{message}");
    assert!(message.contains("Beacon-Candidate: "), "{message}");
    assert_eq!(
        CopyRepository::git(
            &repository.path,
            &["log", "-1", "--format=%an <%ae>", &landed]
        ),
        "Beacon <beacon@localhost>"
    );

    fixture
        .restart()
        .expect("an applied dispatch survives a restart");
}

#[test]
fn one_ruling_lands_once_and_a_second_attempt_is_spent() {
    // gpt1's probe: two dispatchers that both observed the base both succeed under a fast-forward
    // merge, the second reporting the branch already current. The compare-and-swap is what makes
    // application exactly once, and the preparation record is what makes a repeat attempt refuse
    // rather than race.
    let fixture = admitted_mission("once");
    let (repository, ruling_digest) = fixture.approved("once");

    let first = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(first.outcome, DispatchOutcome::Applied);
    let after_first = repository.snapshot();

    // The head has moved, so the second attempt refuses at the live gate before anything else.
    let second = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(second.outcome, DispatchOutcome::RepositoryHeadMoved);
    assert_eq!(
        repository.snapshot(),
        after_first,
        "the second attempt changed the repository"
    );

    // And with the branch put back by hand -- so every live gate passes again -- the authority is
    // still spent. Without this the head gate would be doing the work and "one Ruling, one
    // application" would be an accident of arithmetic.
    CopyRepository::git(
        &repository.path,
        &["reset", "--hard", "--quiet", &repository.revision],
    );
    let restored = repository.snapshot();
    let third = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(third.outcome, DispatchOutcome::AlreadyDispatched);
    assert!(third.left_repository_byte_identical());
    assert_eq!(
        repository.snapshot(),
        restored,
        "a spent authority still wrote to the repository"
    );
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

    fixture.pause.pause().unwrap();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::InstallationPaused);
    assert_eq!(repository.snapshot(), before);

    fixture.pause.resume().unwrap();
    assert_eq!(
        fixture
            .dispatches
            .dispatch(&ruling_digest, &content_for(NEW_BYTES))
            .unwrap()
            .outcome,
        DispatchOutcome::Applied,
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
fn a_repository_beacon_did_not_make_is_refused() {
    // v1 reaches only repositories Beacon made. The blast radius is bounded by construction, not by
    // intent: a Candidate that authorizes an effect is a Material Artifact and v1 has no non-author
    // Evaluation of one (R8).
    //
    // The gate used to ask whether a path sat under a directory the CALLER named, so any repository
    // placed there qualified (gpt1). It asks the store now, by device and inode, and only the act
    // that creates a copy writes that record.
    let fixture = admitted_mission("provenance");
    let elsewhere = std::env::temp_dir().join(format!("beacon-not-a-copy-{}", Uuid::new_v4()));
    fs::create_dir_all(&elsewhere).unwrap();
    let repository = CopyRepository::create(&elsewhere, "outside");
    // Deliberately NOT recorded as Beacon-made. Where the repository sits is irrelevant now: what
    // the gate asks is whether Beacon created it.
    let (proposal_digest, _candidate) = fixture.candidate_for_unrecorded(
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

    // The dispatch must REFUSE, or the landing's own writes would drown the one byte this test is
    // about. An untracked file makes the cleanliness gate refuse -- after the status read that
    // would rewrite the index.
    fs::write(repository.path.join("scratch.txt"), "notes\n").unwrap();

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

    assert_eq!(receipt.outcome, DispatchOutcome::SourceDirty);
    assert_eq!(
        repository.snapshot(),
        before,
        "a gate read wrote to the repository it was measuring"
    );
}

// ---------------------------------------------------------------------------------------------
// The rows gpt1's kernel table marked FAIL
// ---------------------------------------------------------------------------------------------

#[test]
fn a_repository_swapped_for_a_decoy_at_the_same_path_receives_nothing() {
    // gpt1's probe: `git -C <path>` re-resolves the name every time, so renaming the measured
    // directory and installing an equivalent decoy at the same pathname redirected the mutation --
    // the decoy advanced and the real repository stayed at base. The repository is pinned by device
    // and inode now, and the pin is re-checked after every invocation.
    let fixture = admitted_mission("path-swap");
    let (repository, ruling_digest) = fixture.approved("path-swap");
    let original = repository.path.clone();
    let moved = fixture.copies_root.join("copy-path-swap-moved");

    // A decoy that is a real repository of its own. Its head is its own commit, so the head gate
    // judges the directory that is actually there rather than the one that was measured -- and
    // `PinnedRepository::still_pinned` is what makes a swap DURING an attempt void, proved
    // deterministically in `worktree`'s own tests rather than by racing this one.
    fs::rename(&original, &moved).unwrap();
    let decoy = CopyRepository::create(&fixture.copies_root, "path-swap-decoy");
    fs::write(decoy.path.join("decoy.txt"), "decoy\n").unwrap();
    CopyRepository::git(&decoy.path, &["add", "-A"]);
    CopyRepository::git(&decoy.path, &["commit", "--quiet", "-m", "decoy"]);
    fs::rename(&decoy.path, &original).unwrap();
    let decoy_before = raw_repository_snapshot(&original);
    let real_before = raw_repository_snapshot(&moved);

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    // The decoy's head is a different commit, so the head gate refuses it -- and whatever the
    // refusal, neither repository may move.
    assert_ne!(receipt.outcome, DispatchOutcome::Applied);
    assert_eq!(
        raw_repository_snapshot(&original),
        decoy_before,
        "the decoy at the pinned pathname was written to"
    );
    assert_eq!(
        raw_repository_snapshot(&moved),
        real_before,
        "the repository that was measured was written to"
    );
}

#[test]
fn candidate_bytes_enter_the_objects_unchanged_by_attribute_conversion() {
    // gpt1's probe: with `.gitattributes` saying `*.txt text eol=lf`, Git's BUILT-IN conversion --
    // no external filter, so the filter-driver hardening never sees it -- turned CRLF Candidate
    // bytes into an LF blob with a different SHA-256, while `diff --name-status` still reported the
    // declared modification. Objects are written with `hash-object --no-filters` now, so the
    // declared bytes ARE the object bytes.
    let fixture = admitted_mission("normalization");
    let repository = CopyRepository::create(&fixture.copies_root, "normalization");
    fs::write(
        repository.path.join(".gitattributes"),
        "*.txt text eol=lf\n",
    )
    .unwrap();
    fs::write(repository.path.join("notes.txt"), "one\n").unwrap();
    CopyRepository::git(&repository.path, &["add", "-A"]);
    CopyRepository::git(&repository.path, &["commit", "--quiet", "-m", "attributes"]);
    let repository = CopyRepository {
        revision: CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        path: repository.path,
    };

    // A Candidate whose after-bytes carry CRLF, which the attribute would normalize away.
    let crlf: &[u8] = b"one\r\ntwo\r\n";
    let revision = fixture.rebind_repository(&repository);
    let mut record = fixture.proposal("Rewrite the notes with Windows line endings.");
    record.mutations = vec![(MUTATION_MODIFY, "notes.txt".to_owned())];
    record.repository_revision = revision;
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(b"one\n");
    record.context_path = "notes.txt".to_owned();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: Some(sha256(b"one\n")),
                post_image_sha256: Some(sha256(crlf)),
            }],
        )
        .unwrap();
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture
                .candidates
                .present(&proposal.digest().value)
                .unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: "notes.txt".to_owned(),
                bytes: Some(crlf.to_vec()),
            }],
        )
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let landed = receipt.resulting_revision.clone().unwrap();
    // The blob in the published commit is the declared bytes, carriage returns and all.
    let blob = Command::new("git")
        .args(["cat-file", "blob", &format!("{landed}:notes.txt")])
        .current_dir(&repository.path)
        .output()
        .unwrap();
    assert_eq!(
        blob.stdout, crlf,
        "attribute conversion reached the objects"
    );
}

#[test]
fn a_declared_path_carrying_pathspec_magic_touches_only_itself() {
    // gpt1's probe: `git add -- ':(glob)*.txt'` staged a second, undeclared file, because `--` ends
    // option parsing and does not disable pathspec magic. No pathspec is passed at all now -- the
    // tree is built from the base plus one `update-index --cacheinfo` per declared mutation -- and
    // every invocation carries `--literal-pathspecs`.
    let fixture = admitted_mission("pathspec");
    let repository = CopyRepository::create(&fixture.copies_root, "pathspec");
    let magic = ":(glob)*.txt";
    fs::write(repository.path.join(magic), "magic\n").unwrap();
    fs::write(repository.path.join("victim.txt"), "victim\n").unwrap();
    CopyRepository::git(&repository.path, &["add", "-A"]);
    CopyRepository::git(&repository.path, &["commit", "--quiet", "-m", "magic"]);
    let repository = CopyRepository {
        revision: CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        path: repository.path,
    };

    let revision = fixture.rebind_repository(&repository);
    let mut record = fixture.proposal("Rewrite the file whose name is pathspec magic.");
    record.mutations = vec![(MUTATION_MODIFY, magic.to_owned())];
    record.repository_revision = revision;
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(b"magic\n");
    record.context_path = magic.to_owned();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: Some(sha256(b"magic\n")),
                post_image_sha256: Some(sha256(b"rewritten\n")),
            }],
        )
        .unwrap();
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture
                .candidates
                .present(&proposal.digest().value)
                .unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: magic.to_owned(),
                bytes: Some(b"rewritten\n".to_vec()),
            }],
        )
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let landed = receipt.resulting_revision.clone().unwrap();
    // Exactly one path changed, and it is the declared one.
    let diff = Command::new("git")
        .args(["diff", "--name-only", "-z", &repository.revision, &landed])
        .current_dir(&repository.path)
        .output()
        .unwrap();
    let changed: Vec<&str> = std::str::from_utf8(&diff.stdout)
        .unwrap()
        .split('\0')
        .filter(|part| !part.is_empty())
        .collect();
    assert_eq!(
        changed,
        vec![magic],
        "a pathspec expanded past its declaration"
    );
}

// ---------------------------------------------------------------------------------------------
// The walking skeleton
// ---------------------------------------------------------------------------------------------

#[test]
fn the_first_end_to_end_loop_applies_one_canned_change_to_a_sample_project_copy() {
    // Proposal -> Candidate -> Ruling v2 -> dispatch -> Applied, with a canned diff, no Agent and
    // no model. This is BLUEPRINT's Alpha steps 9-12 for the first time: the Operator's decision
    // names exact bytes, and those exact bytes reach the Repository through one act that records
    // what it did.
    //
    // The bundled Sample Project is never the target: its own verifier requires `refs/heads/main`
    // to hold the frozen revision, so a dispatch would make its installation read as tampered.
    // Beacon copies it, and the copy is what v1 dispatch is allowed to reach.
    let fixture = admitted_mission("skeleton");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();
    let copy = samples
        .copy_for_dispatch(&fixture.copies_root, "skeleton")
        .unwrap();

    // The canned change: one file in the sample tracker, rewritten.
    let target = "README.md";
    let before = fs::read(copy.join(target)).expect("the sample project has a README");
    let after = b"# Task Tracker\n\nDispatched by Beacon.\n".to_vec();
    assert_ne!(before, after);

    let repository = CopyRepository {
        revision: CopyRepository::git(&copy, &["rev-parse", "HEAD"]),
        path: copy.clone(),
    };
    let revision = fixture.rebind_repository(&repository);
    let mut record = fixture.proposal("Rewrite the task tracker's README.");
    record.mutations = vec![(MUTATION_MODIFY, target.to_owned())];
    record.repository_revision = revision;
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(&before);
    record.context_path = target.to_owned();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();

    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: Some(sha256(&before)),
                post_image_sha256: Some(sha256(&after)),
            }],
        )
        .unwrap();

    // The Operator is shown the exact change, and rules on what they were shown.
    let presentation = fixture
        .candidates
        .present(&proposal.digest().value)
        .unwrap();
    assert_eq!(presentation.lines().len(), 1);
    assert_eq!(presentation.lines()[0].path, target);
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &presentation,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: target.to_owned(),
                bytes: Some(after.clone()),
            }],
        )
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let landed = receipt.resulting_revision.clone().unwrap();
    assert_eq!(receipt.ruling_digest.value, ruling.digest().value);
    assert_eq!(
        receipt.proposal_digest.as_ref().unwrap().value,
        proposal.digest().value
    );
    assert_eq!(receipt.per_mutation[0].effect, MutationEffect::Written);

    // The Repository holds the change, in its objects and in Beacon's own checkout.
    assert_eq!(CopyRepository::git(&copy, &["rev-parse", "HEAD"]), landed);
    assert_eq!(fs::read(copy.join(target)).unwrap(), after);
    assert_eq!(
        CopyRepository::git(
            &copy,
            &["diff", "--name-status", &repository.revision, &landed]
        ),
        format!("M\t{target}")
    );

    // And the bundled Sample Project is untouched: still frozen, still verifying.
    assert!(
        matches!(
            samples.status().unwrap(),
            beacond::SampleProjectStatus::Installed { .. }
        ),
        "dispatching into the copy disturbed the bundled Sample Project"
    );

    fixture
        .restart()
        .expect("the whole loop survives a restart");
}

#[test]
fn a_swap_that_cannot_be_written_refuses_and_publishes_nothing() {
    // The compare-and-swap's own failure path, driven deterministically: the refs directory is made
    // unwritable, so `update-ref` cannot move the branch. The outcome must be a refusal that names
    // the branch as unmoved -- not `Applied`, and not a claim of an unobserved landing when the ref
    // can be read and plainly still holds the base.
    let fixture = admitted_mission("cas-refused");
    let (repository, ruling_digest) = fixture.approved("cas-refused");
    let base = repository.revision.clone();

    let heads = repository.path.join(".git/refs/heads");
    let original = fs::metadata(&heads).unwrap().permissions().mode();
    fs::set_permissions(&heads, fs::Permissions::from_mode(0o500)).unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    fs::set_permissions(&heads, fs::Permissions::from_mode(original)).unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::LandingRefused);
    assert_eq!(receipt.resulting_revision, None);
    // The commit was prepared, so the Receipt names it and does not claim byte identity.
    assert!(receipt.prepared_commit.is_some());
    assert!(!receipt.left_repository_byte_identical());
    // The branch did not move, and the working tree was not touched.
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base
    );
    assert_eq!(
        fs::read(repository.path.join(TARGET)).unwrap(),
        b"fn main() {}\n"
    );
}

#[test]
fn a_landed_dispatch_replays_from_its_own_receipt_after_a_restart() {
    // The Receipt, the Event and the Run's terminal state are one record checked three ways: the
    // Event payload and the stored Receipt must be byte-identical AND equal as values, and the Run
    // must be in the state the outcome maps to. Startup deliberately re-derives nothing about the
    // repository -- a Receipt records what was true then, and later history is not corruption.
    let fixture = admitted_mission("replay");
    let (repository, ruling_digest) = fixture.approved("replay");
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);

    fixture
        .restart()
        .expect("an applied dispatch survives a restart");

    // The repository moving on afterwards is ordinary history, not a broken store.
    CopyRepository::git(
        &repository.path,
        &["commit", "--quiet", "--allow-empty", "-m", "later"],
    );
    fixture
        .restart()
        .expect("later history in the Repository is not store corruption");

    // But the Receipt and its Event must still agree with each other.
    fixture.tamper(
        "UPDATE events SET payload_json = json_set(payload_json, '$.outcome', 'admissible')
         WHERE kind = 'proposal.dispatched'",
    );
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::DispatchEventMismatch(_))
        ),
        "an Event that disagrees with its Receipt started"
    );
}

#[test]
fn a_dispatch_run_state_matches_its_receipt_outcome() {
    // `check_preflight::verify_run_state` owns cardinality for every Run kind, so a dispatch Run
    // whose state does not match its Receipt is caught there rather than by a second walk.
    let fixture = admitted_mission("run-state");
    let (_repository, ruling_digest) = fixture.approved("run-state");
    fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    let state: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT state FROM runs WHERE kind = 'proposal_dispatch'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "applied");
}

#[test]
fn content_beyond_the_proposals_own_bounds_is_refused_before_it_is_read() {
    // gpt1's P1: deleting the file-size bound left the whole target green, because no test reached
    // `ContentOverBounds`. Three bounds now, each with its own arrangement, so a deletion of any
    // one of them is red on its own rather than covered by the others.
    let fixture = admitted_mission("bounds");
    let repository = CopyRepository::create(&fixture.copies_root, "bounds");

    // A Proposal whose declared file-size bound is small, and content one byte over it.
    let big = vec![b'x'; 4_096];
    let mut record = fixture.proposal("Rewrite the library root with a large file.");
    record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
    record.repository_revision = fixture.rebind_repository(&repository);
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    record.file_size_bytes = 4_095;
    record.output_bytes = 1 << 20;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(&proposal.digest().value, &images_for(BASE_LIB_BYTES, &big))
        .unwrap();
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture
                .candidates
                .present(&proposal.digest().value)
                .unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: TARGET.to_owned(),
                bytes: Some(big.clone()),
            }],
        )
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::ContentOverBounds);
    assert!(receipt.left_repository_byte_identical());
    assert_eq!(repository.snapshot(), before);

    // The aggregate ceiling, with a per-file bound that permits the same content: the two limits
    // are different facts and a refusal must not depend on which one the fixture happened to cross.
    let fixture = admitted_mission("bounds-total");
    let repository = CopyRepository::create(&fixture.copies_root, "bounds-total");
    let mut record = fixture.proposal("Rewrite the library root within the file bound.");
    record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
    record.repository_revision = fixture.rebind_repository(&repository);
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    record.file_size_bytes = 1 << 20;
    record.output_bytes = 4_095;
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(&proposal.digest().value, &images_for(BASE_LIB_BYTES, &big))
        .unwrap();
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture
                .candidates
                .present(&proposal.digest().value)
                .unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();
    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: TARGET.to_owned(),
                bytes: Some(big),
            }],
        )
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::ContentOverBounds);
}

#[test]
fn a_candidate_describing_a_tree_that_is_not_there_is_refused_before_any_object_is_written() {
    // A Candidate may declare a deletion of a path the base revision does not hold. This used to
    // reach the object writes and be caught by the effect post-condition -- after residue existed --
    // because nothing before that read the base tree at all (gpt1). The pre-image gate reads it
    // now, so the refusal costs the repository nothing.
    let fixture = admitted_mission("effect");
    let repository = CopyRepository::create(&fixture.copies_root, "effect");
    let absent = "src/never-there.rs";

    let mut record = fixture.proposal("Delete a file that is not in the tree.");
    record.mutations = vec![(MUTATION_DELETE, absent.to_owned())];
    record.repository_revision = fixture.rebind_repository(&repository);
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: Some(sha256(b"whatever it would have held\n")),
                post_image_sha256: None,
            }],
        )
        .unwrap();
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture
                .candidates
                .present(&proposal.digest().value)
                .unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let head_before = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    let receipt = fixture
        .dispatches
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: absent.to_owned(),
                bytes: None,
            }],
        )
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::PreImageMismatch);
    // Nothing was prepared, so the repository is byte-identical rather than carrying residue --
    // which is the improvement: the same defect used to be caught after objects existed.
    assert_eq!(receipt.prepared_commit, None);
    assert!(receipt.left_repository_byte_identical());
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(receipt.resulting_revision, None);
}

#[test]
fn a_corrupt_ruling_or_proposal_record_refuses_with_a_receipt_rather_than_an_error() {
    // gpt1's P0: a Run is opened before the gates run, so an owning-reader failure that escaped as
    // an error left the Run nonterminal with no Receipt -- and `RulingUnreadable` and
    // `ProposalDigestMismatch` covered only "absent", never the corrupt states they are named for.
    let fixture = admitted_mission("corrupt");
    let (repository, ruling_digest) = fixture.approved("corrupt");
    let before = repository.snapshot();

    // A Ruling whose audit Event no longer says what its bytes say.
    fixture.tamper(
        "UPDATE events SET payload_json = json_set(payload_json, '$.ruling.actor', 'resident')
         WHERE kind = 'ruling.recorded'",
    );
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::RulingUnreadable);
    assert!(receipt.left_repository_byte_identical());
    assert_eq!(repository.snapshot(), before);

    // The Run reached a terminal state with a Receipt, which is what makes the refusal evidence.
    let terminal: i64 = fixture
        .count("SELECT count(*) FROM runs WHERE kind = 'proposal_dispatch' AND state = 'refused'");
    assert_eq!(terminal, 1);
    assert_eq!(
        fixture.count("SELECT count(*) FROM receipts"),
        1,
        "a refusal left no Receipt"
    );
}

#[test]
fn a_proposal_whose_event_disagrees_with_its_row_is_refused() {
    let fixture = admitted_mission("corrupt-proposal");
    let (repository, ruling_digest) = fixture.approved("corrupt-proposal");
    let before = repository.snapshot();

    fixture.tamper(
        "UPDATE events SET payload_json = json_set(payload_json, '$.proposal.actor', 'resident')
         WHERE kind = 'proposal.submitted'",
    );
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::ProposalDigestMismatch);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn resume_refuses_while_a_run_is_unresolved_and_records_what_was_accepted() {
    // AGENTS.md:128-137 makes Resume conditional: every stopping or unconfirmed Run is reviewed and
    // either resolved or explicitly accepted. A single `set_paused(false)` made that transition
    // representable in one call with no Event (gpt1's P0).
    let fixture = admitted_mission("resume");
    fixture.pause.pause().unwrap();
    assert!(fixture.pause.is_paused().unwrap());

    // An open Run, planted the way a crash would leave one.
    fixture.tamper(&format!(
        "INSERT INTO runs (
             workspace_id, id, project_id, kind, repository_revision, charter_revision,
             state, lease_expires_at_ms, started_at_ms, ended_at_ms
         ) VALUES ('{}', '{}', '{}', 'proposal_dispatch', 1, 1, 'unconfirmed', 1, 1, 2)",
        fixture.workspace_id,
        Uuid::new_v4(),
        fixture.project_id,
    ));

    let refusal = fixture.pause.resume().unwrap_err();
    assert!(
        matches!(refusal, beacond::DispatchError::UnresolvedRuns(ref unresolved)
            if unresolved.run_ids.len() == 1),
        "{refusal:?}"
    );
    assert!(
        fixture.pause.is_paused().unwrap(),
        "a refused Resume changed the state"
    );

    // Accepting the uncertainty is permitted, and is recorded with the Runs it covered.
    fixture
        .pause
        .resume_accepting_unresolved("The Run predates this machine; its effects were inspected.")
        .unwrap();
    assert!(!fixture.pause.is_paused().unwrap());

    let payload: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'installation.resumed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["paused"], false);
    assert_eq!(
        value["acceptedUnresolved"]["runIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        value["acceptedUnresolved"]["reason"]
            .as_str()
            .unwrap()
            .contains("inspected")
    );
    fixture
        .restart()
        .expect("a paused-and-resumed store starts");
}

#[test]
fn a_request_with_several_faults_is_refused_by_the_earliest_gate() {
    // The published order is part of the decision, so which refusal a multi-fault request gets is
    // a fact to pin rather than an artifact of reading order (gpt1). Here the Ruling is refused AND
    // the Installation is paused AND the content is wrong: the earliest gate wins.
    let fixture = admitted_mission("precedence");
    let repository = CopyRepository::create(&fixture.copies_root, "precedence");
    let (proposal_digest, _candidate) = fixture.candidate_for(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
    );
    let refused = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Refuse,
            "Not this change.",
            RulingSupersedes::None,
        )
        .unwrap();
    fixture.pause.pause().unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(&refused.digest().value, &content_for(b"wrong bytes\n"))
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::RulingRefused,
        "a later gate reported the refusal for a request that fails an earlier one"
    );
}

#[test]
fn an_interruption_after_the_swap_is_finished_as_applied_at_startup() {
    // The L1/R1 boundary, driven rather than arranged: the process dies immediately after
    // `update-ref` and before anything about it is committed. The ref has moved; the landing
    // transaction is lost. What survives is the claim, which is committed BEFORE the swap for
    // exactly this reason -- and startup must turn it into the truthful terminal Receipt.
    let fixture = admitted_mission("killed-after-swap");
    let (repository, ruling_digest) = fixture.approved("killed-after-swap");

    fixture.dispatch_until_death(&ruling_digest, "after_swap");

    // What a killed process actually left: a moved ref, a committed claim, no Receipt.
    let (prepared, _branch) = fixture
        .preparation()
        .expect("the claim did not survive the crash");
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        prepared,
        "the swap did not happen, so this test proves nothing about recovery"
    );
    assert_eq!(fixture.count("SELECT count(*) FROM receipts"), 0);

    fixture
        .restart()
        .expect("startup finishes the interrupted attempt");

    let (outcome, payload): (String, String) = fixture
        .store
        .connect()
        .query_row("SELECT outcome, receipt_json FROM receipts", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(outcome, "applied");
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["resultingRevision"], prepared);
    assert_eq!(value["preparedCommit"], prepared);
    assert!(
        value["notVerified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "reconstructed_after_interruption"),
        "{payload}"
    );
    let state: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT state FROM runs WHERE kind = 'proposal_dispatch'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(state, "applied");
}

#[test]
fn an_interruption_between_the_claim_and_the_swap_is_finished_as_refused() {
    // The same boundary one step earlier: the claim is committed and the swap never ran. "Refused"
    // is read off the ref, not guessed -- and telling it from the case above is the whole reason
    // the claim persists the commit it was about to publish.
    let fixture = admitted_mission("killed-before-swap");
    let (repository, ruling_digest) = fixture.approved("killed-before-swap");
    let base = repository.revision.clone();

    fixture.dispatch_until_death(&ruling_digest, "before_landing");

    let (prepared, _branch) = fixture
        .preparation()
        .expect("the claim did not survive the crash");
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base,
        "the branch moved before the landing transaction"
    );
    assert_ne!(prepared, base);

    fixture
        .restart()
        .expect("startup finishes the interrupted attempt");

    let (outcome, payload): (String, String) = fixture
        .store
        .connect()
        .query_row("SELECT outcome, receipt_json FROM receipts", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(outcome, "landing_refused");
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    // It names the objects it left, because the claim persisted what they were.
    assert_eq!(value["preparedCommit"], prepared);
    assert!(
        value["notVerified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "unreferenced_object_residue"),
        "{payload}"
    );
}

#[test]
fn the_claim_is_committed_before_the_swap_and_a_second_connection_can_read_it() {
    // Why the claim is not inside the landing transaction. At this point the objects exist, the
    // claim is committed, and the ref has not moved -- so a reader that is not this process can
    // already name the commit an interruption would strand. Inside the landing transaction the row
    // would be invisible until after the swap, which is the window a crash falls into.
    let fixture = admitted_mission("durable-claim");
    let (repository, ruling_digest) = fixture.approved("durable-claim");
    let base = repository.revision.clone();
    let path = fixture.store.path.clone();
    let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&observed);
    let head = repository.path.clone();
    let at_base = Arc::new(Mutex::new(false));
    let still_at_base = Arc::clone(&at_base);

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                let connection = Connection::open(&path).unwrap();
                *seen.lock().unwrap() = connection
                    .query_row(
                        "SELECT prepared_commit FROM dispatch_preparations",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .unwrap();
                *still_at_base.lock().unwrap() =
                    CopyRepository::git(&head, &["rev-parse", "HEAD"]) == base;
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    assert_eq!(
        observed.lock().unwrap().as_deref(),
        receipt.prepared_commit.as_deref(),
        "the claim was not committed before the swap"
    );
    assert!(
        *at_base.lock().unwrap(),
        "the ref had already moved when the claim was read"
    );
}

#[test]
fn an_interruption_before_the_claim_is_terminal_and_names_its_unnameable_residue() {
    // The earliest boundary: objects exist and nothing has recorded what they are. The honest
    // terminal record is that the result is unobserved AND that the residue cannot be named. A
    // guess in either direction would be worse than the uncertainty.
    let fixture = admitted_mission("killed-before-claim");
    let (repository, ruling_digest) = fixture.approved("killed-before-claim");
    let base = repository.revision.clone();

    fixture.dispatch_until_death(&ruling_digest, "before_reserve");

    assert!(fixture.preparation().is_none());
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base
    );

    fixture
        .restart()
        .expect("startup finishes an attempt that never reached its claim");

    let (outcome, payload): (String, String) = fixture
        .store
        .connect()
        .query_row("SELECT outcome, receipt_json FROM receipts", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(outcome, "landing_unobserved");
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let not_verified = value["notVerified"].as_array().unwrap();
    assert!(
        not_verified
            .iter()
            .any(|entry| entry == "residue_cannot_be_named"),
        "{payload}"
    );
    assert!(value["preparedCommit"].is_null());
}

#[test]
fn the_landing_transaction_holds_the_write_lock_across_the_swap() {
    // gpt2's P0 on linearization: the authority that permits a landing must not be released before
    // the irreversible step. It is re-read inside the landing transaction, and that transaction
    // holds the store's write lock across the swap -- so a competing Ruling, Policy publication or
    // Pause cannot commit in the window. This proves the lock is what it claims: while an immediate
    // transaction is open on one connection, a second connection cannot write.
    let fixture = admitted_mission("linearization");
    let holder = fixture.store.connect();
    holder
        .busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();
    let other = fixture.store.connect();
    other
        .busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();

    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let blocked = other.execute_batch(
        "UPDATE installation_pause SET paused = 1, changed_at_ms = 1 WHERE singleton = 1",
    );
    assert!(
        blocked.is_err(),
        "a second writer committed while a landing transaction was open"
    );
    holder.execute_batch("ROLLBACK").unwrap();

    // The control: with no transaction open, the same write succeeds.
    other
        .execute_batch(
            "UPDATE installation_pause SET paused = 1, changed_at_ms = 1 WHERE singleton = 1",
        )
        .expect("the write is only blocked by the lock");
}

#[test]
fn two_concurrent_dispatchers_produce_one_application_and_one_refusal() {
    // gpt1's acceptance row: the committed test dispatched sequentially, so two dispatchers never
    // observed the same base. Both do here, released together by a barrier -- no sleeps, and the
    // outcome is deterministic in shape rather than in timing: the landing transaction holds the
    // store's write lock across the swap, so one attempt lands and the other finds the authority
    // spent when it reaches the same transaction.
    let fixture = admitted_mission("concurrent");
    let (repository, ruling_digest) = fixture.approved("concurrent");
    let base = repository.revision.clone();

    let barrier = Arc::new(std::sync::Barrier::new(2));
    let outcomes: Vec<DispatchOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let dispatches =
                    DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
                let ruling = ruling_digest.clone();
                scope.spawn(move || {
                    barrier.wait();
                    dispatches.dispatch(&ruling, &content_for(NEW_BYTES))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                // Every thread's result, including an error: dropping the ones that failed let a
                // mutant that made one dispatcher error out pass as "one Applied, all outcomes
                // expected" (gpt1). A dispatch that could not answer is a fact about this race.
                handle
                    .join()
                    .unwrap()
                    .map(|receipt| receipt.outcome)
                    .expect("a concurrent dispatcher failed instead of refusing")
            })
            .collect()
    });
    assert_eq!(outcomes.len(), 2, "a dispatcher produced no outcome at all");

    // Exactly one application. The other attempt is refused -- by the spend if it reached the
    // landing transaction, by the head gate if it re-read the repository after the first landed.
    let applied = outcomes
        .iter()
        .filter(|outcome| **outcome == DispatchOutcome::Applied)
        .count();
    assert_eq!(
        applied, 1,
        "two concurrent dispatchers produced {outcomes:?}"
    );
    assert!(
        outcomes.iter().all(|outcome| matches!(
            outcome,
            DispatchOutcome::Applied
                | DispatchOutcome::AlreadyDispatched
                | DispatchOutcome::RepositoryHeadMoved
        )),
        "an unexpected outcome from a concurrent attempt: {outcomes:?}"
    );

    // And the repository advanced exactly once.
    let head = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    assert_ne!(head, base);
    assert_eq!(
        CopyRepository::git(
            &repository.path,
            &["rev-list", "--count", &format!("{base}..{head}")]
        ),
        "1",
        "the branch advanced by more than one commit"
    );
}

// ---------------------------------------------------------------------------------------------
// Receipt v3's published shape
// ---------------------------------------------------------------------------------------------

/// Every member the contract names, in declaration order.
///
/// Pinned as a literal list rather than derived from the type, so adding, removing or reordering a
/// member is a decision someone makes here rather than a silent change to a record other
/// implementations and later readers depend on (gpt2, gpt1).
const RECEIPT_MEMBERS: &[&str] = &[
    "version",
    "runId",
    "workspaceId",
    "projectId",
    "actor",
    "rulingDigest",
    "proposalDigest",
    "candidateDigest",
    "outcome",
    "autonomyProfile",
    "charterRevision",
    "projectPolicyRevision",
    "repositoryRevision",
    "sourceKind",
    "source",
    "sourceIdentity",
    "branch",
    "pinnedBaseRevision",
    "observedHead",
    "preparedCommit",
    "resultingRevision",
    "diffDigest",
    "checkoutUpdated",
    "gitSteps",
    "modelInvocations",
    "costUsdMicros",
    "perMutation",
    "startedAtMs",
    "endedAtMs",
    "notVerified",
];

/// The top-level member names in the order the stored bytes carry them.
///
/// Read off the text rather than out of a parsed map: `serde_json`'s map is sorted, so parsing and
/// reading its keys would assert alphabetical order and pass no matter how the record is written.
/// How the parent tells the child which boundary to die at, and what to dispatch.
const CHILD_BOUNDARY: &str = "BEACON_DISPATCH_CRASH_AT";
const CHILD_STORE: &str = "BEACON_DISPATCH_CRASH_STORE";
const CHILD_WORKSPACE: &str = "BEACON_DISPATCH_CRASH_WORKSPACE";
const CHILD_RULING: &str = "BEACON_DISPATCH_CRASH_RULING";

/// The child half of every crash test: dispatch, and die where the parent said.
///
/// A no-op in an ordinary run -- the parent re-executes this binary with the variables set.
#[test]
fn dispatch_child_that_dies_at_its_boundary() {
    let Ok(boundary) = std::env::var(CHILD_BOUNDARY) else {
        return;
    };
    let store = PathBuf::from(std::env::var(CHILD_STORE).unwrap());
    let workspace_id = Uuid::parse_str(&std::env::var(CHILD_WORKSPACE).unwrap()).unwrap();
    let ruling_digest = std::env::var(CHILD_RULING).unwrap();

    // `abort()` raises SIGABRT: no unwinding, no destructors, no orderly transaction rollback.
    let die: Arc<dyn Fn() + Send + Sync> = Arc::new(|| std::process::abort());
    let failpoints = match boundary.as_str() {
        "before_reserve" => DispatchFailpoints {
            before_reserve: Some(die),
            ..DispatchFailpoints::default()
        },
        "before_landing" => DispatchFailpoints {
            before_landing: Some(die),
            ..DispatchFailpoints::default()
        },
        "after_swap" => DispatchFailpoints {
            after_swap: Some(die),
            ..DispatchFailpoints::default()
        },
        other => panic!("unknown crash boundary {other}"),
    };

    let accounts = Fixture::other_session(&store);
    let dispatches = DispatchService::new(accounts, workspace_id).with_failpoints(failpoints);
    let _ = dispatches.dispatch(&ruling_digest, &content_for(NEW_BYTES));
    // Reaching this line means the boundary was never hit, which the parent must not read as a
    // crash: exit rather than return, so the parent sees a code instead of a signal.
    std::process::exit(97);
}

/// The members a `RulingRefused` leaves absent, in sorted order.
///
/// The same list is asserted against the pinned specimen in `dispatch::tests`. Together they are
/// what ties a published vector to a record this dispatch path can actually produce: the specimen
/// is not a hand-built shape that happens to serialize, it is the shape a real refusal has (gpt2).
const REFUSED_ABSENT: &[&str] = &[
    "autonomyProfile",
    "branch",
    "candidateDigest",
    "diffDigest",
    "observedHead",
    "preparedCommit",
    "resultingRevision",
    "source",
    "sourceIdentity",
    "sourceKind",
];

/// The members of `payload` whose value is `null`, sorted.
fn absent_members(payload: &str) -> Vec<String> {
    let value: serde_json::Map<String, serde_json::Value> = serde_json::from_str(payload).unwrap();
    value
        .iter()
        .filter(|(_, value)| value.is_null())
        .map(|(name, _)| name.clone())
        .collect()
}

fn receipt_members(payload: &str) -> Vec<String> {
    let bytes = payload.as_bytes();
    let mut members = Vec::new();
    let mut depth = 0usize;
    let mut expecting_key = false;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => {
                depth += 1;
                expecting_key = depth == 1;
                index += 1;
            }
            b'}' | b']' => {
                depth -= 1;
                index += 1;
            }
            b'"' => {
                let start = index + 1;
                let mut end = start;
                while bytes[end] != b'"' {
                    if bytes[end] == b'\\' {
                        end += 1;
                    }
                    end += 1;
                }
                if depth == 1 && expecting_key {
                    members.push(payload[start..end].to_owned());
                    expecting_key = false;
                }
                index = end + 1;
            }
            b',' => {
                expecting_key = depth == 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    members
}

#[test]
fn an_applied_receipt_has_the_published_shape() {
    let fixture = admitted_mission("receipt-applied");
    let (_repository, ruling_digest) = fixture.approved("receipt-applied");
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);

    let stored: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(receipt_members(&stored), RECEIPT_MEMBERS);

    // The measurements an Applied Receipt must carry, each one a fact this dispatch established.
    let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(value["version"], 3);
    assert_eq!(value["actor"], "owner");
    assert_eq!(value["outcome"], "applied");
    assert_eq!(value["autonomyProfile"], "supervised");
    assert!(value["sourceIdentity"]["inode"].as_u64().unwrap() > 0);
    assert_eq!(value["branch"], "refs/heads/main");
    assert!(value["diffDigest"].is_string(), "no measured diff digest");
    assert_eq!(value["checkoutUpdated"], true);
    assert_eq!(value["modelInvocations"], 0);
    assert_eq!(value["costUsdMicros"], 0);
    assert_eq!(value["perMutation"][0]["effect"], "written");
    // Every Git step it attempted, in order, with a bounded result.
    let steps: Vec<&str> = value["gitSteps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step"].as_str().unwrap())
        .collect();
    assert_eq!(
        steps,
        vec![
            "read_tree",
            "hash_object",
            "update_index",
            "write_tree",
            "commit_tree",
            "diff_name_status",
            "update_ref",
            "read_tree_checkout",
        ]
    );
    assert!(
        value["gitSteps"]
            .as_array()
            .unwrap()
            .iter()
            .all(|step| step["result"] == "succeeded")
    );
}

#[test]
fn a_refused_receipt_has_the_same_shape_with_the_facts_it_does_not_have_absent() {
    // The same members, so a reader parses one record and not three. What a refusal did not
    // establish is `null`, which is a typed absence rather than a zero standing in for a fact.
    let fixture = admitted_mission("receipt-refused");
    let repository = CopyRepository::create(&fixture.copies_root, "receipt-refused");
    let (proposal_digest, candidate_digest) = fixture.candidate_for(
        &repository,
        "Apply the dispatched change to the library root.",
        BASE_LIB_BYTES,
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Refuse,
            "Not this change.",
            RulingSupersedes::None,
        )
        .unwrap();
    let _ = candidate_digest;

    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::RulingRefused);

    let stored: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(receipt_members(&stored), RECEIPT_MEMBERS);
    // The shape the pinned specimen claims to be: exactly these members absent, no more and no
    // fewer. A vector for a shape production cannot emit is a vector for nothing.
    assert_eq!(absent_members(&stored), REFUSED_ABSENT);
    let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(value["outcome"], "ruling_refused");
    assert!(value["preparedCommit"].is_null());
    assert!(value["resultingRevision"].is_null());
    assert!(value["diffDigest"].is_null());
    assert!(value["observedHead"].is_null());
    assert_eq!(value["checkoutUpdated"], false);
    assert_eq!(value["gitSteps"].as_array().unwrap().len(), 0);
    assert!(
        value["notVerified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "change_not_applied")
    );
}

#[test]
fn a_reconstructed_receipt_has_the_same_shape_and_says_what_it_is() {
    let fixture = admitted_mission("receipt-reconstructed");
    let (_repository, ruling_digest) = fixture.approved("receipt-reconstructed");
    fixture.dispatch_until_death(&ruling_digest, "after_swap");
    fixture.restart().unwrap();

    let stored: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(receipt_members(&stored), RECEIPT_MEMBERS);
    let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(value["outcome"], "applied");
    // It carries no Git steps and no diff digest, because it did not run them: it read the ref.
    assert_eq!(value["gitSteps"].as_array().unwrap().len(), 0);
    assert!(value["diffDigest"].is_null());
    assert!(
        value["notVerified"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry == "reconstructed_after_interruption")
    );
}

#[test]
fn a_modification_whose_declared_before_bytes_are_not_there_is_refused() {
    // The deletion arm above proves the gate reads the base tree. This is the modification arm:
    // the path is there, the bytes under it are not the ones the Candidate says it measured, so
    // the Ruling approved a change to a file the Operator was never shown.
    let fixture = admitted_mission("stale-pre-image");
    let repository = CopyRepository::create(&fixture.copies_root, "stale-pre-image");
    let (proposal_digest, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Modify the library root from bytes it no longer holds.",
        b"what the file held some time ago\n",
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

    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::PreImageMismatch);
    assert_eq!(receipt.prepared_commit, None);
    assert!(receipt.left_repository_byte_identical());
}

#[test]
fn an_applied_receipt_that_does_not_match_its_durable_preparation_is_refused_at_startup() {
    // Byte equality between a Receipt and its Event proves the two copies agree; it does not prove
    // either is true. The durable preparation is the independent record of what this dispatch was
    // about to publish, written inside the landing transaction before the swap, so a stored
    // `Applied` that names a different commit is a claim nothing in this build could have written.
    let fixture = admitted_mission("false-applied");
    let (_repository, ruling_digest) = fixture.approved("false-applied");
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let landed = receipt.resulting_revision.clone().unwrap();
    let forged = "0123456789abcdef0123456789abcdef01234567";

    // The repository is untouched: the lie is entirely in the record, and both copies of it are
    // rewritten together so byte equality still holds.
    fixture.without_run_triggers(&format!(
        "UPDATE receipts SET receipt_json = replace(receipt_json, '{landed}', '{forged}');
         UPDATE events SET payload_json = replace(payload_json, '{landed}', '{forged}')
         WHERE kind = 'proposal.dispatched';"
    ));

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::DispatchEventMismatch(_))
        ),
        "startup accepted an Applied Receipt its own preparation contradicts"
    );
}

#[test]
fn an_applied_receipt_that_names_no_landing_is_refused_at_startup() {
    // The other half: a record that contradicts itself rather than its preparation. Both copies are
    // rewritten the same way, so byte equality still holds and only the arm's own invariant is left
    // to catch it.
    let fixture = admitted_mission("incoherent-applied");
    let (_repository, ruling_digest) = fixture.approved("incoherent-applied");
    fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    fixture.without_run_triggers(
        "UPDATE receipts
            SET receipt_json = json_set(receipt_json, '$.resultingRevision', NULL);
         UPDATE events
            SET payload_json = json_set(payload_json, '$.resultingRevision', NULL)
          WHERE kind = 'proposal.dispatched';",
    );

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::DispatchEventMismatch(_))
        ),
        "startup accepted an Applied Receipt that names no landing"
    );
}

#[test]
fn a_supersession_committed_between_the_gates_and_the_swap_does_not_land() {
    // The race R8 rules on, driven rather than argued. A second session -- its own connection, its
    // own unlock, the real Command -- reverses the Ruling after every gate has passed it and before
    // the landing transaction opens. The authority re-read inside that transaction is the only
    // thing standing between the reversal and a published commit.
    let fixture = admitted_mission("race-supersede");
    let (repository, proposal_digest, ruling_digest) = fixture.approved_proposal("race-supersede");
    let base = repository.revision.clone();
    let path = fixture.store.path.clone();
    let workspace_id = fixture.workspace_id;
    let reversed = ruling_digest.clone();

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                let others = Fixture::other_session(&path);
                let candidates = CandidateService::new(Arc::clone(&others), workspace_id);
                let rulings = RulingService::new(Arc::clone(&others), workspace_id);
                rulings
                    .rule_on_candidate(
                        &candidates.present(&proposal_digest).unwrap(),
                        RulingDecision::Refuse,
                        "On reflection, not this change.",
                        RulingSupersedes::Ruling(reversed.clone()),
                    )
                    .unwrap();
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RulingSuperseded);
    assert_eq!(receipt.resulting_revision, None);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base,
        "a superseded Ruling published a commit"
    );
}

#[test]
fn a_policy_published_between_the_gates_and_the_swap_does_not_land() {
    // The same race on the other authority fact: the Ruling copied a Policy revision, and a second
    // session publishes the next one in the window. Drift is a refusal at the last moment it can be
    // read, not a fact the gates cached.
    let fixture = admitted_mission("race-drift");
    let (repository, ruling_digest) = fixture.approved("race-drift");
    let base = repository.revision.clone();
    let path = fixture.store.path.clone();
    let workspace_id = fixture.workspace_id;
    let project_id = fixture.project_id;

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                let others = Fixture::other_session(&path);
                ProjectPolicyService::new(Arc::clone(&others), workspace_id)
                    .publish(project_id, 1, Some(1), policy_draft(2))
                    .unwrap();
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::RulingDrifted);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base,
        "a drifted Ruling published a commit"
    );
}

#[test]
fn a_precondition_that_becomes_unmet_between_the_gates_and_the_swap_does_not_land() {
    // The third authority fact the re-read owes: the Proposal's live readiness. A second session
    // deactivates the Resident the Proposal was built against, which makes a precondition unmet in
    // exactly the window the gates cannot see.
    let fixture = admitted_mission("race-readiness");
    let (repository, ruling_digest) = fixture.approved("race-readiness");
    let base = repository.revision.clone();
    let path = fixture.store.path.clone();
    let workspace_id = fixture.workspace_id;
    let resident_id = fixture.resident_id;

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                let connection = Connection::open(&path).unwrap();
                let decision: Uuid = connection
                    .query_row(
                        "SELECT id FROM resident_lifecycle_decisions
                         WHERE resident_id = ?1 ORDER BY decided_at_ms DESC, id DESC LIMIT 1",
                        [resident_id.to_string()],
                        |row| row.get::<_, String>(0),
                    )
                    .map(|value| Uuid::parse_str(&value).unwrap())
                    .unwrap();
                drop(connection);
                let others = Fixture::other_session(&path);
                ResidentLifecycleService::new(Arc::clone(&others), workspace_id)
                    .retire(resident_id, Some(decision))
                    .unwrap();
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::ProposalNotReady);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base,
        "a Proposal whose preconditions moved published a commit"
    );
}

#[test]
fn a_branch_that_moves_after_the_swap_is_not_pulled_back_by_the_checkout() {
    // The regression a kernel review probed on the real thing: `reset --hard` moves the current
    // branch as well as the index and worktree, which would make the checkout a SECOND ref write
    // after the one irreversible step -- silently overwriting whatever moved the branch in between.
    //
    // The unit test pinned the primitive; this one binds the production call site to the property,
    // by moving the branch in exactly that window. Restoring `reset --hard` makes it red.
    let fixture = admitted_mission("checkout-no-ref-write");
    let (repository, ruling_digest) = fixture.approved("checkout-no-ref-write");
    let head = repository.path.clone();
    let later = Arc::new(Mutex::new(String::new()));
    let recorded = Arc::clone(&later);

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_checkout: Some(Arc::new(move || {
                CopyRepository::git(
                    &head,
                    &["commit", "--quiet", "--allow-empty", "-m", "someone else"],
                );
                *recorded.lock().unwrap() = CopyRepository::git(&head, &["rev-parse", "HEAD"]);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    assert!(receipt.checkout_updated);

    let moved = later.lock().unwrap().clone();
    assert_ne!(moved, receipt.resulting_revision.clone().unwrap());
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        moved,
        "the checkout moved the ref after the compare-and-swap"
    );
}

#[test]
fn a_repository_declared_process_filter_does_not_run_during_the_landing() {
    // The hardened Git command neutralizes every filter driver it can find in the repository's own
    // configuration -- but it could only find drivers declared with `.smudge` or `.clean`. A driver
    // declared with `filter.<name>.process` alone was invisible to the inventory and therefore left
    // armed for the post-swap checkout, which is a repository-supplied program running on the host
    // (gpt1). Nothing else about the hardening changed; the inventory did.
    let fixture = admitted_mission("process-filter");
    let mut repository = CopyRepository::create(&fixture.copies_root, "process-filter");
    let sentinel = repository.path.join("process-filter-ran");
    let filter = repository.path.join(".git/hostile-process-filter");
    fs::write(
        &filter,
        format!("#!/bin/sh\n: > '{}'\nexit 0\n", sentinel.display()),
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();

    // The attributes are IN the pinned base tree, so the checkout after the swap is the invocation
    // that would run the driver. Declared before the Candidate, or the head gate refuses first and
    // this test proves nothing.
    fs::write(
        repository.path.join(".gitattributes"),
        format!("{TARGET} filter=beacon-process\n"),
    )
    .unwrap();
    CopyRepository::git(&repository.path, &["add", ".gitattributes"]);
    CopyRepository::git(
        &repository.path,
        &["commit", "--quiet", "-m", "declare a process filter"],
    );
    repository.revision = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    CopyRepository::git(
        &repository.path,
        &[
            "config",
            "filter.beacon-process.process",
            &filter.to_string_lossy(),
        ],
    );
    CopyRepository::git(
        &repository.path,
        &["config", "filter.beacon-process.required", "true"],
    );

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

    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "the landing never ran, so the filter was never given the chance"
    );
    assert!(receipt.checkout_updated);
    assert!(
        !sentinel.exists(),
        "a repository-declared process filter ran during the landing"
    );
    // And the bytes are the declared ones, not whatever a filter would have made of them.
    assert_eq!(
        fs::read(repository.path.join(TARGET)).unwrap(),
        NEW_BYTES.to_vec()
    );
}

#[test]
fn a_swap_this_attempt_did_not_perform_is_not_reported_as_its_application() {
    // The prepared commit is a deterministic function of its inputs, so a branch can come to name
    // it without this attempt having put it there. The classifier compared the observed head with
    // the prepared commit and called that `Applied` -- a Receipt claiming an act it did not perform
    // (gpt1). It now requires this attempt's own compare-and-swap to have succeeded.
    //
    // Driven exactly: at the failpoint the claim is committed, so a second writer can read the
    // commit this attempt is about to publish and publish it first.
    let fixture = admitted_mission("not-my-swap");
    let (repository, ruling_digest) = fixture.approved("not-my-swap");
    let path = fixture.store.path.clone();
    let head = repository.path.clone();

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                let connection = Connection::open(&path).unwrap();
                let prepared: String = connection
                    .query_row(
                        "SELECT prepared_commit FROM dispatch_preparations",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                CopyRepository::git(&head, &["update-ref", "refs/heads/main", &prepared]);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::LandingRefused);
    assert_eq!(receipt.resulting_revision, None);
    // The branch does name the prepared commit -- someone else put it there, and this Receipt does
    // not claim otherwise.
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        receipt.prepared_commit.clone().unwrap()
    );
    fixture
        .restart()
        .expect("a refusal that measured its prepared commit still replays");
}

#[test]
fn a_repository_that_inherited_a_recorded_identity_is_refused() {
    // Device and inode are the kernel's names for a directory it currently has, and they are reused
    // after a deletion -- so a provenance row alone proves "a directory Beacon made once had these
    // numbers", not "this is that directory" (gpt1). The token proves the second thing.
    //
    // Simulated exactly: the recorded row is kept and the repository's copy of the token is
    // replaced, which is what an unrelated directory inheriting the identity would look like.
    let fixture = admitted_mission("inherited-identity");
    let (repository, ruling_digest) = fixture.approved("inherited-identity");
    CopyRepository::git(
        &repository.path,
        &[
            "config",
            "beacon.provenanceToken",
            "11111111-1111-4111-8111-111111111111",
        ],
    );

    let before = repository.snapshot();
    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::SourceNotBeaconMade);
    assert_eq!(repository.snapshot(), before);
}

#[test]
fn a_repository_that_lost_its_provenance_token_is_refused() {
    // The other shape of the same thing: the row is there and the repository can no longer produce
    // the token. Absent is not "unset and therefore fine"; it is a repository that cannot show it
    // is the one Beacon made.
    let fixture = admitted_mission("token-removed");
    let (repository, ruling_digest) = fixture.approved("token-removed");
    CopyRepository::git(
        &repository.path,
        &["config", "--unset", "beacon.provenanceToken"],
    );

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::SourceNotBeaconMade);
}

#[test]
fn a_branch_moved_between_the_gates_and_the_swap_is_not_clobbered() {
    // The compare-and-swap's own service-level property, with nothing else standing in for it.
    //
    // The concurrent double-dispatch test cannot prove this: the one-use claim stops the second
    // dispatcher before it reaches the swap, so removing the expected old value leaves that test
    // green (gpt1). Here the competing write is not a second dispatch at all -- it is ordinary
    // history, arriving after every gate has read the head and before the swap -- and the exact-base
    // compare-and-swap is the only thing that can refuse it.
    let fixture = admitted_mission("cas-bound");
    let (repository, ruling_digest) = fixture.approved("cas-bound");
    let head = repository.path.clone();
    let later = Arc::new(Mutex::new(String::new()));
    let recorded = Arc::clone(&later);

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                CopyRepository::git(
                    &head,
                    &[
                        "commit",
                        "--quiet",
                        "--allow-empty",
                        "-m",
                        "ordinary history",
                    ],
                );
                *recorded.lock().unwrap() = CopyRepository::git(&head, &["rev-parse", "HEAD"]);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::LandingRefused);
    assert_eq!(receipt.resulting_revision, None);
    let moved = later.lock().unwrap().clone();
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        moved,
        "the swap overwrote a commit that arrived after the head gate"
    );
    // The declared bytes never reached the working tree either.
    assert_ne!(
        fs::read(repository.path.join(TARGET)).unwrap(),
        NEW_BYTES.to_vec()
    );
}
