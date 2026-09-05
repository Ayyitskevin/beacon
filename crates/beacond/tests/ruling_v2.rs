//! Ruling v2 through the real service (ADR 0039): the authority link.
//!
//! A v2 Ruling is a v1 Ruling that also names the exact Candidate the Operator was shown. This file
//! assembles both grammars with byte-level builders that follow the published tables rather than
//! calling Beacon's encoders, for the reason `tests/ruling.rs` gives: an encoder that changed its
//! layout would agree with itself and disagree with this file.
//!
//! What it is really testing is one sentence: **prior existence is not authorization**. The
//! Candidate ordering rule proves bytes existed before a decision; only this record proves the
//! decision named them.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, CandidateImages, CandidateService, DeclaredCheck,
    LocalModelActivationService, LocalModelEndpoint, MissionDraft, MissionMemberDraft,
    MissionService, OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft,
    ProjectDraft, ProjectPolicyDraft, ProjectPolicyService, ProjectRepositoryService,
    ProjectService, ProposalService, ProviderFeature, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationService, ResidentLifecycleService, RulingDecision, RulingError,
    RulingGrammar, RulingService, RulingSupersedes, SecretString, StarterOrganizationService,
    StarterResidentDraft, StarterRole, StartupConfig, Supervisor, SupervisorError, TeamDraft,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TeamMemberDraft, TeamService, TrackRecordEvidenceKind, TrackRecordRequirement,
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
    rulings: RulingService,
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
        rulings: RulingService::new(Arc::clone(&accounts), workspace_id),
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
// An independent encoder for beacon.ruling.v2
// ---------------------------------------------------------------------------------------------

const RULING_V1_MAGIC: &[u8] = b"beacon.ruling.v1\0";
const RULING_V2_MAGIC: &[u8] = b"beacon.ruling.v2\0";
const APPROVE_REASON: &str = "The declared effects stay inside the Repository and spend nothing.";

/// One `beacon.ruling.v2` record, field by field in ADR 0039's stated order and widths: v1's
/// fields with the Candidate's envelope inserted after the Proposal's.
#[derive(Debug, Clone)]
struct RulingV2Record {
    proposal_digest: String,
    candidate_digest: Option<String>,
    decision: u8,
    reason: String,
    supersedes: Option<String>,
    ruled_at_ms: u64,
}

impl RulingV2Record {
    fn bytes(&self) -> Vec<u8> {
        let mut out = match self.candidate_digest {
            None => RULING_V1_MAGIC.to_vec(),
            Some(_) => RULING_V2_MAGIC.to_vec(),
        };
        out.extend_from_slice(&HEXLOWER.decode(self.proposal_digest.as_bytes()).unwrap());
        push_string(&mut out, "sha256");
        push_string(&mut out, "beacon.proposal.v1");
        if let Some(candidate) = &self.candidate_digest {
            out.extend_from_slice(&HEXLOWER.decode(candidate.as_bytes()).unwrap());
            push_string(&mut out, "sha256");
            push_string(&mut out, "beacon.candidate.v1");
        }
        for revision in [1_u32, 1, 1] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, BASE_REVISION);
        out.push(self.decision);
        push_string(&mut out, &self.reason);
        out.push(0x01);
        match &self.supersedes {
            None => out.push(0x00),
            Some(digest) => {
                out.push(0x01);
                out.extend_from_slice(&HEXLOWER.decode(digest.as_bytes()).unwrap());
            }
        }
        out.extend_from_slice(&self.ruled_at_ms.to_be_bytes());
        out
    }

    fn digest(&self) -> String {
        HEXLOWER.encode(&sha256(&self.bytes()))
    }
}

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

impl Fixture {
    /// A submitted Proposal with its one recorded Candidate.
    fn proposal_with_candidate(&self, summary: &str) -> (String, String) {
        let proposal_digest = self.submit(summary);
        let candidate = self
            .candidates
            .record(&proposal_digest, &default_images())
            .unwrap();
        (proposal_digest, candidate.digest().value.clone())
    }
}

// ---------------------------------------------------------------------------------------------
// The authority link
// ---------------------------------------------------------------------------------------------

