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
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use beacond::{
    AccessOrigin, AutonomyProfile, BeaconCapability, CandidateImages, CandidateService,
    CopyFailpoints, DeclaredCheck, DispatchError, DispatchFailpoints, DispatchOutcome,
    DispatchService, FilterInventoryFailure, LocalModelActivationService, LocalModelEndpoint,
    MissionDraft, MissionMemberDraft, MissionService, MutationEffect, NotVerified,
    ObservedPreImage, OperatorAccountService, OperatorSessionPhase, PauseService,
    PinnedReadFailure, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService,
    ProjectRepositoryService, ProjectService, ProposalService, ProviderFeature, RepositoryDraft,
    RepositoryEffect, RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService,
    RulingDecision, RulingService, RulingSupersedes, SampleProjectError, SampleProjectService,
    SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    Supervisor, SupervisorError, SuppliedContent, TeamDraft, TeamLeadAmbiguityRule,
    TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft,
    TeamService, TrackRecordEvidenceKind, TrackRecordRequirement,
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

        // R58: these batches rewrite stored Events in place -- a v3 Receipt's payload, a forged
        // era -- which is precisely what the Event chain refuses as `LinkDigestMismatch`. It is
        // the chain working, not a defect, and it is why the tamper says so afterwards: a store
        // whose history a fixture rewrote is no longer a chained store, and it declares that
        // rather than forging links to hide it. The Receipt-era question each of these tests asks
        // is then reachable again, which is the only reason to do it.
        connection
            .execute_batch(
                "UPDATE events
                    SET predecessor_kind = 'unchained', predecessor_digest = NULL,
                        link_digest = NULL;
                 UPDATE event_chain_state
                    SET genesis_kind = 'migrated',
                        migrated_through_sequence = (SELECT max(sequence) FROM events),
                        head_digest = NULL,
                        head_sequence = (SELECT max(sequence) FROM events)
                  WHERE id = 1;",
            )
            .unwrap();
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

/// R51: R49's fd-100 collision, at the level a dispatch actually fails on.
///
/// `worktree::tests::a_git_directory_that_opens_on_the_placement_descriptor_still_reaches_the_child`
/// proves the mechanism at the read: with the descriptor table grown past the placement number,
/// `openat(.git)` returns that very number, `dup2(n, n)` is a successful no-op, close-on-exec
/// survives, and the exec closes the pin. This is the same condition through the PUBLIC services --
/// a real dispatch, which is where the loss was actually met: it arrived four gates later as a
/// content disagreement about a repository that was never read.
///
/// The descriptors are held across the whole dispatch rather than around one open, because the
/// dispatch opens the repository itself and this test cannot hand it a pinned one. Placement is not
/// asserted directly for the same reason; what proves this test reaches the condition is its
/// MUTANT: removing `enter()`'s `F_SETFD` branch makes this dispatch refuse.
///
/// WHICH GATE IT REFUSES AT, measured rather than predicted. A pin lost from the FIRST exec is not
/// met at the pre-image gate: the earliest pinned read is the filter-driver inventory, inside
/// `read_pinned`, and that read serves the provenance gate (`provenance_of`), which runs well
/// before the pre-image gate.
///
/// blue's flake reached the pre-image gate instead because its loss was TRANSIENT -- it arrived
/// after the early gates had already read successfully. A deterministic loss cannot get that far.
///
/// R52 UPDATED THIS EXPECTATION, and measured it rather than predicting it
/// (`tools/r52/logs/measure-r51-mutant.log`). R51 recorded that the mutant refuses
/// `SourceNotBeaconMade` and that git's message was thrown away by `read_pinned`'s `.ok()?`. Both
/// halves have moved: the mutant now refuses `FilterInventoryUnreadable` carrying
/// `FilterInventoryFailure::Refused { message }`, and the message REACHES the Receipt --
///
///   fatal: unable to read config file '/proc/self/fd/100/config': No such file or directory
///
/// which is also the correction to R51's own quotation of it. The first child through the lost pin
/// is `config --file /proc/self/fd/100/config` -- the inventory read -- so git complains about the
/// config FILE, not about the repository. `fatal: not a git repository: '/proc/self/fd/100'` is
/// what a later child would have said, and no later child runs.
///
/// (R50 merged first, so the outcome above is the inventory's own from the start; the measured
/// log predates that fold and names the `PreImageUnreadable` it carried then.)
#[test]
fn a_dispatch_whose_git_directory_lands_on_the_placement_descriptor_still_applies() {
    // `PINNED_GIT_DIR_FD` is crate-internal, so the number is stated once here with the reason it
    // is fixed at all: `GIT_DIR` is baked before the fork, so the destination cannot be chosen at
    // run time. If the constant ever moves, this test stops reproducing R49 rather than failing --
    // which is what the mutant in the report actually guards.
    const PLACEMENT_DESCRIPTOR: std::os::fd::RawFd = 100;

    let fixture = admitted_mission("fd100-dispatch");
    let (repository, ruling_digest) = fixture.approved("fd100-dispatch");
    let base = repository.revision.clone();

    // Two descriptors below the placement number are left free -- what `PinnedRepository::open`
    // consumes in order, the root and then `.git` through it -- so the dispatch's own open lands
    // the Git directory exactly on the placement descriptor.
    let mut held: Vec<fs::File> = Vec::new();
    loop {
        let file = fs::File::open("/dev/null").expect("/dev/null opens");
        let number = file.as_raw_fd();
        if number > PLACEMENT_DESCRIPTOR - 2 {
            break;
        }
        held.push(file);
        if number == PLACEMENT_DESCRIPTOR - 2 {
            break;
        }
    }

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    drop(held);

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "R51.FD100.APPLIES -- pre-image read failure: {:?}",
        receipt.pre_image_read_failure
    );
    let landed = receipt
        .resulting_revision
        .clone()
        .expect("an applied dispatch names the revision it produced");
    assert_ne!(landed, base, "R51.FD100.BRANCH_MOVED");
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        landed,
        "R51.FD100.HEAD_IS_THE_LANDED_COMMIT"
    );
}

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
    // R46.7.1: version 4 is the R46 era.
    assert_eq!(receipt.version, 4);
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
    assert_eq!(third.repository_effect(), RepositoryEffect::ByteIdentical);
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

/// R55.2 CONTROL: a detached HEAD is `SourceDetached` through the TYPED `read_pinned`.
///
/// This was briefed as a RED probe -- "after the R52 rebase, detached HEAD would read as an
/// unreadable inventory" -- and it is GREEN on the bare rebase, which is why it is committed as a
/// control rather than as a fix's bar. The reason it is green is the one R52 wrote down: R52 typed
/// only the INVENTORY onto the `Err` channel. The child's own answers, including R55's new
/// `Unreadable(SilentExit { code })` for a silent non-zero at a `Silence` site, still fold to
/// `Ok(None)` inside `read_pinned`. So `symbolic-ref --quiet HEAD` exiting 1 in silence arrives at
/// the gate as `Ok(None)` and refuses `SourceDetached`, exactly as before R52 and before R55.
///
/// The hazard the ruling is about is REAL but not yet reachable: R55 classifies that documented
/// "detached" answer as a FAILURE (`SilentExit`), which is only harmless because the fold erases
/// the difference. The lane that types the child's answers -- the residual R52 named, and
/// DP-r52.1's subject -- is the one where detached HEAD would start refusing as a read failure.
/// THIS TEST IS THAT TRIPWIRE: it fails the moment someone types the child's answers without
/// giving `symbolic-ref` its own witness.
///
/// It also asserts the negative, because asserting only `SourceDetached` would pass if the gate
/// stopped being reached at all.
#[test]
fn a_detached_head_is_detached_and_never_an_unreadable_read() {
    let fixture = admitted_mission("detached-not-unreadable");
    let (repository, ruling_digest) = fixture.approved("detached-not-unreadable");
    CopyRepository::git(
        &repository.path,
        &["checkout", "--quiet", "--detach", &repository.revision],
    );
    let before = repository.snapshot();

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::SourceDetached,
        "R55.2.DETACHED_STAYS_DETACHED"
    );
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::FilterInventoryUnreadable,
        "R55.2.DETACHED_IS_NOT_AN_UNREADABLE_INVENTORY -- a documented negative answer was \
         reported as a read that could not be made"
    );
    assert_eq!(
        receipt.filter_inventory_failure, None,
        "R55.2.DETACHED_CARRIES_NO_READ_FAILURE"
    );
    assert_eq!(repository.snapshot(), before);
}

/// R52.1: a repository broken at the provenance gate is never "not Beacon-made".
///
/// DP-r52.1 row 7 said the provenance TOKEN child's own failure folds into `SourceNotBeaconMade` --
/// a read that produced no answer deciding a fact about the source. MEASURED at this head, it does
/// not, and the reason is R52 itself: the token read and the filter-inventory read are the same
/// `read_pinned` call, in that order, **against the same file**. Whatever breaks the token child
/// has already broken the inventory child one step earlier, and R52 gave that refusal its own
/// outcome and git's own message.
///
/// So this is a TRIPWIRE, not a fix's bar: it pins that the earlier gate keeps catching these, and
/// fails the day someone reorders the two reads or points them at different paths. It asserts the
/// negative first, because asserting only the inventory outcome would pass if the gate stopped
/// being reached at all.
#[test]
fn a_repository_broken_at_the_provenance_gate_is_never_reported_as_not_beacon_made() {
    // The `.git` directory swapped: the inventory reads through the pinned DESCRIPTOR, so its
    // child succeeds -- and its pin check then fails, because the pathname now names a different
    // directory. `Unreadable`, carrying no message, because git said nothing; the pin did.
    let fixture = admitted_mission("r521-pin-swap");
    let (repository, ruling_digest) = fixture.approved("r521-pin-swap");
    let git_dir = repository.path.join(".git");
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_provenance_read: Some(Arc::new(move || {
                let moved = git_dir.with_extension("moved");
                if git_dir.exists() {
                    fs::rename(&git_dir, &moved).unwrap();
                    fs::create_dir(&git_dir).unwrap();
                }
            })),
            ..DispatchFailpoints::default()
        })
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::SourceNotBeaconMade,
        "R52.1.PIN_LOST_IS_NOT_A_CLAIM_ABOUT_THE_SOURCE"
    );
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::FilterInventoryUnreadable,
        "R52.1.PIN_LOST_IS_CAUGHT_AT_THE_INVENTORY"
    );
    assert_eq!(
        receipt.filter_inventory_failure,
        Some(FilterInventoryFailure::Unreadable)
    );
}

/// R52.1: a missing configuration file is "this repository cannot be read", not "no token".
///
/// grok measured the collision this was meant to disentangle: `config --get` exits 1 in silence
/// for a key that is unset AND for a file that is not there
/// (`tools/r52.1/logs/measure-token-child-b1203fd.log`). The brief asked for a file-presence check
/// at the token site to keep the two apart.
///
/// It is not needed, and this is the measurement that says so: the inventory read reaches the same
/// file first with `--list`, which exits **128 with `fatal: unable to read config file`**, so the
/// distinction is already made -- and made with git's own words, which a presence check could not
/// have produced.
#[test]
fn a_missing_configuration_file_is_refused_with_gits_own_words_not_as_an_absent_token() {
    let fixture = admitted_mission("r521-config-gone");
    let (repository, ruling_digest) = fixture.approved("r521-config-gone");
    let config = repository.path.join(".git").join("config");
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_provenance_read: Some(Arc::new(move || {
                fs::remove_file(&config).unwrap();
            })),
            ..DispatchFailpoints::default()
        })
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::SourceNotBeaconMade,
        "R52.1.NO_CONFIG_IS_NOT_AN_ABSENT_TOKEN"
    );
    let Some(FilterInventoryFailure::Refused { message }) =
        receipt.filter_inventory_failure.as_ref()
    else {
        panic!(
            "R52.1.NO_CONFIG_CARRIES_GITS_WORDS -- {:?}",
            receipt.filter_inventory_failure
        );
    };
    assert!(
        message.contains("unable to read config file"),
        "R52.1.NO_CONFIG_CARRIES_GITS_WORDS -- {message}"
    );
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
// The copy's creation-to-first-open window
// ---------------------------------------------------------------------------------------------

/// How many repositories Beacon has recorded as its own.
fn beacon_repository_count(store: &Connection) -> i64 {
    store
        .query_row("SELECT COUNT(*) FROM beacon_repositories", [], |row| {
            row.get(0)
        })
        .unwrap()
}

/// The component belongs to this fixture's copies root, not to another test running in the same
/// process.
fn in_copies_root(created: &Path, copies_root: &Path) -> bool {
    created
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .is_some_and(|parent| parent == copies_root)
}

/// Move Beacon's own child aside and put a directory the test made at that component.
///
/// Beacon's child is moved rather than deleted, so it still exists: a check that only asked "is
/// there a directory here?" would still say yes, and one that compared the name to Beacon's inode
/// after this has something real to be wrong about.
fn swap_in_impostor(created: &Path, copies_root: &Path) -> bool {
    if !in_copies_root(created, copies_root) {
        return false;
    }
    let aside = created.with_file_name("beacons-own-child");
    fs::rename(created, &aside).unwrap();
    fs::create_dir(created).unwrap();
    true
}

