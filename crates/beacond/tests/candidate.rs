//! Candidate v1 through the real service (ADR 0039).
//!
//! Every Candidate checked here is assembled by [`CandidateRecord`], a byte-level builder that
//! follows ADR 0039's published table rather than calling Beacon's encoder. That independence is
//! the point: a canonicalizer that quietly changed its layout would agree with itself and disagree
//! with this file.
//!
//! The Workspace scaffolding is this file's own rather than shared with `tests/ruling.rs`, for the
//! reason that file gives: two test files leaning on one fixture can agree with each other about a
//! grammar that is wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, CandidateError, CandidateImages, CandidateService,
    DeclaredCheck, InconsistentCandidate, LocalModelActivationService, LocalModelEndpoint,
    MalformedCandidate, MissionDraft, MissionMemberDraft, MissionService, MutationKind,
    OperatorAccountError, OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft,
    ProjectDraft, ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService,
    ProjectService, ProposalService, ProviderFeature, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationService, ResidentLifecycleService, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    SupervisorError, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
use data_encoding::{BASE64, HEXLOWER};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A fabricated Repository Binding. Neither the path nor the commit id names anything that exists.
const REPOSITORY_SOURCE: &str = "/home/operator/projects/sample-tracker";
const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

const PROPOSAL_MAGIC: &[u8] = b"beacon.proposal.v1\0";
const CANDIDATE_MAGIC: &[u8] = b"beacon.candidate.v1\0";
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
        for revision in [1_u32, 1, 1] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, BASE_REVISION);
        push_string(&mut out, &self.summary);

        // Readable context: declared, one entry, manifest over the encoded entry list.
        out.push(0x02);
        let mut entries = Vec::new();
        push_count(&mut entries, 1);
        push_string(&mut entries, "src/lib.rs");
        entries.extend_from_slice(&[0x66; 32]);
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

/// One `beacon.candidate.v1` record, field by field in ADR 0039's stated order and widths.
///
/// Built from the published table, never from Beacon's encoder: an encoder that changed its layout
/// would agree with itself and disagree with this.
/// One entry as the published table spells it: `(kind tag, path, pre-image, post-image)`.
type RecordEntry = (u8, String, Option<[u8; 32]>, Option<[u8; 32]>);

#[derive(Debug, Clone)]
struct CandidateRecord {
    proposal_digest: String,
    pinned_base_revision: String,
    entries: Vec<RecordEntry>,
    recorded_at_ms: u64,
}

impl CandidateRecord {
    fn bytes(&self) -> Vec<u8> {
        let mut out = CANDIDATE_MAGIC.to_vec();
        out.extend_from_slice(&HEXLOWER.decode(self.proposal_digest.as_bytes()).unwrap());
        push_string(&mut out, "sha256");
        push_string(&mut out, "beacon.proposal.v1");
        push_string(&mut out, &self.pinned_base_revision);
        push_count(&mut out, self.entries.len());
        for (kind, path, pre, post) in &self.entries {
            out.push(*kind);
            push_string(&mut out, path);
            push_image(&mut out, pre.as_ref());
            push_image(&mut out, post.as_ref());
        }
        out.extend_from_slice(&self.recorded_at_ms.to_be_bytes());
        out
    }

    fn digest(&self) -> String {
        HEXLOWER.encode(&sha256(&self.bytes()))
    }
}

/// An image position: absence is a value, so it is a tag and nothing else.
fn push_image(out: &mut Vec<u8>, image: Option<&[u8; 32]>) {
    match image {
        None => out.push(0x00),
        Some(value) => {
            out.push(0x01);
            out.extend_from_slice(value);
        }
    }
}

struct TestStore {
    path: PathBuf,
}

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

    Fixture {
        store,
        proposals: ProposalService::new(Arc::clone(&accounts), workspace_id),
        candidates: CandidateService::new(Arc::clone(&accounts), workspace_id),
        accounts,
        workspace_id,
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configuration.resident_id,
        configuration_id: configuration.id,
    }
}