#[test]
fn a_v2_ruling_names_the_exact_candidate_it_was_shown() {
    let fixture = admitted_mission("v2-record");
    let (proposal_digest, candidate_digest) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");

    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    // The bytes are the record: an independently built v2 record of the same facts must be them.
    let expected = RulingV2Record {
        proposal_digest: proposal_digest.clone(),
        candidate_digest: Some(candidate_digest.clone()),
        decision: 0x01,
        reason: APPROVE_REASON.to_owned(),
        supersedes: None,
        ruled_at_ms: ruling.payload().ruled_at_ms,
    };
    assert_eq!(ruling.canonical_bytes(), expected.bytes().as_slice());
    assert_eq!(ruling.digest().value, expected.digest());
    assert_eq!(ruling.digest().canonicalizer, "beacon.ruling.v2");

    // The Candidate is carried as a whole envelope, and the grammar says which record this is.
    assert_eq!(ruling.grammar(), RulingGrammar::V2);
    let candidate = ruling.candidate().expect("a v2 Ruling names a Candidate");
    assert_eq!(candidate.value, candidate_digest);
    assert_eq!(candidate.algorithm, "sha256");
    assert_eq!(candidate.canonicalizer, "beacon.candidate.v1");

    // v1's whole payload is still there and still means what it meant.
    assert_eq!(ruling.payload().proposal.value, proposal_digest);
    assert_eq!(ruling.payload().decision, RulingDecision::Approve);
    assert_eq!(
        ruling.payload().bindings.pinned_base_revision,
        BASE_REVISION
    );
    assert!(ruling.force().superseded_by().is_none());
    assert!(ruling.force().drift().is_empty());

    assert_eq!(fixture.candidate_bindings(), 1);
    assert_eq!(fixture.ruling_rows(), 1);
    assert_eq!(fixture.ruling_events(), 1);
    assert_eq!(fixture.rulings.get(&ruling.digest().value).unwrap(), ruling);
}

#[test]
fn a_v1_ruling_stays_valid_and_names_no_candidate() {
    // R8: nothing already recorded is invalidated by v2's arrival. A v1 Ruling is a complete
    // record of a decision; it simply names no bytes, which is why dispatch cannot act on it.
    let fixture = admitted_mission("v1-still-valid");
    let (proposal_digest, _) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");

    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    assert_eq!(ruling.grammar(), RulingGrammar::V1);
    assert!(ruling.candidate().is_none());
    assert_eq!(ruling.digest().canonicalizer, "beacon.ruling.v1");
    assert_eq!(
        ruling.canonical_bytes(),
        RulingV2Record {
            proposal_digest,
            candidate_digest: None,
            decision: 0x01,
            reason: APPROVE_REASON.to_owned(),
            supersedes: None,
            ruled_at_ms: ruling.payload().ruled_at_ms,
        }
        .bytes()
        .as_slice(),
        "a v1 Ruling's bytes are exactly what they were before v2 existed"
    );
    // No relation row: "names no Candidate" is the absence of one, not a null column.
    assert_eq!(fixture.candidate_bindings(), 0);
    fixture.restart().expect("a v1 Ruling survives a restart");
}