/// Hand the component back to Beacon's own child, discarding the impostor.
///
/// This is what makes a substitution visible to exactly ONE of the two identity comparisons: the
/// descriptor was opened on the impostor, but by the time the staging name is checked it names
/// Beacon's directory again.
fn restore_beacons_child(created: &Path, copies_root: &Path) -> bool {
    if !in_copies_root(created, copies_root) {
        return false;
    }
    let aside = created.with_file_name("beacons-own-child");
    if !aside.is_dir() {
        return false;
    }
    fs::remove_dir(created).unwrap();
    fs::rename(&aside, created).unwrap();
    true
}

#[test]
fn the_directory_beacon_opens_must_be_the_one_beacon_created() {
    // gpt1's probe, committed: the replacement is installed in the create-to-open window and LEFT
    // there. Nothing is restored, so this is the shape a real substitution has.
    //
    // It is the scenario that pins WHEN the inode is captured. Reading it after `openat` instead of
    // before makes both comparisons compare the impostor with itself, and everything downstream
    // agrees: the copy is written into the replacement, the provenance token is generated for it,
    // and `copy_for_dispatch` returns `Ok`. That mutant is invisible to the two tests below, which
    // both restore or defer the swap -- measured, not assumed.
    let fixture = admitted_mission("copy-probe");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();

    let copies_root = fs::canonicalize(&fixture.copies_root).unwrap();
    let fired = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&fired);
    let samples = samples.with_copy_failpoints(CopyFailpoints {
        before_open: Some(Arc::new(move |created: &Path| {
            let mut seen = observed.lock().unwrap();
            if !*seen && swap_in_impostor(created, &copies_root) {
                *seen = true;
            }
        })),
        ..CopyFailpoints::default()
    });
    let outcome = samples.copy_for_dispatch(&fixture.copies_root, "probe");

    assert!(
        *fired.lock().unwrap(),
        "the failpoint never fired, so this test proves nothing"
    );
    assert!(
        matches!(outcome, Err(SampleProjectError::UnsafeDestination)),
        "the directory Beacon opens must be the one Beacon created, got {outcome:?}"
    );
    assert_eq!(beacon_repository_count(&fixture.store.connect()), 0);
    assert!(!fixture.copies_root.join("sample-project-probe").exists());
}

#[test]
fn a_descriptor_that_is_not_the_directory_beacon_created_is_refused() {
    // `copy_for_dispatch` creates the copy's root with `mkdirat` and opens it with `openat`: two
    // syscalls, and POSIX offers no `O_CREAT` for a directory to make them one. A review replaced
    // the final component in exactly that interval, and the previous build answered with the
    // replacement -- which then received a freshly generated provenance token and was recorded in
    // `beacon_repositories` as Beacon-created, with `copy_for_dispatch` returning `Ok`.
    //
    // Every run of that probe was a reviewer's temporary patch, because nothing in the tree could
    // reach the interval. This is the first committed test of the seam.
    //
    // The substitution here is undone before the staging name is checked, so the NAME comparison
    // cannot see it: only the comparison between the opened descriptor and the inode `mkdirat`
    // produced can. That is deliberate -- with both comparisons firing on one scenario, deleting
    // either left the test green.
    let fixture = admitted_mission("copy-swap");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();
    assert_eq!(beacon_repository_count(&fixture.store.connect()), 0);

    let copies_root = fs::canonicalize(&fixture.copies_root).unwrap();
    let swap_root = copies_root.clone();
    let fired = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&fired);
    let samples = samples.with_copy_failpoints(CopyFailpoints {
        // The impostor is in place for the length of the create-to-open window...
        before_open: Some(Arc::new(move |created: &Path| {
            let mut seen = observed.lock().unwrap();
            if !*seen && swap_in_impostor(created, &swap_root) {
                *seen = true;
            }
        })),
        // ...and the name is handed back before it is checked, so only the descriptor can tell.
        after_open: Some(Arc::new(move |created: &Path| {
            restore_beacons_child(created, &copies_root);
        })),
        ..CopyFailpoints::default()
    });
    let outcome = samples.copy_for_dispatch(&fixture.copies_root, "swap");

    assert!(
        *fired.lock().unwrap(),
        "the failpoint never fired, so this test proves nothing"
    );
    assert!(
        matches!(outcome, Err(SampleProjectError::UnsafeDestination)),
        "a descriptor that is not the directory Beacon made must be refused, got {outcome:?}"
    );
    // The refusal happens before provenance exists: nothing recorded, and the published name never
    // created, so a later run cannot adopt what this one left behind.
    assert_eq!(beacon_repository_count(&fixture.store.connect()), 0);
    assert!(!fixture.copies_root.join("sample-project-swap").exists());
}

#[test]
fn a_staging_name_that_stops_naming_beacons_directory_is_not_published() {
    // The other half. Here the descriptor IS Beacon's -- the substitution happens after `openat` --
    // so the descriptor comparison passes and only the check that the staging name still refers to
    // that inode can refuse. Without it, `renameat` would publish the impostor under the name the
    // copy is about to be written to.
    let fixture = admitted_mission("copy-rename");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();

    let copies_root = fs::canonicalize(&fixture.copies_root).unwrap();
    let fired = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&fired);
    let samples = samples.with_copy_failpoints(CopyFailpoints {
        after_open: Some(Arc::new(move |created: &Path| {
            let mut seen = observed.lock().unwrap();
            if !*seen && swap_in_impostor(created, &copies_root) {
                *seen = true;
            }
        })),
        ..CopyFailpoints::default()
    });
    let outcome = samples.copy_for_dispatch(&fixture.copies_root, "rename");

    assert!(
        *fired.lock().unwrap(),
        "the failpoint never fired, so this test proves nothing"
    );
    assert!(
        matches!(outcome, Err(SampleProjectError::UnsafeDestination)),
        "a staging name that no longer names Beacon's directory must not be published, got {outcome:?}"
    );
    assert_eq!(beacon_repository_count(&fixture.store.connect()), 0);
    assert!(!fixture.copies_root.join("sample-project-rename").exists());
}

#[test]
fn an_unsubstituted_copy_is_made_and_recorded() {
    // The counterexample: without a failpoint the same call succeeds and records exactly one
    // repository. Without this, the two tests above are satisfiable by a `copy_for_dispatch` that
    // refuses everything.
    let fixture = admitted_mission("copy-clean");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();

    let copy = samples
        .copy_for_dispatch(&fixture.copies_root, "clean")
        .unwrap();

    assert_eq!(copy, fixture.copies_root.join("sample-project-clean"));
    assert!(copy.join("README.md").is_file());
    assert_eq!(beacon_repository_count(&fixture.store.connect()), 1);
    // Published under the name callers use, with no staging component left behind.
    let leftovers: Vec<_> = fs::read_dir(&fixture.copies_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with('.'))
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging names left behind: {leftovers:?}"
    );
    // Mode 0700, set on the descriptor.
    assert_eq!(
        fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn a_published_copy_name_cannot_redirect_writes_before_the_descriptor_is_reopened() {
    let fixture = admitted_mission("copy-after-publish");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();

    let victim = fixture
        .copies_root
        .parent()
        .unwrap()
        .join(format!("dispatch-copy-victim-{}.md", Uuid::new_v4()));
    fs::write(&victim, b"operator bytes\n").unwrap();
    let victim_for_hook = victim.clone();
    let fired = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&fired);
    let samples = samples.with_copy_failpoints(CopyFailpoints {
        after_publish: Some(Arc::new(move |published: &Path| {
            let mut seen = observed.lock().unwrap();
            if *seen {
                return;
            }
            let aside = published.with_file_name("beacons-published-child");
            fs::rename(published, aside).unwrap();
            fs::create_dir(published).unwrap();
            std::os::unix::fs::symlink(&victim_for_hook, published.join("README.md")).unwrap();
            *seen = true;
        })),
        ..CopyFailpoints::default()
    });

    let outcome = samples.copy_for_dispatch(&fixture.copies_root, "after-publish");
    let victim_bytes = fs::read(&victim).unwrap();
    let _ = fs::remove_file(&victim);

    assert!(
        *fired.lock().unwrap(),
        "the post-publication seam was not reached"
    );
    assert!(matches!(
        outcome,
        Err(SampleProjectError::UnsafeDestination)
    ));
    assert_eq!(
        victim_bytes, b"operator bytes\n",
        "path-based copy_tree followed the replacement's child symlink and wrote outside Beacon's directory"
    );
}

#[test]
fn a_symlink_planted_inside_beacons_own_copy_is_refused_rather_than_followed() {
    // The second witness, and the one sol1's probe cannot be: after the descriptor fix, its
    // arrangement is answered by copying through the descriptor, so `O_NOFOLLOW`/`O_EXCL` on each
    // entry survives every mutant it can see. Here the plant is INSIDE the directory Beacon
    // published and still holds -- which an attacker who can rename in the copies root can also do
    // -- so the descriptor is no defence and only the open flags are.
    //
    // Without them the copy writes through the link and the victim outside the copies root gets the
    // bundled README; with them the entry already exists and is not a regular file, and the copy
    // refuses.
    let fixture = admitted_mission("copy-plant-inside");
    let samples = SampleProjectService::new(
        Arc::clone(&fixture.accounts),
        fixture.workspace_id,
        &fixture.store.path,
    )
    .unwrap();
    samples.install().unwrap();

    let victim = fixture
        .copies_root
        .parent()
        .unwrap()
        .join(format!("dispatch-copy-victim-inside-{}.md", Uuid::new_v4()));
    fs::write(&victim, b"operator bytes\n").unwrap();
    let victim_for_hook = victim.clone();
    let fired = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&fired);
    let samples = samples.with_copy_failpoints(CopyFailpoints {
        after_publish: Some(Arc::new(move |published: &Path| {
            let mut seen = observed.lock().unwrap();
            if *seen {
                return;
            }
            std::os::unix::fs::symlink(&victim_for_hook, published.join("README.md")).unwrap();
            *seen = true;
        })),
        ..CopyFailpoints::default()
    });

    let outcome = samples.copy_for_dispatch(&fixture.copies_root, "plant-inside");
    let victim_bytes = fs::read(&victim).unwrap();
    let _ = fs::remove_file(&victim);

    assert!(
        *fired.lock().unwrap(),
        "the post-publication seam was not reached"
    );
    assert!(
        outcome.is_err(),
        "a copy over a planted symlink inside Beacon's own directory succeeded"
    );
    assert_eq!(
        victim_bytes, b"operator bytes\n",
        "the copy followed a symlink planted inside the directory it holds"
    );
}

// ---------------------------------------------------------------------------------------------
// The walking skeleton
// ---------------------------------------------------------------------------------------------

/// BLUEPRINT alpha criterion: "Two Workspaces in one Installation cannot see, spend, EXECUTE, or
/// reuse each other's state or authority by default."
///
/// The execute clause, which had no bar of its own. The reads have several
/// (`artifact :: a_cross_workspace_read_is_not_found_never_a_value`,
/// `mission :: a_cross_workspace_read_is_a_refusal_and_not_an_empty_result`,
/// `outcome_collector :: collector_refuses_cross_workspace_reference_even_when_ids_exist`); nothing
/// asked whether one Workspace can make another Workspace's approved change HAPPEN.
///
/// The attempt is the strongest one available: a real Ruling, approved by the Operator in the
/// Workspace that owns it, dispatched through a service scoped to a different Workspace. Every
/// identity is genuine; only the scope differs.
///
/// The refusal has to be BY NAME. `RulingNotFound` says "no Ruling in this Workspace has that
/// digest", which is the true statement; a silent empty result, or an `InvalidStore`, would report
/// a broken installation for what is an ordinary boundary holding.
#[test]
fn a_ruling_cannot_be_dispatched_from_another_workspace() {
    let fixture = admitted_mission("execute-cross-workspace");
    let repository = CopyRepository::create(&fixture.copies_root, "execute-cross-workspace");
    let (proposal_digest, _) = fixture.candidate_for(&repository, "Rewrite it.", BASE_LIB_BYTES);
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    let head_before = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    let bytes_before = fs::read(repository.path.join(TARGET)).unwrap();

    // A second Workspace in the same Installation. It exists as a row, the way a real one would.
    let elsewhere = Uuid::new_v4();
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
            [elsewhere.to_string()],
        )
        .unwrap();

    let stranger = DispatchService::new(Arc::clone(&fixture.accounts), elsewhere);
    let refusal = stranger
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .expect_err("another Workspace dispatched this Workspace's Ruling");
    assert!(
        matches!(refusal, DispatchError::RulingNotFound),
        "the crossing was refused, but not as a Ruling this Workspace does not have: {refusal:?}"
    );

    // Refused is not enough: nothing may have happened. The repository is byte-identical, its head
    // has not moved, and no Receipt was written -- a refusal that still executed would satisfy the
    // assertion above and break the criterion.
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        head_before,
        "the foreign dispatch moved the head it was refused"
    );
    assert_eq!(
        fs::read(repository.path.join(TARGET)).unwrap(),
        bytes_before,
        "the foreign dispatch wrote the bytes it was refused"
    );
    assert_eq!(
        fixture
            .store
            .connect()
            .query_row("SELECT count(*) FROM receipts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "the foreign dispatch recorded a Receipt"
    );
}

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
    assert_eq!(
        receipt.repository_effect(),
        RepositoryEffect::UnreferencedObjects
    );
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
fn a_receipt_cannot_evade_startup_replay_by_losing_its_event() {
    // R48 bar (b): the Run verifier owns the cardinality as well as the Event replay. A Receipt
    // whose terminal Event is gone must not become invisible merely because the Event walk has
    // nothing left to decode.
    let fixture = admitted_mission("receipt-without-event");
    let (_repository, ruling_digest) = fixture.approved("receipt-without-event");
    fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    let deleted = fixture
        .store
        .connect()
        .execute("DELETE FROM events WHERE kind = 'proposal.dispatched'", [])
        .unwrap();
    assert_eq!(deleted, 1, "the bar did not remove the terminal Event");
    assert_eq!(fixture.count("SELECT count(*) FROM receipts"), 1);

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::RunStateMismatch { .. })
        ),
        "a terminal Receipt with no Event evaded startup replay"
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
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
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
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(receipt.resulting_revision, None);
}