impl Fixture {
    fn proposal(&self, summary: &str) -> ProposalRecord {
        ProposalRecord {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            resident_id: self.resident_id,
            configuration_id: self.configuration_id,
            summary: summary.to_owned(),
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

    fn candidate_rows(&self) -> i64 {
        self.count("SELECT count(*) FROM candidates")
    }

    fn candidate_events(&self) -> i64 {
        self.count("SELECT count(*) FROM events WHERE kind = 'proposal.candidate_recorded'")
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
                 WHERE type = 'trigger' AND tbl_name = 'candidates'
                 ORDER BY name",
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
                "DROP TRIGGER candidates_no_update;
                 DROP TRIGGER candidates_no_delete;",
            )
            .unwrap();
        let outcome = f(&connection);
        for trigger in triggers {
            connection.execute_batch(&trigger).unwrap();
        }
        outcome
    }
}

fn inconsistent(error: &CandidateError) -> Option<InconsistentCandidate> {
    match error {
        CandidateError::Inconsistent(reason) => Some(*reason),
        _ => None,
    }
}

fn malformed(error: &CandidateError) -> Option<MalformedCandidate> {
    match error {
        CandidateError::Malformed(reason) => Some(*reason),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Recording and identity
// ---------------------------------------------------------------------------------------------

/// The images a truthful Candidate carries for [`default_mutations`]: a modification that changes
/// bytes, a deletion of bytes that existed, and a creation of bytes that did not.
fn default_images() -> Vec<CandidateImages> {
    vec![
        CandidateImages {
            pre_image_sha256: Some([0x11; 32]),
            post_image_sha256: Some([0x22; 32]),
        },
        CandidateImages {
            pre_image_sha256: Some([0x33; 32]),
            post_image_sha256: None,
        },
        CandidateImages {
            pre_image_sha256: None,
            post_image_sha256: Some([0x44; 32]),
        },
    ]
}

/// The same images as an independent record's entry list, given the Proposal they belong to.
fn expected_record(proposal_digest: &str, recorded_at_ms: u64) -> CandidateRecord {
    CandidateRecord {
        proposal_digest: proposal_digest.to_owned(),
        pinned_base_revision: BASE_REVISION.to_owned(),
        entries: vec![
            (
                MUTATION_MODIFY,
                "src/lib.rs".to_owned(),
                Some([0x11; 32]),
                Some([0x22; 32]),
            ),
            (
                MUTATION_DELETE,
                "src/old.rs".to_owned(),
                Some([0x33; 32]),
                None,
            ),
            (
                MUTATION_CREATE,
                "docs/new.md".to_owned(),
                None,
                Some([0x44; 32]),
            ),
        ],
        recorded_at_ms,
    }
}

#[test]
fn a_candidate_names_the_exact_proposal_and_base_it_was_built_against() {
    let fixture = admitted_mission("record");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    // The bytes are the record. An independently built record of the same facts must be them,
    // byte for byte, and the digest must be SHA-256 of exactly those bytes.
    let expected = expected_record(&proposal_digest, candidate.payload().recorded_at_ms);
    assert_eq!(candidate.canonical_bytes(), expected.bytes().as_slice());
    assert_eq!(candidate.digest().value, expected.digest());
    assert_eq!(candidate.digest().algorithm, "sha256");
    assert_eq!(candidate.digest().canonicalizer, "beacon.candidate.v1");

    // The Proposal's identity is copied whole, and the base revision with it: a pre-image digest
    // is a fact about one tree at one revision.
    let payload = candidate.payload();
    assert_eq!(payload.proposal.value, proposal_digest);
    assert_eq!(payload.proposal.algorithm, "sha256");
    assert_eq!(payload.proposal.canonicalizer, "beacon.proposal.v1");
    assert_eq!(payload.pinned_base_revision, BASE_REVISION);

    // One entry per declared mutation, carrying the Proposal's kinds and paths rather than the
    // caller's: the service was handed images only.
    assert_eq!(payload.entries.len(), 3);
    assert_eq!(payload.entries[0].kind, MutationKind::ModifyFile);
    assert_eq!(payload.entries[0].path, "src/lib.rs");
    assert_eq!(payload.entries[1].kind, MutationKind::DeleteFile);
    assert_eq!(payload.entries[2].kind, MutationKind::CreateFile);

    assert_eq!(fixture.candidate_rows(), 1);
    assert_eq!(fixture.candidate_events(), 1);

    // And the same Candidate reads back through both queries, from the stored bytes.
    let by_digest = fixture.candidates.get(&candidate.digest().value).unwrap();
    let for_proposal = fixture
        .candidates
        .for_proposal(&proposal_digest)
        .unwrap()
        .expect("the Proposal has a Candidate");
    assert_eq!(by_digest, candidate);
    assert_eq!(for_proposal, candidate);
}

#[test]
fn the_entries_follow_the_proposals_mutation_order_and_not_its_sorted_paths() {
    // The grammar stores target paths ascending and the mutation list in the caller's intended
    // order, and those two orders differ in every fixture here: sorted gives `docs/new.md` first,
    // the declared sequence gives `src/lib.rs`. A Candidate that mirrored the paths would pair
    // every image with the wrong file while every digest still verified.
    let fixture = admitted_mission("order");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let paths: Vec<&str> = candidate
        .payload()
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    assert_eq!(paths, vec!["src/lib.rs", "src/old.rs", "docs/new.md"]);
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_ne!(
        paths, sorted,
        "the fixture no longer distinguishes the two orders"
    );

    // Each image landed on its own declared mutation.
    assert_eq!(
        candidate.payload().entries[0].pre_image_sha256,
        Some([0x11; 32])
    );
    assert_eq!(
        candidate.payload().entries[1].pre_image_sha256,
        Some([0x33; 32])
    );
    assert_eq!(
        candidate.payload().entries[2].post_image_sha256,
        Some([0x44; 32])
    );
}

#[test]
fn a_second_candidate_for_one_proposal_is_refused_by_the_store() {
    // Uniqueness is what makes the byte selection determinate: two sets of post-image digests for
    // one Proposal would leave "which bytes" with no answer. It is not what makes it authorized --
    // that is Ruling v2, which binds this record's digest.
    let fixture = admitted_mission("unique");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let first = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let mut different = default_images();
    different[0].post_image_sha256 = Some([0x55; 32]);
    let refusal = fixture
        .candidates
        .record(&proposal_digest, &different)
        .unwrap_err();

    assert_eq!(
        inconsistent(&refusal),
        Some(InconsistentCandidate::AlreadyRecorded)
    );
    // Refused means nothing was written: one row, one Event, and the first Candidate unchanged.
    assert_eq!(fixture.candidate_rows(), 1);
    assert_eq!(fixture.candidate_events(), 1);
    assert_eq!(
        fixture.candidates.get(&first.digest().value).unwrap(),
        first
    );
}

#[test]
fn images_that_do_not_match_the_proposals_mutations_write_nothing() {
    let fixture = admitted_mission("mismatch");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    // One image too few: the entry list would not mirror the mutation list.
    let short = &default_images()[..2];
    let refusal = fixture
        .candidates
        .record(&proposal_digest, short)
        .unwrap_err();
    assert_eq!(
        inconsistent(&refusal),
        Some(InconsistentCandidate::EntryCountMismatch)
    );

    // A creation carrying a pre-image: the kind says the file did not exist, and the image says
    // what it held. Both cannot be true.
    let mut wrong_kind = default_images();
    wrong_kind[2].pre_image_sha256 = Some([0x66; 32]);
    let refusal = fixture
        .candidates
        .record(&proposal_digest, &wrong_kind)
        .unwrap_err();
    assert_eq!(malformed(&refusal), Some(MalformedCandidate::ImageForKind));

    // A modification that changes nothing declares no effect, and the mutation declared one.
    let mut empty_effect = default_images();
    empty_effect[0].post_image_sha256 = empty_effect[0].pre_image_sha256;
    let refusal = fixture
        .candidates
        .record(&proposal_digest, &empty_effect)
        .unwrap_err();
    assert_eq!(malformed(&refusal), Some(MalformedCandidate::EmptyEffect));

    assert_eq!(fixture.candidate_rows(), 0);
    assert_eq!(fixture.candidate_events(), 0);
}

#[test]
fn a_candidate_for_a_proposal_this_workspace_never_recorded_is_refused() {
    let fixture = admitted_mission("absent");
    // A well-formed digest that names nothing.
    let refusal = fixture
        .candidates
        .record(&"a".repeat(64), &default_images())
        .unwrap_err();
    assert_eq!(
        inconsistent(&refusal),
        Some(InconsistentCandidate::ProposalNotFound)
    );

    // A digest that is not one at all is refused before the store is consulted.
    let refusal = fixture
        .candidates
        .record("not-a-digest", &default_images())
        .unwrap_err();
    assert_eq!(
        malformed(&refusal),
        Some(MalformedCandidate::ProposalDigestValue)
    );
    assert_eq!(fixture.candidate_rows(), 0);
    assert_eq!(fixture.candidate_events(), 0);
}

#[test]
fn no_candidate_yet_and_no_such_proposal_are_different_answers() {
    // A dispatch acts differently on each: one is "nothing to apply yet", the other is "this
    // decision names bytes this Workspace does not hold". Collapsing them into `None` would make
    // the second look like the first.
    let fixture = admitted_mission("distinct");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    assert_eq!(
        fixture.candidates.for_proposal(&proposal_digest).unwrap(),
        None
    );
    let refusal = fixture
        .candidates
        .for_proposal(&"b".repeat(64))
        .unwrap_err();
    assert_eq!(
        inconsistent(&refusal),
        Some(InconsistentCandidate::ProposalNotFound)
    );
}

#[test]
fn the_audit_event_carries_the_exact_bytes_as_padded_base64_and_the_owner_actor() {
    let fixture = admitted_mission("event");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let connection = fixture.store.connect();
    let (kind, payload_json, occurred_at_ms) = connection
        .query_row(
            "SELECT kind, payload_json, occurred_at_ms FROM events
             WHERE kind = 'proposal.candidate_recorded'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(kind, "proposal.candidate_recorded");
    assert_eq!(
        occurred_at_ms as u64,
        candidate.payload().recorded_at_ms,
        "the Event's time and the bytes' time are one fact"
    );

    let value: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
    let recorded = &value["candidate"];
    assert_eq!(recorded["actor"], "owner");
    assert_eq!(recorded["digest"]["value"], candidate.digest().value);
    assert_eq!(recorded["digest"]["canonicalizer"], "beacon.candidate.v1");
    let carried = BASE64
        .decode(
            recorded["canonicalBytesBase64"]
                .as_str()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
    assert_eq!(carried, candidate.canonical_bytes());
}

#[test]
fn recording_a_candidate_creates_no_run_receipt_worktree_or_external_effect() {
    // A Candidate says what a change WOULD do. It decides nothing and applies nothing, so nothing
    // in the execution half of the store may move when one is recorded.
    let fixture = admitted_mission("inert");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    let before = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("beacon-worktree-")
        })
        .count();

    fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    assert_eq!(fixture.count("SELECT count(*) FROM runs"), 0);
    assert_eq!(fixture.count("SELECT count(*) FROM receipts"), 0);
    let after = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("beacon-worktree-")
        })
        .count();
    assert_eq!(before, after, "recording a Candidate created a worktree");
}

#[test]
fn unauthenticated_recording_returns_only_the_account_refusal() {
    // The digest is malformed too. An implementation that validated it before authenticating
    // would hand an unauthenticated caller a validation oracle.
    let fixture = admitted_mission("locked");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    fixture.accounts.lock().unwrap();

    let refusal = fixture
        .candidates
        .record("not-a-digest", &default_images())
        .unwrap_err();

    assert!(
        matches!(
            refusal,
            CandidateError::Account(OperatorAccountError::NotAuthenticated)
        ),
        "{refusal:?}"
    );
    let _ = proposal_digest;
    assert_eq!(fixture.candidate_rows(), 0);
    assert_eq!(fixture.candidate_events(), 0);
}

// ---------------------------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------------------------

#[test]
fn a_recorded_candidate_survives_a_restart() {
    // The positive control for every reconciliation test below: an untampered store starts.
    let fixture = admitted_mission("restart");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    fixture.restart().expect("an untampered store starts");

    assert_eq!(
        fixture.candidates.get(&candidate.digest().value).unwrap(),
        candidate
    );
}

#[test]
fn a_row_that_disagrees_with_its_own_bytes_fails_startup() {
    let fixture = admitted_mission("tampered-row");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    // The bytes still hash to the digest and still decode; only the indexed count moved. Without
    // `columns_match` this store would read as sound.
    fixture.tamper(&format!(
        "UPDATE candidates SET entry_count = 2 WHERE digest_value = '{}'",
        candidate.digest().value
    ));

    // Either reconciliation half may reach it first -- the Event replay re-verifies the same row
    // its payload names, and the whole-table pass re-verifies every row. Both are refusals of this
    // store; asserting one variant would be asserting the order they run in.
    let outcome = fixture.restart();
    assert!(
        matches!(
            outcome,
            Err(SupervisorError::CandidateStateMismatch { .. }
                | SupervisorError::CandidateEventMismatch(_))
        ),
        "a row disagreeing with its bytes started: {outcome:?}"
    );
}

#[test]
fn a_candidate_whose_entries_no_longer_match_its_proposal_fails_startup() {
    // The check no read performs, and the whole point of copying: these bytes verify against their
    // own digest, decode, re-encode, and agree with every indexed column. Only the Proposal they
    // name disagrees -- one entry has drifted onto a path nobody proposed. Without the agreement
    // block in reconciliation this store starts clean and a dispatch would write to that path.
    let fixture = admitted_mission("drifted-entries");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let mut drifted = expected_record(&proposal_digest, candidate.payload().recorded_at_ms);
    drifted.entries[0].1 = "src/never-proposed.rs".to_owned();
    let bytes = drifted.bytes();
    let digest = drifted.digest();
    // The row AND its Event are rewritten together, so the store is internally consistent and only
    // the copy rule is broken.
    let event_payload = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": digest,
            },
            "canonicalBytesBase64": BASE64.encode(&bytes),
            "actor": "owner",
            "recordedAtMs": candidate.payload().recorded_at_ms,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "UPDATE candidates SET canonical_bytes = X'{}', digest_value = '{}';
         UPDATE events SET payload_json = '{}' WHERE kind = 'proposal.candidate_recorded';",
        HEXLOWER.encode(&bytes),
        digest,
        event_payload.replace('\'', "''")
    ));

    let outcome = fixture.restart();
    assert!(
        matches!(outcome, Err(SupervisorError::CandidateStateMismatch { .. })),
        "entries that no longer match their Proposal started: {outcome:?}"
    );
}