#[test]
fn a_ruling_can_only_be_recorded_for_a_change_that_was_presented() {
    // gpt1's P0: "the Candidate the Operator was shown" was prose -- the Command took two digest
    // strings, so anyone holding them could create authority over bytes nobody displayed. It now
    // takes a `CandidatePresentation`, which only `CandidateService::present` produces. A caller
    // cannot build one: its fields are private and it derives no `Deserialize`, so the compiler
    // refuses the unshown call rather than the store refusing it afterwards.
    let fixture = admitted_mission("presented");
    let (proposal_digest, candidate_digest) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");

    // What the Operator is shown IS what the Ruling binds: the same Candidate identity, the same
    // base revision, and one line per declared mutation carrying the exact bytes before and after.
    let presentation = fixture.candidates.present(&proposal_digest).unwrap();
    assert_eq!(presentation.candidate().value, candidate_digest);
    assert_eq!(presentation.proposal().value, proposal_digest);
    assert_eq!(presentation.pinned_base_revision(), BASE_REVISION);
    assert_eq!(presentation.lines().len(), 3);
    assert_eq!(presentation.lines()[0].path, "src/lib.rs");
    assert_eq!(
        presentation.lines()[0].post_image_sha256.as_deref(),
        Some(HEXLOWER.encode(&[0x22; 32]).as_str())
    );
    assert_eq!(
        presentation.lines()[1].post_image_sha256,
        None,
        "a deletion has no after"
    );
    assert_eq!(
        presentation.lines()[2].pre_image_sha256,
        None,
        "a creation has no before"
    );

    let ruling = fixture
        .rulings
        .rule_on_candidate(
            &presentation,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();
    assert_eq!(ruling.candidate().unwrap().value, candidate_digest);

    // A second Proposal has its own Candidate, and its presentation binds that one. There is no
    // expressible call that crosses them.
    let (other_proposal, other_candidate) =
        fixture.proposal_with_candidate("Add a second regression test, for the other invariant.");
    assert_ne!(other_candidate, candidate_digest);
    let other = fixture.candidates.present(&other_proposal).unwrap();
    assert_eq!(other.candidate().value, other_candidate);
}

#[test]
fn a_proposal_with_no_candidate_cannot_be_presented_and_so_cannot_be_ruled_under_v2() {
    // The refusal moves to the door it belongs at: there is nothing to show, so there is nothing to
    // rule on. The correction an Operator reads is "record a Candidate first".
    let fixture = admitted_mission("no-candidate");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    let refusal = fixture.candidates.present(&proposal_digest).unwrap_err();
    assert!(
        matches!(
            refusal,
            beacond::CandidateError::Inconsistent(beacond::InconsistentCandidate::NotRecorded)
        ),
        "{refusal:?}"
    );
    assert_eq!(fixture.ruling_rows(), 0);
    assert_eq!(fixture.candidate_bindings(), 0);
}

#[test]
fn a_v2_ruling_supersedes_a_v1_one_and_the_force_reports_the_successors_own_grammar() {
    // The ordinary way a Proposal becomes dispatchable: it was ruled before the Candidate existed,
    // and is ruled again under the grammar that names one. The chain crosses grammars, and the
    // superseded Ruling must report its successor's envelope rather than its own.
    let fixture = admitted_mission("cross-grammar-chain");
    let (proposal_digest, _candidate) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");

    let first = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Not yet -- the checks are not selected.",
            RulingSupersedes::None,
        )
        .unwrap();
    let second = fixture
        .rulings
        .rule_on_candidate(
            &fixture.candidates.present(&proposal_digest).unwrap(),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::Ruling(first.digest().value.clone()),
        )
        .unwrap();

    let first = fixture.rulings.get(&first.digest().value).unwrap();
    let superseded_by = first
        .force()
        .superseded_by()
        .expect("the first Ruling was reversed");
    assert_eq!(superseded_by.value, second.digest().value);
    assert_eq!(
        superseded_by.canonicalizer, "beacon.ruling.v2",
        "the successor's grammar was assumed rather than read"
    );
    assert!(second.force().superseded_by().is_none());

    let chain = fixture.rulings.list_for_proposal(&proposal_digest).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].grammar(), RulingGrammar::V1);
    assert_eq!(chain[1].grammar(), RulingGrammar::V2);
    fixture
        .restart()
        .expect("a cross-grammar chain survives a restart");
}

#[test]
fn a_row_claiming_v2_over_v1_bytes_fails_startup() {
    // The guard the CHECK constraints cannot be: both canonicalizers are legal column values now,
    // so what keeps them apart is the bytes' own magic. A row claiming to bind a Candidate whose
    // bytes name none would let a dispatch believe a decision covered bytes nobody was shown.
    let fixture = admitted_mission("grammar-lie");
    let (proposal_digest, candidate_digest) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");
    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    fixture.tamper(&format!(
        "UPDATE rulings SET digest_canonicalizer = 'beacon.ruling.v2' WHERE digest_value = '{}';
         INSERT INTO ruling_candidates (workspace_id, proposal_digest, ruling_digest, candidate_digest)
         VALUES ('{}', '{}', '{}', '{}');",
        ruling.digest().value,
        fixture.workspace_id,
        proposal_digest,
        ruling.digest().value,
        candidate_digest,
    ));

    let outcome = fixture.restart();
    assert!(
        matches!(
            outcome,
            Err(SupervisorError::RulingStateMismatch { .. }
                | SupervisorError::RulingEventMismatch(_))
        ),
        "a row claiming a Candidate its bytes do not name started: {outcome:?}"
    );
}

#[test]
fn unauthenticated_v2_recording_returns_only_the_account_refusal() {
    // The presentation is obtained while authenticated and used after the session locks: the
    // account gate must refuse before any of the record work happens, so holding a valid
    // presentation is not a way past it.
    let fixture = admitted_mission("v2-locked");
    let (proposal_digest, _) =
        fixture.proposal_with_candidate("Add a regression test for the invariant checker.");
    let presentation = fixture.candidates.present(&proposal_digest).unwrap();
    fixture.accounts.lock().unwrap();

    let refusal = fixture
        .rulings
        .rule_on_candidate(
            &presentation,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap_err();

    assert!(
        matches!(
            refusal,
            RulingError::Account(beacond::OperatorAccountError::NotAuthenticated)
        ),
        "{refusal:?}"
    );
    assert_eq!(fixture.ruling_rows(), 0);
    assert_eq!(fixture.candidate_bindings(), 0);
}