/// R46.7.1: a Receipt written BEFORE this lane carries no observed pre-image, and startup accepts
/// it as UNRECORDED -- never repaired, never invented.
///
/// The lockout R46.7 named, in the other direction: item 6's owner rule refuses a mismatch with no
/// observation, and every historical mismatch has none. Keying the rule on the Receipt's VERSION
/// rather than on the field's presence is what keeps an upgraded store readable.
/// R54 (sol1's cold read): the era rule was ONE-SIDED. A version-3 Receipt is read as
/// "unrecorded" (R46.7.1), and nothing checked that it actually WAS unrecorded -- so a hand-written
/// hybrid carrying the version-4-only observed field was accepted as a legacy record. An era marker
/// that only licenses absence is not a marker; it is a hole shaped like one.
#[test]
fn sol1_a_version_three_receipt_cannot_carry_version_four_observed_evidence() {
    let fixture = admitted_mission("v3-carries-v4-observation");
    let repository = CopyRepository::create(&fixture.copies_root, "v3-carries-v4-observation");
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
    assert_eq!(receipt.version, 4);
    assert!(receipt.per_mutation[0].observed_pre_image.is_some());

    // Hand-write the Event/Receipt projection as a v3 wire record but retain the field that v3
    // could not have emitted. The typed owner, not serde's permissive struct decoder, must decide
    // whether this hybrid has a meaning.
    fixture.without_run_triggers(
        "UPDATE receipts
            SET version = 3,
                receipt_json = json_set(receipt_json, '$.version', 3);
         UPDATE events
            SET payload_json = json_set(payload_json, '$.version', 3)
          WHERE kind = 'proposal.dispatched';",
    );
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::DispatchEventMismatch(_))
        ),
        "R46.LEGACY.OBSERVATION_DOMAIN: a v3 Receipt carrying v4-only observedPreImage opened Ready"
    );
}

#[test]
fn an_older_schema_reader_accepts_legacy_receipts_but_refuses_a_future_outcome() {
    let fixture = admitted_mission("legacy-mismatch");
    let repository = CopyRepository::create(&fixture.copies_root, "legacy-mismatch");
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

    // Rewrite the stored Receipt and its Event as a version-3 record with no observed field: what
    // this store would hold had the dispatch run before R46. The two copies stay byte-identical,
    // because the owner compares them.
    let connection = Connection::open(&fixture.store.path).unwrap();
    let stored: String = connection
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&stored).unwrap();
    value["version"] = serde_json::json!(3);
    for record in value["perMutation"].as_array_mut().unwrap() {
        record.as_object_mut().unwrap().remove("observedPreImage");
    }
    let legacy = serde_json::to_string(&value).unwrap();
    assert!(
        !legacy.contains("observedPreImage"),
        "the legacy Receipt still carries the field"
    );
    // Receipts are immutable, so the trigger comes off and goes back byte-exactly as 0033 wrote
    // it -- the schema signature is compared at startup and an indented re-CREATE would refuse
    // before this test's own question was reached.
    connection
        .execute_batch("DROP TRIGGER receipts_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE receipts SET version = 3, receipt_json = ?1",
            [legacy.as_str()],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER receipts_no_update
BEFORE UPDATE ON receipts
BEGIN
    SELECT RAISE(ABORT, 'Receipts are immutable');
END;",
        )
        .unwrap();
    connection
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE kind = 'proposal.dispatched'",
            [legacy.as_str()],
        )
        .unwrap();
    // R58: rewriting a stored Event's payload is what the chain refuses as `LinkDigestMismatch`,
    // so this store says what it now is -- one whose history a fixture rewrote, chained from here
    // rather than from its genesis. Without it the chain answers before the Receipt-era question
    // this test asks.
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
    drop(connection);

    fixture
        .restart()
        .expect("a version-3 mismatch without the observed field is unrecorded, not incoherent");

    // R48 bar (c): this process stands in for the older binary. Both stores have today's schema,
    // but only the historical Receipt belongs to its typed vocabulary. A token written by a
    // hypothetical future binary is SQL-valid after 0034 and still unreadable here.
    let future = admitted_mission("future-outcome");
    let (_repository, ruling_digest) = future.approved("future-outcome");
    future
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    assert_eq!(
        future
            .store
            .connect()
            .query_row("SELECT version FROM schema_meta", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        36
    );
    future.without_run_triggers(
        "UPDATE receipts
            SET outcome = 'future_outcome',
                receipt_json = json_set(receipt_json, '$.outcome', 'future_outcome');
         UPDATE events
            SET payload_json = json_set(payload_json, '$.outcome', 'future_outcome')
          WHERE kind = 'proposal.dispatched';",
    );
    assert!(
        matches!(
            future.restart(),
            Err(SupervisorError::DispatchEventMismatch(_))
        ),
        "the older typed owner returned a value for a future outcome token"
    );
}

#[test]
fn astra_a_truthful_absent_pre_image_mismatch_remains_readable_after_restart() {
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
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        head_before
    );
    assert_eq!(receipt.resulting_revision, None);
    // astra wrote this line as `observed_pre_image.is_none()`, which was the shape of the DEFECT:
    // the absence went unrecorded and the owner then refused the store. R46.7 rules that an
    // observed absence is a VALUE, so the assertion becomes the ruling's shape -- still "no byte
    // digest", now stated as the observation it is. The probe's real property, the restart below,
    // is unchanged.
    assert!(
        receipt
            .per_mutation
            .iter()
            .all(|record| matches!(record.observed_pre_image, Some(ObservedPreImage::Absent))),
        "R46.MISMATCH.ABSENCE_HAS_NO_BYTE_DIGEST"
    );
    fixture
        .restart()
        .expect("R46.MISMATCH.OBSERVED_ABSENCE_REOPENS");
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
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
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
fn a_recovery_that_cannot_name_its_run_is_a_failed_write() {
    // `finish_prepared_dispatches` is the startup reconciliation step that turns an interrupted
    // attempt into a Run with a terminal Receipt. Nothing here asserted what startup
    // says when that write cannot complete, so the name it reported was never compile-checked by
    // anything -- which is how it kept a `...StateMismatch` name while not being a verification
    // pass (R28.1).
    //
    // The failure is driven, not argued: both sides of the preparation-to-Run join are given an id
    // that is not a UUID, so the recovery reads a Run it cannot name. Both immutability triggers
    // are dropped and restored around the write, and BOTH sides are changed together -- changing
    // only the Run leaves the preparation dangling, and `PRAGMA foreign_key_check` then refuses the
    // store before this pass is ever reached.
    let fixture = admitted_mission("recovery-unnameable");
    let (_repository, ruling_digest) = fixture.approved("recovery-unnameable");
    fixture.dispatch_until_death(&ruling_digest, "after_swap");
    assert_eq!(fixture.count("SELECT count(*) FROM receipts"), 0);

    let connection = fixture.store.connect();
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    let triggers: Vec<String> = [
        "runs_single_terminal_transition",
        "dispatch_preparations_no_update",
    ]
    .iter()
    .map(|name| {
        connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = ?1",
                [name],
                |row| row.get::<_, String>(0),
            )
            .unwrap()
    })
    .collect();
    connection
        .execute_batch(
            "DROP TRIGGER runs_single_terminal_transition;
             DROP TRIGGER dispatch_preparations_no_update;
             UPDATE runs SET id = 'not-a-uuid' WHERE kind = 'proposal_dispatch';
             UPDATE dispatch_preparations SET run_id = 'not-a-uuid';",
        )
        .unwrap();
    for trigger in &triggers {
        connection.execute_batch(trigger).unwrap();
    }
    drop(connection);

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::DispatchRecoveryFailed)
        ),
        "a recovery write that could not complete did not say so at startup"
    );
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
fn an_interruption_before_the_claim_is_terminal_and_says_what_it_cannot_know() {
    // The earliest boundary: the Run is open, nothing was claimed, and whether any object reached
    // the object database is unknown -- a death here can fall before the first write as easily as
    // after it. The honest terminal record says the landing result is unobserved and that any
    // residue is unknown. A guess in either direction would be worse than the uncertainty.
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
            .any(|entry| entry == "object_residue_unknown"),
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

/// The top-level member names in the order the stored bytes carry them.
///
/// Read off the text rather than out of a parsed map: `serde_json`'s map is sorted, so parsing and
/// reading its keys would assert alphabetical order and pass no matter how the record is written.
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
    assert_eq!(value["version"], 4);
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
    // Every invocation this dispatch made on its own behalf, in the order it made them: the
    // provenance token read, the pre-image read, the three live gate reads, the object writes, the
    // post-condition's diff and its re-read of the prepared commit's own blob, the swap, the
    // observation of what the swap produced, and the checkout.
    assert_eq!(
        steps,
        vec![
            "config",
            "cat_file",
            "symbolic_ref",
            "rev_parse",
            "status",
            "read_tree",
            "hash_object",
            "update_index",
            "write_tree",
            "commit_tree",
            "diff_name_status",
            "cat_file",
            "update_ref",
            "rev_parse",
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

    // The two invocations whose argv used to be written twice -- once to run and once to record --
    // asserted in FULL, because the half that mattered was the half a prefix assertion stopped
    // before. Dropping the old value from `update-ref` turns a compare-and-swap into a plain
    // update; a Receipt that still spelled four arguments while three ran would be a record of an
    // act nobody performed (Fable).
    let argv_of = |name: &str| -> Vec<String> {
        value["gitSteps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|step| step["step"] == name)
            .unwrap_or_else(|| panic!("no {name} step in the recorded list"))["argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|argument| argument.as_str().unwrap().to_owned())
            .collect()
    };
    let branch = value["branch"].as_str().unwrap();
    let prepared = value["preparedCommit"].as_str().unwrap();
    let base = value["pinnedBaseRevision"].as_str().unwrap();
    assert_eq!(
        argv_of("update_ref"),
        vec!["update-ref", branch, prepared, base],
        "the recorded swap is not the compare-and-swap that ran"
    );
    assert_eq!(
        argv_of("read_tree_checkout"),
        vec!["read-tree", "-u", "--reset", prepared],
        "the recorded checkout is not the invocation that ran"
    );

    // The other twelve steps build their argv once and hand the same binding to the call and to
    // the record, so a drift is not expressible there. What IS expressible at every site is a
    // record naming one command while another ran, so each step's argv is held to the Git
    // subcommand its name denotes. This is the coarse half of the property; the full argv is
    // asserted above for the two sites where the argv was written twice.
    for step in value["gitSteps"].as_array().unwrap() {
        let name = step["step"].as_str().unwrap();
        let expected = match name {
            "diff_name_status" => "diff",
            "read_tree_checkout" => "read-tree",
            other => &other.replace('_', "-"),
        };
        assert_eq!(
            step["argv"][0].as_str().unwrap(),
            expected,
            "the {name} step records a {:?} invocation",
            step["argv"][0]
        );
    }
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

/// R46: a pre-image the product could NOT READ refuses as `PreImageUnreadable`, naming the read.
///
/// The constructed fault is the one subcase reachable end to end: `.git/config` is made unreadable,
/// so `pinned_filter_drivers` cannot establish which filter drivers the hardening must suppress and
/// the byte read never runs. Nothing was compared, so the refusal must not claim a comparison --
/// before R46 this same store refused `PreImageMismatch`, which is the Receipt blue reproduced with
/// `observed=READ_NONE`.
///
/// The other four subcases are driven at their own boundary in
/// `worktree::r46_read_classification`: a child that cannot start, one killed by a signal, one that
/// outlives its budget and a pin broken after the read cannot be constructed from here without
/// removing `git` from the machine or racing a rename.
#[test]
fn a_pre_image_the_product_could_not_read_is_refused_as_unreadable_not_as_a_mismatch() {
    let fixture = admitted_mission("pre-image-unreadable");
    let repository = CopyRepository::create(&fixture.copies_root, "pre-image-unreadable");
    let (proposal_digest, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Modify the library root, with the filter inventory made unreadable.",
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

    // THE CONSTRUCTED FAULT, at the read it is about: `.git` is replaced with a fresh directory of
    // the same name, so the repository the pre-image gate is about to read is no longer the one
    // this attempt pinned. The subcase asserted below is the one the product actually reaches --
    // MEASURED, not predicted: the filter-driver inventory is the first read to touch the
    // replacement, and it fails there before `cat-file` is ever spawned.
    //
    // Breaking the repository from OUTSIDE the dispatch does not reach this gate: an unreadable
    // `.git/config` refuses as `SourceNotBeaconMade` several gates earlier, which is why the fault
    // is installed at the read through a failpoint rather than before the call.
    let git_dir = repository.path.join(".git");
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_pre_image_read: Some(Arc::new(move || {
                let moved = git_dir.with_extension("moved");
                if git_dir.exists() {
                    fs::rename(&git_dir, &moved).unwrap();
                    fs::create_dir(&git_dir).unwrap();
                }
            })),
            ..DispatchFailpoints::default()
        })
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    // THE NAMED ASSERTION, and R46's property is the one that has not moved: NOT a mismatch. No
    // bytes were read, so nothing was compared, and this is the Receipt blue reproduced with
    // `observed=READ_NONE`.
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::PreImageMismatch,
        "a read that failed was reported as a comparison that differed"
    );
    // R50 moved the NAME of this refusal one gate earlier, and the failpoint measured why: the
    // replacement `.git` breaks the pin, the filter-driver INVENTORY is the first read to touch
    // it, and it fails there before `cat-file` is ever spawned. R47 could only say
    // `PreImageUnreadable` with an inventory subcase -- a refusal naming the pre-image read when
    // the read that failed was a precondition of every gate. `PreImageUnreadable` keeps its own
    // producer and its own test: see
    // `a_declared_create_against_a_broken_repository_is_refused_as_unreadable_not_applied`.
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::FilterInventoryUnreadable,
        "the refusal does not name the read that actually failed"
    );
    assert_eq!(
        receipt.filter_inventory_failure,
        Some(FilterInventoryFailure::Unreadable),
        "the Receipt does not say why the inventory could not be read"
    );
    // ...and the retired field is absent, because there is only one path now.
    assert_eq!(receipt.pre_image_read_failure, None);
    // Nothing was prepared, and the repository is untouched: this is a gate refusal like any other.
    assert_eq!(receipt.prepared_commit, None);
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
}

/// R46.3: a BROKEN repository is refused as unreadable -- never as "absent", never Applied.
///
/// `cat-file` exits 128 both for "path does not exist in this tree" and for "this repository is
/// broken", so classifying on the exit code made the second satisfy "declared absent, and absent".
/// A declared CREATE is the shape that exposes it: its pre-image is `None`, so the collapse does
/// not merely mis-name a refusal, it turns a broken repository into an APPLIED change.
#[test]
fn a_declared_create_against_a_broken_repository_is_refused_as_unreadable_not_applied() {
    let fixture = admitted_mission("broken-object-store");
    let repository = CopyRepository::create(&fixture.copies_root, "broken-object-store");
    fixture.beacon_made(&repository);
    let revision = fixture.rebind_repository_at(&repository, 1);
    let created = b"a file this repository does not hold\n".to_vec();
    let mut record = fixture.proposal("Create a file, against a repository that breaks first.");
    record.mutations = vec![(MUTATION_CREATE, "src/created.rs".to_owned())];
    record.repository_revision = revision;
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                // A CREATE: nothing is declared before it, which is the shape the collapse turned
                // into an application.
                pre_image_sha256: None,
                post_image_sha256: Some(sha256(&created)),
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

    // THE CONSTRUCTED FAULT: the object store is removed at the read. Every `cat-file` then exits
    // 128 saying the repository is not there -- which is not a statement about this path.
    let objects = repository.path.join(".git").join("objects");
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_pre_image_read: Some(Arc::new(move || {
                if objects.exists() {
                    fs::remove_dir_all(&objects).unwrap();
                }
            })),
            ..DispatchFailpoints::default()
        })
        .dispatch(
            &ruling.digest().value,
            &[SuppliedContent {
                path: "src/created.rs".to_owned(),
                bytes: Some(created),
            }],
        )
        .unwrap();

    // THE NAMED ASSERTION.
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::PreImageUnreadable,
        "a broken repository was read as a path that is simply not there"
    );
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "a declared create Applied against a repository that could not be read"
    );
    // ...carrying what git said, so the evidence is not erased.
    let Some(PinnedReadFailure::ReadRefused { message }) = receipt.pre_image_read_failure.as_ref()
    else {
        panic!(
            "the Receipt does not say which read failed: {:?}",
            receipt.pre_image_read_failure
        );
    };
    assert!(
        !message.is_empty(),
        "the refusal carries no message from git"
    );
    assert_eq!(receipt.prepared_commit, None);
}