#[test]
fn a_candidate_without_its_event_fails_startup() {
    let fixture = admitted_mission("orphan-row");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    fixture.tamper("DELETE FROM events WHERE kind = 'proposal.candidate_recorded'");

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::CandidateStateMismatch { .. })
        ),
        "a Candidate with no evidence started"
    );
}

#[test]
fn an_event_naming_no_candidate_fails_startup() {
    let fixture = admitted_mission("orphan-event");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    // A second Event for the same Candidate: the count rule refuses it. An Event naming a digest
    // no row carries is refused by the same rule from the other side.
    let payload = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'proposal.candidate_recorded'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    fixture.tamper(&format!(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES ('{}', 'proposal.candidate_recorded', '{}', {})",
        fixture.workspace_id,
        payload.replace('\'', "''"),
        candidate.payload().recorded_at_ms
    ));

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::CandidateStateMismatch { .. })
        ),
        "a duplicated Event started"
    );
}

#[test]
fn a_candidate_read_for_use_proves_its_event_pairing_too() {
    // gpt1's kernel read: a consumer that decides inside one transaction cannot borrow the last
    // startup's conclusion, so the read a consumer performs must prove what startup proves. Here
    // the row and its bytes are untouched and only the evidence is gone: a reader that checked the
    // record and not its Event would hand this Candidate to a dispatch.
    let fixture = admitted_mission("read-proves-event");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();
    assert!(
        fixture
            .candidates
            .for_proposal(&proposal_digest)
            .unwrap()
            .is_some(),
        "the positive control: an untampered Candidate reads back"
    );

    fixture.tamper("DELETE FROM events WHERE kind = 'proposal.candidate_recorded'");

    let outcome = fixture.candidates.for_proposal(&proposal_digest);
    assert!(
        matches!(outcome, Err(CandidateError::InvalidStore)),
        "a Candidate with no evidence was handed out for use: {outcome:?}"
    );
}