/// R47: an inventory git REFUSED refuses the dispatch, naming the inventory and carrying why.
///
/// The filter-driver inventory decides which drivers the hardening suppresses, and the comment at
/// its call site says so. Before R47 a `.git/config` git could not read produced an EMPTY
/// inventory -- `if !success { still_pinned().then(Vec::new) }` -- so the byte read then ran with
/// the hardening switched off, against a repository the product had just failed to read. That is
/// the second route to the Receipt R46 exists to prevent, and it is a hardening boundary rather
/// than only an evidence one (opus, R46 report, "A finding beyond the brief").
///
/// The fault is installed AT the read through R46's failpoint, which keeps this test about the
/// PRE-IMAGE read specifically: the provenance read is the dispatch's first pinned read, so a
/// `.git/config` broken before the call is met several gates earlier.
///
/// R52 UPDATE: that earlier gate used to refuse `SourceNotBeaconMade` -- a claim about the source
/// drawn from a read that never ran -- and R47 recorded it as the one thing it could not fix. It
/// is fixed;
/// `an_unreadable_inventory_at_the_provenance_gate_is_not_a_claim_about_the_source` is the
/// companion that holds the earlier gate to the same standard this one holds the later one to.
#[test]
fn an_inventory_git_refused_refuses_the_dispatch_and_never_runs_with_the_hardening_off() {
    let fixture = admitted_mission("inventory-refused");
    let repository = CopyRepository::create(&fixture.copies_root, "inventory-refused");
    let (proposal_digest, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Modify the library root, with the filter inventory refused at the read.",
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

    // THE CONSTRUCTED FAULT: the configuration the inventory reads is made unreadable, at the read.
    // MEASURED (tools/r47/logs/measure-config-site.log): `config --file F --includes --name-only
    // --null --list` exits 128 with `Permission denied` on stderr, while a configuration that
    // simply names no entries exits 0 in silence -- so this shape is a failure and never an
    // absence.
    let configuration = repository.path.join(".git").join("config");
    let broken = configuration.clone();
    let restore = fs::metadata(&configuration).unwrap().permissions();
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_pre_image_read: Some(Arc::new(move || {
                fs::set_permissions(&broken, fs::Permissions::from_mode(0o000)).unwrap();
            })),
            ..DispatchFailpoints::default()
        })
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();
    fs::set_permissions(&configuration, restore).unwrap();

    // THE NAMED ASSERTION. R50: the refusal is the INVENTORY's own outcome, not a pre-image one.
    // R47 got the behaviour right and the name wrong -- it refused, but as `PreImageUnreadable`,
    // which says the byte read failed when no byte read was ever attempted.
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::FilterInventoryUnreadable,
        "a dispatch proceeded past a filter inventory git refused, or misnamed the refusal"
    );
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::PreImageMismatch,
        "nothing was compared, so the refusal must not claim a comparison"
    );
    // ...carrying WHY, so the evidence is not erased. Before R47 the inventory returned an empty
    // set and the read proceeded with the hardening switched off.
    let Some(FilterInventoryFailure::Refused { message }) =
        receipt.filter_inventory_failure.as_ref()
    else {
        panic!(
            "the Receipt does not say why the inventory could not be read: {:?}",
            receipt.filter_inventory_failure
        );
    };
    assert_eq!(
        receipt.pre_image_read_failure, None,
        "the retired subcase path is still producing Receipts"
    );
    assert!(
        message.contains("Permission denied"),
        "the refusal does not carry what git said: {message}"
    );
    assert_eq!(receipt.prepared_commit, None);
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
}

/// R52: an inventory the product could not read is not evidence about the source's PROVENANCE.
///
/// The counterpart of the test above, one gate earlier and without a failpoint. `.git/config` is
/// made unreadable BEFORE the dispatch, so the very first pinned read -- the filter-driver
/// inventory serving the provenance gate -- fails. Until R52 that failure was
/// `pinned_filter_drivers(repository).ok()?`: the typed failure collapsed to `None`, the token
/// looked absent, and the Receipt said `SourceNotBeaconMade` -- "this is not a repository Beacon
/// made" -- about a repository nobody had managed to ask. git's message was discarded on the way
/// (opus, R51).
///
/// The refusal must name the READ and carry what git said, and it must NOT name the source. This
/// asserts both directions, because only asserting the new outcome would still pass if some later
/// gate started producing the old lie.
#[test]
fn an_unreadable_inventory_at_the_provenance_gate_is_not_a_claim_about_the_source() {
    let fixture = admitted_mission("provenance-inventory");
    let (repository, ruling_digest) = fixture.approved("provenance-inventory");
    let before = repository.snapshot();

    // THE CONSTRUCTED FAULT, in front of the call rather than at a failpoint: this is the state a
    // dispatch can actually START in, which is what makes the provenance gate the one that meets
    // it. Measured shape (R47): `config --file ... --list` exits 128 with `Permission denied` on
    // stderr, so it is a failure and never an absence.
    let configuration = repository.path.join(".git").join("config");
    let restore = fs::metadata(&configuration).unwrap().permissions();
    fs::set_permissions(&configuration, fs::Permissions::from_mode(0o000)).unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    fs::set_permissions(&configuration, restore).unwrap();

    // THE NAMED ASSERTION, in the direction that matters most: NOT a claim about the source.
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::SourceNotBeaconMade,
        "R52.PROVENANCE.NOT_A_CLAIM_ABOUT_THE_SOURCE -- a read that never ran decided the source \
         was not Beacon-made"
    );
    // ...and it names the read that did fail, with R50's own outcome for it. Not
    // `PreImageUnreadable`: at the provenance gate no pre-image read has been attempted, which is
    // the misnomer R50 removed and the reason these two lanes are one fix in two halves.
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::FilterInventoryUnreadable,
        "R52.PROVENANCE.NAMES_THE_READ"
    );
    assert_eq!(
        receipt.pre_image_read_failure, None,
        "R52.PROVENANCE.NAMES_THE_READ -- a gate that never read a pre-image named one"
    );
    let Some(FilterInventoryFailure::Refused { message }) =
        receipt.filter_inventory_failure.as_ref()
    else {
        panic!(
            "R52.PROVENANCE.CARRIES_THE_MESSAGE -- the Receipt does not name the inventory: {:?}",
            receipt.filter_inventory_failure
        );
    };
    // git's own words, which `.ok()?` used to throw away.
    assert!(
        message.contains("Permission denied"),
        "R52.PROVENANCE.CARRIES_THE_MESSAGE -- no message from git: {message}"
    );
    // A gate refusal, so the repository is untouched.
    assert_eq!(receipt.prepared_commit, None);
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
    assert_eq!(repository.snapshot(), before);
}