#[test]
fn a_candidate_read_for_use_proves_the_copy_rule_too() {
    // The same reasoning for the other half: entries that no longer match the Proposal's declared
    // mutations must refuse at read time, not only at the next restart.
    let fixture = admitted_mission("read-proves-copy");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let mut drifted = expected_record(&proposal_digest, candidate.payload().recorded_at_ms);
    drifted.entries[0].1 = "src/never-proposed.rs".to_owned();
    let bytes = drifted.bytes();
    let digest = drifted.digest();
    let event_payload = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": digest,
            },
            "canonicalBytesBase64": BASE64.encode(&bytes),
            "actor": "owner",
            "recordedAtMs": candidate.payload().recorded_at_ms,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "UPDATE candidates SET canonical_bytes = X'{}', digest_value = '{}';
         UPDATE events SET payload_json = '{}' WHERE kind = 'proposal.candidate_recorded';",
        HEXLOWER.encode(&bytes),
        digest,
        event_payload.replace('\'', "''")
    ));

    let outcome = fixture.candidates.for_proposal(&proposal_digest);
    assert!(
        matches!(outcome, Err(CandidateError::InvalidStore)),
        "entries that no longer match their Proposal were handed out for use: {outcome:?}"
    );
}

#[test]
fn a_candidate_larger_than_the_envelope_is_refused_by_the_codec_not_by_the_store() {
    // gpt1's P0. Per-field limits do not bound a record: a legal sub-64-KiB Proposal with enough
    // declared mutations expands into a Candidate that exceeds the envelope ADR 0039 promises,
    // because every entry adds a path plus 66 bytes of images. The refusal must be the codec's and
    // must name the envelope -- reporting a length failure as "already recorded" sent a caller to
    // look for a Candidate that does not exist.
    let fixture = admitted_mission("over-envelope");
    let mut record = fixture.proposal("Rewrite every module header in one pass.");
    // Short paths on purpose: the Proposal spends 2*len + 17 bytes per target and the Candidate
    // spends len + 75, so the two envelopes are crossed at different counts. 800 nine-byte paths
    // is ~28 KiB of Proposal and just over 64 KiB of Candidate.
    record.mutations = (0..800)
        .map(|index| (MUTATION_MODIFY, format!("s/f{index:03}.rs")))
        .collect();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let images = vec![
        CandidateImages {
            pre_image_sha256: Some([0x11; 32]),
            post_image_sha256: Some([0x22; 32]),
        };
        800
    ];

    let refusal = fixture
        .candidates
        .record(&proposal.digest().value, &images)
        .unwrap_err();

    assert_eq!(
        malformed(&refusal),
        Some(MalformedCandidate::OverLimitLength),
        "an over-envelope Candidate was not refused by the codec: {refusal:?}"
    );
    assert_eq!(fixture.candidate_rows(), 0);
    assert_eq!(fixture.candidate_events(), 0);

    // The positive control: the same Proposal shape, small enough to fit, records normally. Without
    // it, a codec that refused everything would pass the assertion above.
    let fixture = admitted_mission("under-envelope");
    let mut record = fixture.proposal("Rewrite a few module headers.");
    record.mutations = (0..200)
        .map(|index| (MUTATION_MODIFY, format!("s/f{index:03}.rs")))
        .collect();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let images = vec![
        CandidateImages {
            pre_image_sha256: Some([0x11; 32]),
            post_image_sha256: Some([0x22; 32]),
        };
        200
    ];
    let candidate = fixture
        .candidates
        .record(&proposal.digest().value, &images)
        .unwrap();
    assert!(candidate.canonical_bytes().len() <= 65_536);
    assert_eq!(fixture.candidate_rows(), 1);
}