/// R47: the legitimate empty inventory is NOT a refusal -- it is exit 0 and no drivers.
///
/// Measured rather than assumed, and it is the premise this lane turned out to correct: at this
/// argv git exits 0 for a configuration that names no filter drivers (empty file, section-only,
/// comment-only, `/dev/null` -- tools/r47/logs/measure-config-site.log). Every dispatch in this
/// suite already runs against such a repository, so the bar is that they still Apply; this asserts
/// it where a reader can see it, so tightening the inventory cannot quietly refuse the normal case.
#[test]
fn a_repository_that_configures_no_filter_drivers_still_applies() {
    let fixture = admitted_mission("inventory-empty");
    let repository = CopyRepository::create(&fixture.copies_root, "inventory-empty");
    let (proposal_digest, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Modify the library root in a repository that names no filter drivers.",
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
    let configuration = fs::read_to_string(repository.path.join(".git").join("config")).unwrap();
    assert!(
        !configuration.contains("[filter"),
        "this repository configures a filter driver, so it is not the empty-inventory case"
    );

    let receipt = fixture
        .dispatches
        .dispatch(&ruling.digest().value, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "an inventory that names no drivers was refused as one that could not be read"
    );
    assert_eq!(receipt.pre_image_read_failure, None);
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
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);

    // R46.5 / grok's `R46.MISMATCH.OBSERVED_DIGEST`: a Receipt that claims a comparison carries
    // the digest it COMPARED AGAINST. Before this it carried the declared pre-image, the declared
    // post-image and the path -- everything the Candidate said, and nothing that was read.
    let record = receipt
        .per_mutation
        .iter()
        .find(|record| record.path == TARGET)
        .expect("the refused mutation is recorded");
    let Some(ObservedPreImage::Bytes { digest }) = record.observed_pre_image.as_ref() else {
        panic!(
            "a mismatch Receipt records the digest it read: {:?}",
            record.observed_pre_image
        );
    };
    assert_eq!(
        *digest,
        sha256(BASE_LIB_BYTES)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        "the observed digest is not what the base revision holds"
    );
    assert_ne!(
        Some(digest.as_str()),
        record.pre_image.as_deref(),
        "a mismatch whose observed digest equals the declared one is not a mismatch"
    );
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
    // R48 bar (d), and the other half of the preparation test: `applied` remains a SQL-valid,
    // typed token, but this record contradicts itself. Both copies are rewritten the same way, so
    // byte equality still holds and only the owner's outcome-specific invariant is left to catch
    // it.
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
fn a_precondition_already_unmet_is_refused_by_the_gates_and_names_it() {
    // The gate-side half of the readiness check, which had no test of its own: deleting the single
    // readiness check from the shared evaluator turned only the landing-window test red, so the
    // gates' copy of it was unfalsifiable. Both call sites go red now, which is the property
    // extracting one evaluator was for.
    //
    // Same arrangement as the race above, without the race: the Resident is retired BEFORE the
    // dispatch, so the precondition is already unmet when the gates read it.
    let fixture = admitted_mission("unready-at-the-gate");
    let (repository, ruling_digest) = fixture.approved("unready-at-the-gate");
    let base = repository.revision.clone();
    let decision: Uuid = fixture
        .store
        .connect()
        .query_row(
            "SELECT id FROM resident_lifecycle_decisions
             WHERE resident_id = ?1 ORDER BY decided_at_ms DESC, id DESC LIMIT 1",
            [fixture.resident_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .map(|value| Uuid::parse_str(&value).unwrap())
        .unwrap();
    ResidentLifecycleService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .retire(fixture.resident_id, Some(decision))
        .unwrap();

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::ProposalNotReady);
    assert!(
        !receipt.not_verified.is_empty() && receipt.prepared_commit.is_none(),
        "a gate refusal prepared a commit"
    );
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        base,
        "a refused dispatch moved the branch"
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
fn a_swap_this_attempt_performed_is_reported_even_when_the_branch_moves_on() {
    // The other half of sol1's residue finding. This attempt's compare-and-swap returns 0 -- the
    // ref moved, which is the irreversible step -- and a competing writer then moves the branch
    // before this attempt reads it back. The old classification called that `LandingRefused`,
    // whose published meaning is "the change was not published". It was published. A Receipt that
    // under-reports a mutation Beacon performed is the one direction a record must never take, so
    // the outcome follows the swap and `observedHead` carries what the branch actually held.
    let fixture = admitted_mission("moved-after-swap");
    let (repository, ruling_digest) = fixture.approved("moved-after-swap");
    let base = repository.revision.clone();
    let path = repository.path.clone();
    let moved_to = base.clone();
    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            after_swap: Some(Arc::new(move || {
                // Another writer, between this attempt's swap and its observation.
                CopyRepository::git(&path, &["update-ref", "refs/heads/main", &moved_to]);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "a swap this attempt performed was reported as a refusal"
    );
    let prepared = receipt.prepared_commit.clone().unwrap();
    assert_eq!(
        receipt.resulting_revision.as_deref(),
        Some(prepared.as_str()),
        "resultingRevision is what the swap installed, not what the ref names now"
    );
    assert_eq!(
        receipt.observed_head.as_deref(),
        Some(base.as_str()),
        "observedHead must record what the branch actually held, not what was installed"
    );
    // The tree is not reset to a commit the branch no longer names: that would be a second write
    // against a ref this attempt no longer owns.
    assert!(!receipt.checkout_updated);
    // And the residue claim is true here, because the branch was OBSERVED to name something else.
    assert!(
        receipt
            .not_verified
            .contains(&NotVerified::UnreferencedObjectResidue),
        "a branch observed to hold something else leaves this attempt's objects unreferenced"
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
                let prepared_tree = format!("{prepared}^{{tree}}");
                let tree = CopyRepository::git(&head, &["rev-parse", &prepared_tree]);
                let descendant = CopyRepository::git(
                    &head,
                    &["commit-tree", &tree, "-p", &prepared, "-m", "descendant"],
                );
                CopyRepository::git(&head, &["update-ref", "refs/heads/main", &descendant]);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::LandingRefused);
    assert_eq!(receipt.resulting_revision, None);
    // The branch names a descendant of the prepared commit, so its history reaches the prepared
    // object even though its tip is something else.
    let prepared = receipt.prepared_commit.clone().unwrap();
    let observed = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    assert_ne!(observed, prepared);
    CopyRepository::git(
        &repository.path,
        &["merge-base", "--is-ancestor", &prepared, &observed],
    );
    assert!(
        !receipt
            .not_verified
            .contains(&NotVerified::UnreferencedObjectResidue),
        "the branch reaches the prepared commit, so the Receipt cannot claim that no ref names its objects"
    );
    // Both layers, because they are two guards and one assertion cannot kill both mutants. sol1's
    // re-run moved this test down to the persisted token, which left `repository_effect` with no
    // witness at all -- its own mutant went from RED to surviving. The accessor reads the token, so
    // it must agree with it here.
    assert_ne!(
        receipt.repository_effect(),
        RepositoryEffect::UnreferencedObjects,
        "the accessor claims unreferenced objects the persisted record does not"
    );
    fixture
        .restart()
        .expect("a refusal that measured its prepared commit still replays");
}

#[test]
fn swapping_only_the_git_directory_cannot_redirect_a_pinned_dispatch() {
    let fixture = admitted_mission("gitdir-swap");
    let (repository, ruling_digest) = fixture.approved("gitdir-swap");
    let base = repository.revision.clone();
    let decoy = fixture.copies_root.join("gitdir-decoy");
    let cloned = Command::new("git")
        .args(["clone", "--quiet", "--no-hardlinks"])
        .arg(&repository.path)
        .arg(&decoy)
        .status()
        .unwrap();
    assert!(cloned.success());

    // The replacement's own bytes, before anything runs. This is the assertion that tells the
    // descriptor pin apart from the refusal: a child that rediscovered `.git` by name would put a
    // loose object in here, and the outcome would be the same refusal either way.
    let replacement_before = raw_repository_snapshot(&decoy.join(".git"));

    let watched = repository.path.clone();
    let decoy_for_hook = decoy.clone();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    beacond::set_pinned_spawn_hook(Some(Arc::new(move |path| {
        if path != watched || observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 1 {
            return;
        }
        fs::rename(watched.join(".git"), watched.join(".git-beacon-original")).unwrap();
        fs::rename(decoy_for_hook.join(".git"), watched.join(".git")).unwrap();
    })));

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();
    beacond::set_pinned_spawn_hook(None);
    assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);

    let original_git = repository.path.join(".git-beacon-original");
    let original_head = Command::new("git")
        .arg(format!("--git-dir={}", original_git.display()))
        .args(["rev-parse", "refs/heads/main"])
        .output()
        .unwrap();
    assert!(original_head.status.success());
    let original_head = String::from_utf8(original_head.stdout).unwrap();
    let redirected_head = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);

    fs::rename(repository.path.join(".git"), decoy.join(".git")).unwrap();
    // Measured here, while the replacement still exists and before the cleanup below removes it.
    let replacement_after = raw_repository_snapshot(&decoy.join(".git"));
    fs::rename(&original_git, repository.path.join(".git")).unwrap();
    CopyRepository::git(&repository.path, &["reset", "--hard", &base]);
    let _ = fs::remove_dir_all(&decoy);

    // sol1's arrangement, with the two assertions the fix makes separable.
    //
    // The probe asked that the pinned repository receive the landing while the replacement received
    // nothing. Two mechanisms answer this swap now, and one test asserting a conjunction could not
    // tell them apart, so each assertion below is killed by a different deletion:
    //
    //   * the replacement receiving NOTHING is the descriptor pin. Delete `GIT_DIR` from `enter`
    //     and `hash-object -w` puts a loose object in the replacement -- with the same outcome,
    //     because `still_pinned` then refuses anyway;
    //   * the outcome NOT being `Applied` is `still_pinned`'s new `.git` check. Delete it and the
    //     dispatch runs to completion in the correct repository and reports `Applied`.
    //
    // Refusing rather than proceeding is deliberate: the Receipt records `source` as a PATHNAME,
    // and after a persistent swap that pathname names a different repository. Continuing would
    // produce a truthful landing described by a lying reference.
    assert_eq!(
        replacement_after, replacement_before,
        "the replacement received a write, so a child rediscovered .git by name"
    );
    assert_ne!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "a persistent .git swap was not noticed; pinned ref={}, replacement ref={redirected_head}, base={base}",
        original_head.trim(),
    );
}

#[test]
fn exact_candidate_bytes_include_non_utf8_file_content() {
    let fixture = admitted_mission("binary-content");
    let repository = CopyRepository::create(&fixture.copies_root, "binary-content");
    fixture.beacon_made(&repository);
    let revision = fixture.rebind_repository_at(&repository, 1);
    let binary = vec![0xff, 0x00, 0x80, b'\n'];
    let mut record = fixture.proposal("Create an exact binary file.");
    record.mutations = vec![(MUTATION_CREATE, "data.bin".to_owned())];
    record.repository_revision = revision;
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: None,
                post_image_sha256: Some(sha256(&binary)),
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
                path: "data.bin".to_owned(),
                bytes: Some(binary.clone()),
            }],
        )
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "the unqualified exact-byte contract rejected non-UTF-8 content"
    );
    assert_eq!(fs::read(repository.path.join("data.bin")).unwrap(), binary);
}

#[test]
fn replacement_refs_cannot_widen_the_commit_beyond_the_declared_change() {
    let fixture = admitted_mission("replace-object");
    let repository = CopyRepository::create(&fixture.copies_root, "replace-object");
    let base = repository.revision.clone();
    fs::write(
        repository.path.join("extra.txt"),
        b"not in the pinned base\n",
    )
    .unwrap();
    CopyRepository::git(&repository.path, &["add", "extra.txt"]);
    CopyRepository::git(
        &repository.path,
        &["commit", "--quiet", "-m", "replacement tree"],
    );
    let replacement = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    CopyRepository::git(
        &repository.path,
        &["update-ref", "refs/heads/main", &base, &replacement],
    );
    CopyRepository::git(&repository.path, &["replace", &base, &replacement]);
    assert!(CopyRepository::git(&repository.path, &["status", "--porcelain"]).is_empty());

    let (proposal_digest, _) = fixture.candidate_for(
        &repository,
        "Modify only src/lib.rs from the literal pinned base.",
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

    // sol1's arrangement, with the outcome it produces once the replacement is not honoured.
    //
    // The probe committed `extra.txt`, moved the branch back to the literal base, and left the file
    // on disk. Under the replaced view HEAD contains it and the tree is clean -- which is how the
    // dispatch reached the object writes and published a commit carrying an undeclared path. Under
    // the LITERAL view the same tree has an untracked file, so G11's cleanliness gate refuses three
    // steps before anything is written. The finding is not that the outcome changed; it is that the
    // literal graph and the gate's view are now the same graph. `..._is_simply_not_honoured` below
    // holds the positive half, where the tree is clean literally and the dispatch applies narrowly.
    assert_eq!(receipt.outcome, DispatchOutcome::SourceDirty);
    assert!(
        receipt.prepared_commit.is_none(),
        "a refusal at G11 prepared a commit"
    );
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "refs/heads/main"]),
        base,
        "a refused dispatch moved the branch"
    );
}