#[test]
fn a_row_whose_bytes_were_swapped_for_another_valid_candidate_fails_startup() {
    // gpt1's P1: removing the stored-byte SHA-256 comparison left every test green, because the
    // one tampering test that changed bytes also changed `digest_value` to match. This is the
    // shape that needs the hash: the row keeps its digest and its columns stay self-consistent,
    // and only the bytes are another well-formed Candidate's.
    let fixture = admitted_mission("swapped-bytes");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    // A different, entirely valid record for the same Proposal: same entries, different images.
    let mut other = expected_record(&proposal_digest, candidate.payload().recorded_at_ms);
    other.entries[0].3 = Some([0x99; 32]);
    let other_bytes = other.bytes();
    assert_ne!(other_bytes, candidate.canonical_bytes());
    assert_ne!(other.digest(), candidate.digest().value);

    // The bytes move in the row AND in its Event, so the two still agree with each other and every
    // indexed column still describes them: entry count, base revision, Proposal digest and recorded
    // time are identical between the two records. The ONLY thing left that can notice is that the
    // digest no longer names these bytes.
    let event_payload = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": candidate.digest().value,
            },
            "canonicalBytesBase64": BASE64.encode(&other_bytes),
            "actor": "owner",
            "recordedAtMs": candidate.payload().recorded_at_ms,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "UPDATE candidates SET canonical_bytes = X'{}' WHERE digest_value = '{}';
         UPDATE events SET payload_json = '{}' WHERE kind = 'proposal.candidate_recorded';",
        HEXLOWER.encode(&other_bytes),
        candidate.digest().value,
        event_payload.replace('\'', "''")
    ));

    let outcome = fixture.restart();
    assert!(
        matches!(
            outcome,
            Err(SupervisorError::CandidateStateMismatch { .. }
                | SupervisorError::CandidateEventMismatch(_))
        ),
        "bytes swapped for another valid Candidate started: {outcome:?}"
    );
}