#[test]
fn a_replacement_ref_is_simply_not_honoured_and_the_dispatch_applies_literally() {
    // The positive half of sol1's counterexample, and the one that carries its assertion.
    //
    // Same replacement, but `extra.txt` is removed from the working tree, so the tree matches the
    // LITERAL base exactly and disagrees with the replaced one. Before the fix this dispatch was
    // refused as `SourceDirty` -- the gate saw a file missing from the replaced HEAD. After it, the
    // replacement is invisible: the dispatch applies, and `base..prepared` in the literal graph is
    // exactly the declared path. Deleting `GIT_NO_REPLACE_OBJECTS` flips both tests at once and in
    // opposite directions, which is what proves the neutralizer decides *which graph is
    // authoritative* rather than merely making something stricter.
    let fixture = admitted_mission("replace-ignored");
    let repository = CopyRepository::create(&fixture.copies_root, "replace-ignored");
    let base = repository.revision.clone();
    fs::write(
        repository.path.join("extra.txt"),
        b"not in the pinned base\n",
    )
    .unwrap();
    CopyRepository::git(&repository.path, &["add", "extra.txt"]);
    CopyRepository::git(
        &repository.path,
        &["commit", "--quiet", "-m", "replacement tree"],
    );
    let replacement = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    CopyRepository::git(
        &repository.path,
        &["update-ref", "refs/heads/main", &base, &replacement],
    );
    // Index AND working tree back to the literal base -- moving the ref alone leaves `extra.txt`
    // staged, which is a dirty tree under either view and would refuse for the wrong reason.
    CopyRepository::git(&repository.path, &["reset", "--quiet", "--hard", &base]);
    CopyRepository::git(&repository.path, &["replace", &base, &replacement]);
    // Under the replaced view the tree is dirty; under the literal view it is clean. The two views
    // disagree, which is the whole point of the arrangement.
    assert!(
        !CopyRepository::git(&repository.path, &["status", "--porcelain"]).is_empty(),
        "the replaced view should see extra.txt missing"
    );

    let (proposal_digest, _) = fixture.candidate_for(
        &repository,
        "Modify only src/lib.rs from the literal pinned base.",
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
    assert_eq!(receipt.outcome, DispatchOutcome::Applied);
    let prepared = receipt.prepared_commit.unwrap();

    let literal = Command::new("git")
        .args(["diff", "--name-status", &base, &prepared])
        .current_dir(&repository.path)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
        .unwrap();
    assert!(literal.status.success());
    assert_eq!(
        String::from_utf8(literal.stdout).unwrap(),
        "M\tsrc/lib.rs\n",
        "the literal base..commit effect is wider than the Candidate declaration"
    );
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

#[test]
fn a_reconstructed_receipt_that_cannot_name_its_residue_claims_no_byte_identity() {
    // The state a boolean could not hold. This Receipt says the object residue is unknown -- the
    // interruption fell before the claim, so whether anything was written is unrecorded -- and the
    // same record reported the repository as provably byte-identical, because it had no prepared
    // commit to point at (gpt2). Unknown is its own answer.
    let fixture = admitted_mission("unknown-residue");
    let (_repository, ruling_digest) = fixture.approved("unknown-residue");
    fixture.dispatch_until_death(&ruling_digest, "before_reserve");
    fixture.restart().unwrap();

    let payload: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    let receipt: beacond::DispatchReceipt = serde_json::from_str(&payload).unwrap();
    assert_eq!(receipt.prepared_commit, None);
    assert!(
        receipt
            .not_verified
            .contains(&NotVerified::ObjectResidueUnknown)
    );
    assert_eq!(receipt.repository_effect(), RepositoryEffect::Unknown);
}

#[test]
fn a_landing_nobody_observed_claims_no_unreferenced_objects() {
    // The other half of the defect the four-arm `repository_effect` was introduced to fix (gpt2).
    // `ObjectResidueUnknown` was mapped to `Unknown`; every other non-Applied Receipt carrying a
    // prepared commit fell through to `UnreferencedObjects`, whose contract asserts that objects
    // exist which no ref names.
    //
    // A reconstructed `LandingUnobserved` carries `LandingResultUnknown` for exactly the reason
    // that assertion cannot be made: the repository could not say what became of the commit. Here
    // the branch holds a DESCENDANT of the prepared commit, so the commit is reachable and named by
    // history -- `UnreferencedObjects` is not merely unproven, it is false.
    let fixture = admitted_mission("landing-unobserved");
    let (repository, ruling_digest) = fixture.approved("landing-unobserved");
    fixture.dispatch_until_death(&ruling_digest, "after_swap");

    let (prepared, _branch) = fixture
        .preparation()
        .expect("the claim did not survive the crash");
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        prepared,
        "the swap did not happen, so this test proves nothing"
    );

    // Later history descending from the prepared commit: the branch no longer equals it and no
    // longer equals the pinned base, which is what startup reads as "unobserved".
    fs::write(repository.path.join(TARGET), OTHER_BYTES).unwrap();
    CopyRepository::git(&repository.path, &["add", TARGET]);
    CopyRepository::git(&repository.path, &["commit", "-m", "a later commit"]);
    let head = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    assert_ne!(head, prepared);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", &format!("{head}~1")]),
        prepared,
        "the later commit must descend from the prepared one"
    );

    fixture.restart().unwrap();

    let payload: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    let receipt: beacond::DispatchReceipt = serde_json::from_str(&payload).unwrap();
    assert_eq!(receipt.outcome, DispatchOutcome::LandingUnobserved);
    assert_eq!(receipt.prepared_commit.as_deref(), Some(prepared.as_str()));
    assert!(
        receipt
            .not_verified
            .contains(&NotVerified::LandingResultUnknown)
    );
    assert_eq!(
        receipt.repository_effect(),
        RepositoryEffect::Unknown,
        "a landing nobody observed cannot assert that objects exist which no ref names"
    );
}

/// The eleven tokens every Receipt carries, in the order ADR 0039 fixes.
const ALWAYS_NOT_VERIFIED: &[&str] = &[
    "execution_policy_constraint",
    "sandbox_constraint",
    "network_constraint",
    "model_provider_constraint",
    "command_allowlist_constraint",
    "tool_allowlist_constraint",
    "evaluations_constraint",
    "non_author_evaluation_of_candidate",
    "safety_snapshot",
    "base_revision_reachability",
    "declared_checks_against_result",
];

#[test]
fn a_live_unobserved_landing_persists_no_residue_claim_either() {
    // The same exclusion on the LIVE path. gpt2 named both sites; the reconstructed one is driven
    // by the test below, and without this one deleting the live exclusion leaves the suite green.
    //
    // A live `LandingUnobserved` is what a dispatch writes when the swap happened and the
    // observation afterwards could not answer. The repository is renamed out from under the
    // attempt between the two, so `read_pinned` finds the pin broken and returns nothing -- the
    // attempt holds a prepared commit and cannot say whether a ref names it, which is precisely
    // when the affirmative residue token must not be written.
    let fixture = admitted_mission("live-unobserved");
    let (repository, ruling_digest) = fixture.approved("live-unobserved");
    let moved = repository.path.with_file_name("moved-mid-dispatch");
    let source = repository.path.clone();
    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            after_swap: Some(Arc::new(move || {
                // After the ref moved, before the observation reads it back.
                let _ = fs::rename(&source, &moved);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    assert_eq!(receipt.outcome, DispatchOutcome::LandingUnobserved);
    assert!(
        receipt.prepared_commit.is_some(),
        "without a prepared commit the residue token would not be pushed anyway"
    );

    let payload: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let tokens: Vec<&str> = value["notVerified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap())
        .collect();
    assert!(
        tokens.contains(&"landing_result_unknown"),
        "the unknown is what this state has to say: {payload}"
    );
    assert!(
        !tokens.contains(&"unreferenced_object_residue"),
        "a live unobserved landing must not claim objects no ref names: {payload}"
    );
    assert_eq!(receipt.repository_effect(), RepositoryEffect::Unknown);
}

#[test]
fn the_persisted_receipt_of_an_unobserved_landing_makes_no_residue_claim() {
    // gpt2, s4f P1. `repository_effect()` normalizes this state to `Unknown`, but the RECORD still
    // serialized `unreferenced_object_residue`, whose declared meaning is "Objects were written that
    // no ref names". In this exact state that token is not merely unproven, it is false: the branch
    // holds a descendant of the prepared commit, so a ref names it.
    //
    // A record must not say what the API refuses to say. This test reads the persisted bytes rather
    // than calling `repository_effect()`, because the normalization is precisely what would hide the
    // defect: the API answered `Unknown` while the stored vocabulary answered otherwise, and only
    // the stored bytes are what another implementation, an export, or a later reader sees.
    let fixture = admitted_mission("persisted-unobserved");
    let (repository, ruling_digest) = fixture.approved("persisted-unobserved");
    fixture.dispatch_until_death(&ruling_digest, "after_swap");
    let (prepared, _branch) = fixture
        .preparation()
        .expect("the claim did not survive the crash");

    fs::write(repository.path.join(TARGET), OTHER_BYTES).unwrap();
    CopyRepository::git(&repository.path, &["add", TARGET]);
    CopyRepository::git(&repository.path, &["commit", "-m", "a later commit"]);
    let head = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", &format!("{head}~1")]),
        prepared,
        "the later commit must descend from the prepared one, or the residue token is not false"
    );

    fixture.restart().unwrap();

    let payload: String = fixture
        .store
        .connect()
        .query_row("SELECT receipt_json FROM receipts", [], |row| row.get(0))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(value["outcome"], "landing_unobserved");
    assert_eq!(value["preparedCommit"], prepared);

    let tokens: Vec<&str> = value["notVerified"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry.as_str().unwrap())
        .collect();
    assert!(
        !tokens.contains(&"unreferenced_object_residue"),
        "the record claims objects no ref names, while a ref names them: {payload}"
    );
    // The whole vocabulary, in order: the unknown is what is persisted, and a future token that
    // contradicted it would have to pass through here.
    let mut expected: Vec<&str> = ALWAYS_NOT_VERIFIED.to_vec();
    expected.push("landing_result_unknown");
    expected.push("reconstructed_after_interruption");
    assert_eq!(tokens, expected, "{payload}");
}

/// R46.8, the observed-evidence contract as a table. One probe per cell, each a real dispatch
/// through the public services followed by a restart, because the failure this lane kept finding
/// was a Receipt the store would not read back.
///
/// | declared | observed  | this file's probe                                          |
/// |----------|-----------|------------------------------------------------------------|
/// | present  | `Bytes` ≠ | `a_mismatch_cell_declared_present_bytes_differ_reopens`     |
/// | present  | `Absent`  | `astra_a_truthful_absent_pre_image_mismatch_remains_readable_after_restart` |
/// | absent   | `Bytes`   | `astra_a_create_conflict_remains_readable_after_restart`    |
/// | absent   | `Absent`  | `a_holding_cell_declared_absent_and_absent_applies_and_reopens` |
/// | present  | `Bytes` = | `a_holding_cell_declared_present_bytes_equal_applies_and_reopens` |
#[test]
fn a_mismatch_cell_declared_present_bytes_differ_reopens() {
    let fixture = admitted_mission("cell-present-differs");
    let repository = CopyRepository::create(&fixture.copies_root, "cell-present-differs");
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
    assert_eq!(receipt.version, 4);
    let record = receipt
        .per_mutation
        .iter()
        .find(|record| record.path == TARGET)
        .expect("the refused mutation is recorded");
    let Some(ObservedPreImage::Bytes { digest }) = record.observed_pre_image.as_ref() else {
        panic!("R46.CELL.PRESENT_DIFFERS_CARRIES_BYTES: {record:?}");
    };
    assert_ne!(Some(digest.as_str()), record.pre_image.as_deref());
    fixture.restart().expect("R46.CELL.PRESENT_DIFFERS_REOPENS");
}

/// The `(absent, Absent)` cell: a create whose path really is not there HOLDS, and applies.
#[test]
fn a_holding_cell_declared_absent_and_absent_applies_and_reopens() {
    let fixture = admitted_mission("cell-absent-absent");
    let repository = CopyRepository::create(&fixture.copies_root, "cell-absent-absent");
    let created = "src/created.rs";
    let mut record = fixture.proposal("Create a file the base revision does not hold.");
    record.mutations = vec![(MUTATION_CREATE, created.to_owned())];
    record.repository_revision = fixture.rebind_repository(&repository);
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: None,
                post_image_sha256: Some(sha256(NEW_BYTES)),
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
                path: created.to_owned(),
                bytes: Some(NEW_BYTES.to_vec()),
            }],
        )
        .unwrap();

    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "R46.CELL.ABSENT_AND_ABSENT_HOLDS"
    );
    // A cell that HOLDS records the observation too -- the evidence is not only kept for refusals.
    assert!(
        receipt
            .per_mutation
            .iter()
            .all(|record| matches!(record.observed_pre_image, Some(ObservedPreImage::Absent))),
        "R46.CELL.ABSENT_AND_ABSENT_CARRIES_ABSENT: {:?}",
        receipt.per_mutation
    );
    fixture
        .restart()
        .expect("R46.CELL.ABSENT_AND_ABSENT_REOPENS");
}

/// The `(present, Bytes =)` cell: a modify whose declared pre-image is what the tree holds applies.
#[test]
fn a_holding_cell_declared_present_bytes_equal_applies_and_reopens() {
    let fixture = admitted_mission("cell-present-equal");
    let repository = CopyRepository::create(&fixture.copies_root, "cell-present-equal");
    let (proposal_digest, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Modify the library root from the bytes it actually holds.",
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
        "R46.CELL.PRESENT_AND_EQUAL_HOLDS"
    );
    let record = receipt
        .per_mutation
        .iter()
        .find(|record| record.path == TARGET)
        .expect("the applied mutation is recorded");
    let Some(ObservedPreImage::Bytes { digest }) = record.observed_pre_image.as_ref() else {
        panic!("R46.CELL.PRESENT_AND_EQUAL_CARRIES_BYTES: {record:?}");
    };
    assert_eq!(
        Some(digest.as_str()),
        record.pre_image.as_deref(),
        "the cell that holds is the one whose observation EQUALS the declaration"
    );
    fixture
        .restart()
        .expect("R46.CELL.PRESENT_AND_EQUAL_REOPENS");
}