#[test]
fn an_event_whose_digest_names_no_candidate_is_refused_by_the_replay() {
    // gpt1's P1: the test that claimed this actually duplicated an Event for an existing
    // Candidate, which the per-Candidate count rule catches on its own. This is a TRUE orphan --
    // an Event naming a digest no row carries -- and running it settles which guard fires: the
    // startup replay reaches every event row and `verify_candidate_event` cannot load a record for
    // this one. The whole-table count in `verify_candidate_state` is therefore unreachable today,
    // and says so in its own comment rather than being claimed as this test's subject.
    let fixture = admitted_mission("true-orphan");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let orphan = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": "f".repeat(64),
            },
            "canonicalBytesBase64": BASE64.encode(candidate.canonical_bytes()),
            "actor": "owner",
            "recordedAtMs": candidate.payload().recorded_at_ms,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES ('{}', 'proposal.candidate_recorded', '{}', {})",
        fixture.workspace_id,
        orphan.replace('\'', "''"),
        candidate.payload().recorded_at_ms
    ));

    let outcome = fixture.restart();
    assert!(
        matches!(outcome, Err(SupervisorError::CandidateEventMismatch(_))),
        "an Event naming no Candidate started: {outcome:?}"
    );
}

#[test]
fn a_candidate_read_for_use_verifies_its_event_rather_than_counting_it() {
    // gpt1's slice-2 P1. Counting rows whose digest matches proves a row exists and nothing about
    // what it says: a probe changed only `$.candidate.actor` to a value the closed vocabulary does
    // not have, and the reader handed the Candidate out. The point-of-use proof now runs the owning
    // Event verifier against that exact row, so the actor, the time, the envelope and the bytes are
    // all checked where the record is used.
    let fixture = admitted_mission("event-verified");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let candidate = fixture
        .candidates
        .record(&proposal_digest, &default_images())
        .unwrap();

    let tampered = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": candidate.digest().value,
            },
            "canonicalBytesBase64": BASE64.encode(candidate.canonical_bytes()),
            "actor": "resident",
            "recordedAtMs": candidate.payload().recorded_at_ms,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "UPDATE events SET payload_json = '{}' WHERE kind = 'proposal.candidate_recorded'",
        tampered.replace('\'', "''")
    ));

    // The refusal is the JSON one, because the actor vocabulary is closed and `resident` is not in
    // it -- reached only because the reader now parses the Event instead of counting it.
    let outcome = fixture.candidates.for_proposal(&proposal_digest);
    assert!(
        matches!(
            outcome,
            Err(CandidateError::InvalidStore | CandidateError::Json(_))
        ),
        "an Event naming an actor the vocabulary does not have was accepted: {outcome:?}"
    );

    // And the same Event with a valid actor but the wrong recorded time is refused too, so the
    // check is not merely serde's.
    let wrong_time = serde_json::json!({
        "candidate": {
            "digest": {
                "algorithm": "sha256",
                "canonicalizer": "beacon.candidate.v1",
                "value": candidate.digest().value,
            },
            "canonicalBytesBase64": BASE64.encode(candidate.canonical_bytes()),
            "actor": "owner",
            "recordedAtMs": candidate.payload().recorded_at_ms + 1,
        }
    })
    .to_string();
    fixture.tamper(&format!(
        "UPDATE events SET payload_json = '{}' WHERE kind = 'proposal.candidate_recorded'",
        wrong_time.replace('\'', "''")
    ));
    let outcome = fixture.candidates.for_proposal(&proposal_digest);
    assert!(
        matches!(outcome, Err(CandidateError::InvalidStore)),
        "an Event whose time disagrees with its record was accepted: {outcome:?}"
    );
}

/// R28 tier 2 pass 5 of 14: a Candidate row the log does not account for is refused at startup,
/// and the refusal names WHICH Candidate and carries the reader's own fault.
///
/// Pass 1's plant shape; the planted digest is all `f`, so it sorts last by construction (R39.3).
#[test]
fn startup_refuses_an_unaccounted_candidate_row_and_names_the_row() {
    // R44: BOTH ENDS. A planted digest of all `f` sorts LAST in this pass's own
    // ORDER BY and all `0` sorts FIRST -- the extremes of the hex space, so the
    // placement holds against any honest digest. Tampering only one end makes
    // "names the failing row" and "names that end" the same assertion.
    for (end, fill) in [("last", "f"), ("first", "0")] {
        let fixture = admitted_mission(&format!("r28t2-candidate-startup-{end}"));
        let proposal_digest = fixture.submit("A Proposal for the planted Candidate to copy.");
        fixture
            .candidates
            .record(&proposal_digest, &default_images())
            .expect("the fixture records a Candidate");
        // `candidates` is UNIQUE on (workspace_id, proposal_digest) -- one Candidate per Proposal --
        // so the planted copy needs a Proposal of its own to name (measured: the first draft got that
        // constraint back).
        let second_proposal =
            fixture.submit("A second Proposal, so the planted row has one to name.");

        let connection = fixture.store.connect();
        let existing: String = connection
            .query_row(
                "SELECT digest_value FROM candidates ORDER BY digest_value LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the fixture recorded a Candidate");
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('candidates')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let planted = fill.repeat(64);
        let names = columns.join(", ");
        let values = columns
            .iter()
            .map(|column| match column.as_str() {
                "digest_value" => "?1".to_owned(),
                "proposal_digest" => "?3".to_owned(),
                _ => column.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute(
                &format!(
                    "INSERT INTO candidates ({names}) SELECT {values} FROM candidates
                 WHERE digest_value = ?2"
                ),
                rusqlite::params![planted, existing, second_proposal],
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
            let enumerated =
                beacond::enumerate_candidates(&connection).expect("the pass's enumeration reads");
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

        drop(connection);

        let refusal = fixture
            .restart()
            .expect_err("a store holding an unaccounted Candidate row reopened Ready");
        assert!(
            matches!(
                &refusal,
                SupervisorError::CandidateStateMismatch {
                    workspace_id,
                    digest_value,
                    ..
                } if *workspace_id == fixture.workspace_id && digest_value == &planted
            ),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );

        // R28.4: the key and the fault's NAME (kimi's P3-2 form).
        let line = refusal.to_string();
        assert!(
            line.contains(&planted) && line.contains(&fixture.workspace_id.to_string()),
            "the refusal does not name the row it refused: {line}"
        );
        let named = line.rsplit(": ").next().unwrap().to_owned();
        // kimi's P2: PINNED, not just shaped. A name-shaped tail is satisfied by ANY arm,
        // so without this a pass carrying a fixed-but-wrong fault survives -- the M9 class
        // measured surviving on the pass-7 bar.
        assert_eq!(
            named, "InvalidStore",
            "the refusal carries the wrong fault: {line}"
        );
        assert!(
            !named.contains(' ') && !named.contains('{'),
            "the tail of the line is a rendering, not a name: {line}"
        );
        assert!(
            format!("{refusal:?}").contains(&named),
            "the named fault is not reachable through Debug: {refusal:?}"
        );
    }
}