/// A second post-image, so two Rulings on one base prepare two different commits.
const OTHER_BYTES: &[u8] = b"fn main() { /* the other dispatch */ }\n";

#[test]
fn two_dispatch_commands_racing_one_branch_are_separated_by_the_compare_and_swap() {
    // R9's property, with two Dispatch Commands rather than a Command and a raw writer.
    //
    // Two dispatchers of the SAME Ruling cannot exercise it: the one-use claim refuses the loser
    // before it reaches the swap, which is why removing the expected-old value leaves that test
    // green (gpt1). So these are two Rulings -- two Proposals, two Candidates, one repository
    // revision, one base -- and neither claim blocks the other. Both reach the landing transaction
    // together, and the compare-and-swap is the only thing that can separate them.
    let fixture = admitted_mission("two-commands");
    let repository = CopyRepository::create(&fixture.copies_root, "two-commands");
    fixture.beacon_made(&repository);
    let revision = fixture.rebind_repository(&repository);
    let base = repository.revision.clone();

    let ruled = |summary: &str, post: &[u8]| {
        let mut record = fixture.proposal(summary);
        record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
        record.repository_revision = revision;
        record.base_revision = base.clone();
        record.context_digest = sha256(BASE_LIB_BYTES);
        let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
        fixture
            .candidates
            .record(&proposal.digest().value, &images_for(BASE_LIB_BYTES, post))
            .unwrap();
        fixture
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
            .unwrap()
            .digest()
            .value
            .clone()
    };
    let first = ruled(
        "Apply the dispatched change to the library root.",
        NEW_BYTES,
    );
    let second = ruled(
        "Apply the other dispatched change to the root.",
        OTHER_BYTES,
    );

    // The barrier sits inside the act, after each attempt has claimed its Ruling and before either
    // opens its landing transaction: both have observed the same base and both are about to swap.
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let outcomes: Vec<DispatchOutcome> = std::thread::scope(|scope| {
        let handles: Vec<_> = [(first.clone(), NEW_BYTES), (second.clone(), OTHER_BYTES)]
            .into_iter()
            .map(|(ruling, bytes)| {
                let barrier = Arc::clone(&barrier);
                let dispatches =
                    DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
                        .with_failpoints(DispatchFailpoints {
                            before_landing: Some(Arc::new(move || {
                                barrier.wait();
                            })),
                            ..DispatchFailpoints::default()
                        });
                scope.spawn(move || dispatches.dispatch(&ruling, &content_for(bytes)))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap()
                    .expect("a racing dispatcher failed instead of refusing")
                    .outcome
            })
            .collect()
    });

    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == DispatchOutcome::Applied)
            .count(),
        1,
        "two Dispatch Commands raced one branch and produced {outcomes:?}"
    );
    assert!(
        outcomes.contains(&DispatchOutcome::LandingRefused),
        "the loser was not refused by the swap: {outcomes:?}"
    );

    // One commit, not two, and the branch is exactly one ahead of the base.
    let head = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
    assert_eq!(
        CopyRepository::git(
            &repository.path,
            &["rev-list", "--count", &format!("{base}..{head}")]
        ),
        "1"
    );
}

#[test]
fn a_decoy_installed_between_the_head_check_and_the_landing_receives_nothing() {
    // The kernel row as it was written: a decoy at the pinned pathname AFTER the gates measured the
    // real repository and BEFORE the landing. The committed test swapped before `dispatch` was
    // called, which measures the head gate rather than the landing -- my C32 report said otherwise
    // and was wrong (gpt2).
    //
    // Entering by descriptor is what makes this safe: the pathname now resolves to the decoy, and
    // nothing this attempt does can reach it.
    let fixture = admitted_mission("decoy-mid-attempt");
    let (repository, ruling_digest) = fixture.approved("decoy-mid-attempt");
    let original = repository.path.clone();
    let moved = fixture.copies_root.join("copy-decoy-mid-attempt-moved");
    let copies_root = fixture.copies_root.clone();
    let base = repository.revision.clone();
    let decoy_snapshot = Arc::new(Mutex::new(String::new()));
    let recorded = Arc::clone(&decoy_snapshot);
    let moved_for_hook = moved.clone();

    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                fs::rename(&original, &moved_for_hook).unwrap();
                let decoy = CopyRepository::create(&copies_root, "decoy-mid-attempt-decoy");
                fs::write(decoy.path.join("decoy.txt"), "decoy\n").unwrap();
                CopyRepository::git(&decoy.path, &["add", "-A"]);
                CopyRepository::git(&decoy.path, &["commit", "--quiet", "-m", "decoy"]);
                fs::rename(&decoy.path, &original).unwrap();
                *recorded.lock().unwrap() = raw_repository_snapshot(&original);
            })),
            ..DispatchFailpoints::default()
        });

    let receipt = dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .unwrap();

    // Whatever this attempt concluded, the decoy standing at the pinned pathname received nothing.
    assert_eq!(
        raw_repository_snapshot(&repository.path),
        decoy_snapshot.lock().unwrap().clone(),
        "the decoy at the pinned pathname was written to"
    );
    // And the repository that was measured did not move either: a swap it could not verify is not
    // a swap it performs.
    assert_ne!(receipt.outcome, DispatchOutcome::Applied);
    assert_eq!(CopyRepository::git(&moved, &["rev-parse", "HEAD"]), base);
}

#[test]
fn a_target_that_escapes_the_repository_cannot_reach_a_dispatch_at_all() {
    // "A target that escapes refuses before anything is published" is true one step earlier than it
    // sounds: an escaping path is unrepresentable in a Proposal, so no Candidate can carry it, no
    // Ruling can name it, and no dispatch exists to refuse. Asserted here, in the dispatch tests,
    // because that is where the claim is made.
    let fixture = admitted_mission("escape");
    for escape in ["../outside.rs", "/etc/passwd", "src/../../outside.rs"] {
        let mut record = fixture.proposal("Write outside the repository.");
        record.mutations = vec![(MUTATION_MODIFY, escape.to_owned())];
        assert!(
            fixture.proposals.submit(&record.bytes()).is_err(),
            "a Proposal declaring {escape} was accepted"
        );
    }
}

#[test]
fn a_decoy_swapped_in_for_the_duration_of_one_invocation_receives_nothing() {
    // The window `still_pinned` cannot see, and the only arrangement that tells descriptor entry
    // apart from pathname entry: the directory is replaced AFTER the pin is checked and BEFORE the
    // child runs, and put back before the check that follows. A command entering by name would
    // write into the decoy and the post-check would still pass, which is exactly the probe that
    // failed this code once already.
    //
    // I found this one myself, after the mid-attempt decoy test above turned out to be caught by
    // `still_pinned` rather than by descriptor entry -- `current_dir(path)` survived every test in
    // the repository. It does not survive this one.
    let fixture = admitted_mission("swap-during");
    let (repository, ruling_digest) = fixture.approved("swap-during");
    let original = repository.path.clone();
    let moved = fixture.copies_root.join("copy-swap-during-moved");
    let decoy = CopyRepository::create(&fixture.copies_root, "swap-during-decoy");
    let decoy_home = decoy.path.clone();
    let decoy_before = raw_repository_snapshot(&decoy_home);
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let watched = original.clone();
    let moved_for_hook = moved.clone();
    let decoy_for_hook = decoy_home.clone();
    let counted = Arc::clone(&writes);
    beacond::set_pinned_spawn_hook(Some(Arc::new(move |path| {
        // Only this test's repository, and only its SECOND write: `hash-object -w`, which puts a
        // loose object into whichever repository it runs in. The first write is `read-tree`, which
        // would merely fail in the decoy and prove nothing about where it ran.
        if path != watched || counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) != 1 {
            return;
        }
        fs::rename(&watched, &moved_for_hook).unwrap();
        fs::rename(&decoy_for_hook, &watched).unwrap();
    })));

    let receipt = fixture
        .dispatches
        .dispatch(&ruling_digest, &content_for(NEW_BYTES));
    beacond::set_pinned_spawn_hook(None);
    let receipt = receipt.unwrap();

    // Put the decoy back where the test can measure it, and the real repository back in place.
    fs::rename(&original, &decoy_home).unwrap();
    fs::rename(&moved, &original).unwrap();

    assert!(
        writes.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "the invocation this test swaps around never ran, so it proves nothing"
    );
    assert_eq!(
        raw_repository_snapshot(&decoy_home),
        decoy_before,
        "the decoy received a write from a command that should have entered by descriptor"
    );
    assert_ne!(receipt.outcome, DispatchOutcome::Applied);
}

#[test]
fn astra_a_create_conflict_remains_readable_after_restart() {
    // Ordinary public commands: a create declaration conflicts with an existing base file.
    // No store writes, repository damage, failpoints, or production-code changes.
    let fixture = admitted_mission("create-conflict");
    let repository = CopyRepository::create(&fixture.copies_root, "create-conflict");
    let mut record = fixture.proposal("Create a file whose path is already present.");
    record.mutations = vec![(MUTATION_CREATE, TARGET.to_owned())];
    record.repository_revision = fixture.rebind_repository(&repository);
    record.base_revision = repository.revision.clone();
    record.context_digest = sha256(BASE_LIB_BYTES);
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    fixture
        .candidates
        .record(
            &proposal.digest().value,
            &[CandidateImages {
                pre_image_sha256: None,
                post_image_sha256: Some(sha256(NEW_BYTES)),
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
                path: TARGET.to_owned(),
                bytes: Some(NEW_BYTES.to_vec()),
            }],
        )
        .unwrap();
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::PreImageMismatch,
        "R46.MISMATCH.CREATE_CONFLICT_IS_NAMED"
    );
    assert_eq!(
        receipt.version, 4,
        "R46.MISMATCH.CREATE_CONFLICT_IS_VERSION_4"
    );
    assert_eq!(receipt.prepared_commit, None);
    assert_eq!(receipt.resulting_revision, None);
    assert_eq!(receipt.repository_effect(), RepositoryEffect::ByteIdentical);
    assert_eq!(
        CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]),
        head_before
    );
    eprintln!(
        "CREATE_CONFLICT_OBSERVATION={:?}",
        receipt.per_mutation[0].observed_pre_image
    );
    fixture
        .restart()
        .expect("R46.MISMATCH.OBSERVED_PRESENT_REOPENS");
}

// R54.1/R54.2: Astra’s committed cold-read regressions and their byte/era controls.
fn astra_r55_mismatch(label: &str) -> (Fixture, beacond::DispatchReceipt) {
    let fixture = admitted_mission(label);
    let repository = CopyRepository::create(&fixture.copies_root, label);
    let (proposal, _) = fixture.candidate_for_with_pre_image(
        &repository,
        "Measure the pre-image.",
        b"different declared bytes\n",
    );
    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal).unwrap(),
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
        DispatchOutcome::PreImageMismatch,
        "ASTRA.R54.FIXTURE_MISMATCH"
    );
    fixture.restart().expect("ASTRA.R54.FIXTURE_REOPENS");
    (fixture, receipt)
}

fn astra_r55_replace(fixture: &Fixture, value: &serde_json::Value, column_version: i64) {
    let encoded = serde_json::to_string(value).unwrap().replace('\'', "''");
    fixture.without_run_triggers(&format!(
        "PRAGMA ignore_check_constraints=ON; UPDATE receipts SET version={column_version}, receipt_json='{encoded}'; UPDATE events SET payload_json='{encoded}' WHERE kind='proposal.dispatched'; PRAGMA ignore_check_constraints=OFF;"
    ));
}

#[test]
fn astra_r54_v3_null_observation_is_presence_not_legacy_absence() {
    let (fixture, receipt) = astra_r55_mismatch("astra-era-null");
    let mut legacy = serde_json::to_value(&receipt).unwrap();
    legacy["version"] = 3.into();
    legacy["perMutation"][0]
        .as_object_mut()
        .unwrap()
        .remove("observedPreImage");
    astra_r55_replace(&fixture, &legacy, 3);
    fixture
        .restart()
        .expect("ASTRA.R54.ABSENT_KEY_IS_LEGACY_CONTROL");
    legacy["perMutation"][0]["observedPreImage"] = serde_json::Value::Null;
    assert!(
        legacy["perMutation"][0]
            .as_object()
            .unwrap()
            .contains_key("observedPreImage")
    );
    astra_r55_replace(&fixture, &legacy, 3);
    let result = fixture.restart();
    assert!(
        matches!(result, Err(SupervisorError::DispatchEventMismatch(_))),
        "ASTRA.R54.V3_NULL_IS_PRESENT: key present with null reopened as {result:?}"
    );
}

#[test]
fn astra_r54_v4_observed_digest_is_not_an_empty_string() {
    let (fixture, receipt) = astra_r55_mismatch("astra-empty-digest");
    let mut value = serde_json::to_value(&receipt).unwrap();
    value["perMutation"][0]["observedPreImage"] = serde_json::json!({"bytes":{"digest":""}});
    astra_r55_replace(&fixture, &value, 4);
    let result = fixture.restart();
    assert!(
        matches!(result, Err(SupervisorError::DispatchEventMismatch(_))),
        "ASTRA.R54.EMPTY_OBSERVED_DIGEST: an empty string was accepted as a measured digest: {result:?}"
    );
}

#[test]
fn astra_r54_binary_empty_and_absent_preimages_keep_their_era() {
    for (label, bytes) in [
        ("astra-binary", Some(&b"\xff\x00\x80"[..])),
        ("astra-empty", Some(&b""[..])),
        ("astra-absent", None),
    ] {
        let fixture = admitted_mission(label);
        let mut repository = CopyRepository::create(&fixture.copies_root, label);
        if let Some(bytes) = bytes {
            fs::write(repository.path.join(TARGET), bytes).unwrap();
        } else {
            fs::remove_file(repository.path.join(TARGET)).unwrap();
        }
        CopyRepository::git(&repository.path, &["add", "-A"]);
        CopyRepository::git(&repository.path, &["commit", "--quiet", "-m", "held bytes"]);
        repository.revision = CopyRepository::git(&repository.path, &["rev-parse", "HEAD"]);
        fixture.beacon_made(&repository);
        let revision = fixture.rebind_repository_at(&repository, 1);
        let mut record = fixture.proposal("Compare the bytes held.");
        record.repository_revision = revision;
        record.base_revision = repository.revision.clone();
        // The proposal's context is a separate, unchanged file: reach the pre-image gate.
        record.context_path = "src/old.rs".to_owned();
        record.context_digest = sha256(b"fn old() {}\n");
        record.mutations = vec![(MUTATION_MODIFY, TARGET.to_owned())];
        let proposal = fixture
            .proposals
            .submit(&record.bytes())
            .unwrap()
            .digest()
            .value
            .clone();
        fixture
            .candidates
            .record(
                &proposal,
                &images_for(b"different declared bytes\n", NEW_BYTES),
            )
            .unwrap();
        let ruling = fixture
            .rulings
            .rule_on_candidate(
                &fixture.candidates.present(&proposal).unwrap(),
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
            DispatchOutcome::PreImageMismatch,
            "ASTRA.R54.BYTES_OUTCOME {label}"
        );
        let expected = bytes
            .map(|b| ObservedPreImage::Bytes {
                digest: HEXLOWER.encode(&sha256(b)),
            })
            .unwrap_or(ObservedPreImage::Absent);
        assert_eq!(
            receipt.per_mutation[0].observed_pre_image,
            Some(expected),
            "ASTRA.R54.BYTES_ARE_NOT_ABSENCE {label}"
        );
        fixture.restart().expect("ASTRA.R54.BYTES_V4_RESTART");
        let mut value = serde_json::to_value(&receipt).unwrap();
        value["version"] = 3.into();
        astra_r55_replace(&fixture, &value, 3);
        assert!(
            matches!(
                fixture.restart(),
                Err(SupervisorError::DispatchEventMismatch(_))
            ),
            "ASTRA.R54.BYTES_V3_PRESENT_REFUSED {label}"
        );
        value["perMutation"][0]
            .as_object_mut()
            .unwrap()
            .remove("observedPreImage");
        astra_r55_replace(&fixture, &value, 3);
        fixture.restart().expect("ASTRA.R54.BYTES_V3_UNRECORDED");
    }
}

/// The table holds the outcome fixed as a mismatch, with an in-domain observed
/// digest different from the declaration. Domain membership alone is not a verdict.
fn astra_observation_cell(
    version: u32,
    observation: Option<serde_json::Value>,
    accepted: bool,
    name: &str,
) {
    let (fixture, receipt) = astra_r55_mismatch("era-cell");
    let mut wire = serde_json::to_value(&receipt).unwrap();
    wire["version"] = version.into();
    let mutation = wire["perMutation"][0].as_object_mut().unwrap();
    mutation.remove("observedPreImage");
    if let Some(observation) = observation {
        mutation.insert("observedPreImage".into(), observation);
    }
    astra_r55_replace(&fixture, &wire, i64::from(version));
    let result = fixture.restart();
    if accepted {
        assert!(result.is_ok(), "{name}: {result:?}");
    } else {
        assert!(
            matches!(result, Err(SupervisorError::DispatchEventMismatch(_))),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn r541_v3_absent_key_is_unrecorded() {
    astra_observation_cell(3, None, true, "R54.CELL.V3.ABSENT_IS_UNRECORDED");
}

#[test]
fn r541_v3_null_key_is_refused() {
    astra_observation_cell(
        3,
        Some(serde_json::Value::Null),
        false,
        "R54.CELL.V3.NULL_IS_PRESENT",
    );
}

#[test]
fn r541_v3_in_domain_value_is_refused() {
    astra_observation_cell(
        3,
        Some(
            serde_json::json!({"bytes":{"digest":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}),
        ),
        false,
        "R54.CELL.V3.VALUE_IS_PRESENT",
    );
}

#[test]
fn r541_v3_out_of_domain_value_is_refused() {
    astra_observation_cell(
        3,
        Some(serde_json::json!({"bytes":{"digest":""}})),
        false,
        "R54.CELL.V3.BAD_VALUE_IS_PRESENT",
    );
}

#[test]
fn r542_v4_absent_key_is_refused() {
    astra_observation_cell(4, None, false, "R54.CELL.V4.ABSENT_IS_UNRECORDED");
}

#[test]
fn r542_v4_null_key_is_refused() {
    astra_observation_cell(
        4,
        Some(serde_json::Value::Null),
        false,
        "R54.CELL.V4.NULL_IS_NOT_AN_OBSERVATION",
    );
}

#[test]
fn r542_v4_sha256_of_empty_is_recorded_evidence() {
    astra_observation_cell(
        4,
        Some(
            serde_json::json!({"bytes":{"digest":"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}}),
        ),
        true,
        "R54.CELL.V4.EMPTY_BLOB_DIGEST_IS_EVIDENCE",
    );
}

#[test]
fn r542_v4_out_of_domain_value_is_refused() {
    astra_observation_cell(
        4,
        Some(serde_json::json!({"bytes":{"digest":""}})),
        false,
        "R54.CELL.V4.BAD_DIGEST_IS_NOT_EVIDENCE",
    );
}

// ---------------------------------------------------------------------------------------------
// B7 — the phase boundary a human session lock lands on.
//
// FOUR named cells across the two tests below. They measure only what the existing public services
// already do; they add no authority, change no production code, and claim nothing about Agent
// continuity or about the durability of an interrupted dispatch's Receipt. The canonical reading is
// the measured addendum in beacon-team/DP-B7-lock-continuity-opus.md; the older whole-dispatch-lock
// and bootstrap-digest expectations it supersedes were falsified and are not reproduced here.
// ---------------------------------------------------------------------------------------------

/// B7.CONTROL.APPLIED — the ordinary unlocked dispatch really is `Applied`.
///
/// Without it the locked bars below could pass because the dispatch never worked at all.
#[test]
fn b7_control_an_unlocked_session_completes_the_dispatch() {
    let fixture = admitted_mission("b7-unlocked-control");
    let (_repository, ruling_digest) = fixture.approved("b7-unlocked-control");
    let receipt = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .dispatch(&ruling_digest, &content_for(NEW_BYTES))
        .expect("B7.CONTROL.APPLIED: an unlocked session completes the dispatch");
    assert_eq!(
        receipt.outcome,
        DispatchOutcome::Applied,
        "B7.CONTROL.APPLIED"
    );
}

/// Three cells on the locked path, sharing one expensive fixture rather than duplicating it:
///
/// * **B7.PHASE.LOCK_COMPLETED_THEN_REFUSED** — a lock that COMPLETES at `before_landing` is
///   observed by the next phase, which refuses with exactly `DispatchError::Account(NotAuthenticated)`.
/// * **B7.LOCK.STATUS_UNAVAILABLE** — the public status then reports `workspace_access == false`
///   AND `operator_commands == false`, both asserted, because `status_for` derives them from one
///   boolean and a single-field check would not notice if that ever changed.
/// * **B7.LOCK.NEW_COMMAND_REFUSED** — a subsequent dispatch through the public service is that
///   same exact error, not a catch-all.
///
/// `before_landing` fires after `reserve` returned and before the landing closure opens
/// (`dispatch.rs:1220`), which is where the `AccountState` guard is free and a lock can complete.
/// The rendezvous is two-phase and carries the locker's RESULT, so the hook cannot return until the
/// lock has actually returned, and a locker that failed releases the hook with its error instead of
/// parking it. `recv_timeout` is a teardown guard against a hung run: it is never an asserted
/// outcome, and nothing here sleeps for a desired result or asserts elapsed time.
#[test]
fn b7_a_lock_completed_between_dispatch_phases_refuses_the_next_phase_and_new_commands() {
    use std::sync::Mutex;
    use std::sync::mpsc::{RecvTimeoutError, channel};
    use std::time::Duration;

    let fixture = admitted_mission("b7-lock-mid-dispatch");
    let (_repository, ruling_digest) = fixture.approved("b7-lock-mid-dispatch");

    let (arrived_tx, arrived_rx) = channel::<()>();
    let (locked_tx, locked_rx) = channel::<Result<OperatorSessionPhase, String>>();
    let locked_rx = Arc::new(Mutex::new(locked_rx));

    // ONE shared account service: the session lives in that instance, so locking a second one would
    // lock a different session and prove nothing.
    let locker_accounts = Arc::clone(&fixture.accounts);
    let locker = thread::spawn(move || {
        arrived_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("B7.HARNESS: the hook never signalled arrival");
        let outcome = locker_accounts
            .lock()
            .map(|status| status.phase)
            .map_err(|error| format!("{error:?}"));
        locked_tx
            .send(outcome.clone())
            .expect("B7.HARNESS: the hook stopped listening");
        outcome
    });

    let hook_locked_rx = Arc::clone(&locked_rx);
    let dispatches = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .with_failpoints(DispatchFailpoints {
            before_landing: Some(Arc::new(move || {
                arrived_tx
                    .send(())
                    .expect("B7.HARNESS: the locker stopped listening");
                match hook_locked_rx
                    .lock()
                    .expect("B7.HARNESS: the handshake mutex is poisoned")
                    .recv_timeout(Duration::from_secs(30))
                {
                    Ok(Ok(_phase)) => {}
                    Ok(Err(error)) => panic!("B7.HARNESS: the locker failed to lock: {error}"),
                    Err(RecvTimeoutError::Timeout) => {
                        panic!("B7.HARNESS: the lock never completed within the guard")
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        panic!("B7.HARNESS: the locker thread vanished")
                    }
                }
            })),
            ..DispatchFailpoints::default()
        });

    let observed = dispatches.dispatch(&ruling_digest, &content_for(NEW_BYTES));
    let locked_phase = locker
        .join()
        .expect("B7.HARNESS: the locker thread panicked");

    // The lock really landed, on the same service the dispatch is using, before the hook returned.
    assert_eq!(
        locked_phase,
        Ok(OperatorSessionPhase::Locked),
        "B7.PHASE.LOCK_COMPLETED_THEN_REFUSED: the lock must complete before the hook returns"
    );
    assert!(
        matches!(
            observed,
            Err(DispatchError::Account(
                beacond::OperatorAccountError::NotAuthenticated
            ))
        ),
        "B7.PHASE.LOCK_COMPLETED_THEN_REFUSED: the next phase must refuse as Account(NotAuthenticated), got {observed:?}"
    );

    let status = fixture
        .accounts
        .status()
        .expect("B7.LOCK.STATUS_UNAVAILABLE: status is readable while locked");
    assert!(
        !status.workspace_access,
        "B7.LOCK.STATUS_UNAVAILABLE: workspace_access must be false, got {status:?}"
    );
    assert!(
        !status.operator_commands,
        "B7.LOCK.STATUS_UNAVAILABLE: operator_commands must be false, got {status:?}"
    );

    // A NEW Command through the public service, with no failpoints, is the same exact error and not
    // a catch-all. Nothing is asserted about the store's rows: an unobservable absence is not a
    // claim this test can make.
    let after_lock = DispatchService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .dispatch(&ruling_digest, &content_for(NEW_BYTES));
    assert!(
        matches!(
            after_lock,
            Err(DispatchError::Account(
                beacond::OperatorAccountError::NotAuthenticated
            ))
        ),
        "B7.LOCK.NEW_COMMAND_REFUSED: a new Command must be Account(NotAuthenticated), got {after_lock:?}"
    );
}
