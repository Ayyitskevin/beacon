//! Ruling v1 through the real service (ADR 0037).
//!
//! Every Ruling record checked here is assembled by [`RulingRecord`], a byte-level builder that
//! follows ADR 0037's published table rather than calling Beacon's encoder. That independence is
//! the point: a canonicalizer that quietly changed its layout would agree with itself and disagree
//! with this file.
//!
//! The Workspace scaffolding below is deliberately this file's own rather than shared with
//! `tests/proposal.rs`. Slice 2 does not reopen the file six review rounds hardened, and the one
//! thing that must not be shared is exactly the encoder independence above: two test files that
//! lean on one fixture can agree with each other about a grammar that is wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, DeclaredCheck, InconsistentRuling,
    LocalModelActivationService, LocalModelEndpoint, MalformedRuling, MissionDraft,
    MissionMemberDraft, MissionService, OperatorAccountError, OperatorAccountService,
    OperatorSessionPhase, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProjectPolicyService, ProjectRepositoryService, ProjectService, ProposalService,
    ProviderFeature, RepositoryDraft, RepositorySourceKind, ResidentConfigurationService,
    ResidentLifecycleService, RulingDecision, RulingError, RulingService, RulingSupersedes,
    SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    Supervisor, SupervisorError, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement, UnmetPrecondition,
};
use data_encoding::{BASE64, HEXLOWER};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A fabricated Repository Binding. Neither the path nor the commit id names anything that exists.
const REPOSITORY_SOURCE: &str = "/home/operator/projects/sample-tracker";
const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const OTHER_BASE_REVISION: &str = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";
const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

const PROPOSAL_MAGIC: &[u8] = b"beacon.proposal.v1\0";
const RULING_MAGIC: &[u8] = b"beacon.ruling.v1\0";
const APPROVE_REASON: &str = "The declared effects stay inside the Repository and spend nothing.";

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

/// One truthful `beacon.proposal.v1` record, with only the knob this file needs: the summary,
/// which is what makes a second Proposal in the same Mission a different Proposal.
struct ProposalRecord {
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    summary: String,
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

        push_count(&mut out, 1);
        push_string(&mut out, "src/lib.rs");
        push_count(&mut out, 1);
        out.push(0x02);
        push_string(&mut out, "src/lib.rs");
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

/// One `beacon.ruling.v1` record, field by field in ADR 0037's stated order and widths.
#[derive(Debug, Clone)]
struct RulingRecord {
    proposal_digest: String,
    charter_revision: u32,
    policy_revision: u32,
    repository_revision: u32,
    base_revision: String,
    decision_tag: u8,
    reason: String,
    decider_tag: u8,
    supersedes: Option<String>,
    ruled_at_ms: u64,
}

impl RulingRecord {
    fn bytes(&self) -> Vec<u8> {
        let mut out = RULING_MAGIC.to_vec();
        out.extend_from_slice(&HEXLOWER.decode(self.proposal_digest.as_bytes()).unwrap());
        push_string(&mut out, "sha256");
        push_string(&mut out, "beacon.proposal.v1");
        for revision in [
            self.charter_revision,
            self.policy_revision,
            self.repository_revision,
        ] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, &self.base_revision);
        out.push(self.decision_tag);
        push_string(&mut out, &self.reason);
        out.push(self.decider_tag);
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

// ---------------------------------------------------------------------------------------------
// One admitted Mission with one submitted Proposal, built the way an Operator would build it
// ---------------------------------------------------------------------------------------------

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir()
                .join(format!("beacond-ruling-{label}-{}.sqlite3", Uuid::new_v4())),
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
/// Mission, and no Ruling yet.
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
    /// any state, so a store left with triggers missing refuses startup for the wrong reason and a
    /// reconciliation test passes without ever reaching reconciliation.
    fn tamper(&self, statements: &str) {
        let connection = self.store.connect();
        let mut statement = connection
            .prepare(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger' AND tbl_name IN ('rulings', 'ruling_supersessions')
                 ORDER BY name",
            )
            .unwrap();
        let triggers = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(statement);
        assert_eq!(triggers.len(), 4);

        connection
            .execute_batch(
                "DROP TRIGGER rulings_no_update;
                 DROP TRIGGER rulings_no_delete;
                 DROP TRIGGER ruling_supersessions_no_update;
                 DROP TRIGGER ruling_supersessions_no_delete;",
            )
            .unwrap();
        connection.execute_batch(statements).unwrap();
        for trigger in triggers {
            connection.execute_batch(&trigger).unwrap();
        }
    }
}

fn inconsistent(error: &RulingError) -> Option<InconsistentRuling> {
    match error {
        RulingError::Inconsistent(reason) => Some(*reason),
        _ => None,
    }
}

fn malformed(error: &RulingError) -> Option<MalformedRuling> {
    match error {
        RulingError::Malformed(reason) => Some(*reason),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Recording and identity
// ---------------------------------------------------------------------------------------------

#[test]
fn an_owner_records_one_ruling_bound_to_the_exact_proposal_it_named() {
    let fixture = admitted_mission("record");
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");

    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    // The digest names bytes this file built from the published table, not from Beacon's encoder.
    let expected = RulingRecord {
        proposal_digest: proposal_digest.clone(),
        charter_revision: 1,
        policy_revision: 1,
        repository_revision: 1,
        base_revision: BASE_REVISION.to_owned(),
        decision_tag: 0x01,
        reason: APPROVE_REASON.to_owned(),
        decider_tag: 0x01,
        supersedes: None,
        ruled_at_ms: ruling.payload().ruled_at_ms,
    };
    assert_eq!(ruling.canonical_bytes(), expected.bytes().as_slice());
    assert_eq!(ruling.digest().value, expected.digest());
    assert_eq!(ruling.digest().algorithm, "sha256");
    assert_eq!(ruling.digest().canonicalizer, "beacon.ruling.v1");

    // Two envelopes, both explicit: the Ruling's own, and the Proposal's copied whole.
    assert_eq!(ruling.payload().proposal.value, proposal_digest);
    assert_eq!(ruling.payload().proposal.algorithm, "sha256");
    assert_eq!(
        ruling.payload().proposal.canonicalizer,
        "beacon.proposal.v1"
    );
    assert_eq!(ruling.payload().decision, RulingDecision::Approve);
    assert_eq!(ruling.payload().reason, APPROVE_REASON);
    assert_eq!(ruling.payload().supersedes, RulingSupersedes::None);
    assert_eq!(
        ruling.payload().bindings.pinned_base_revision,
        BASE_REVISION
    );

    assert_eq!(fixture.ruling_rows(), 1);
    assert_eq!(fixture.ruling_events(), 1);
    assert_eq!(fixture.rulings.get(&expected.digest()).unwrap(), ruling);
    assert_eq!(
        fixture.rulings.list_for_proposal(&proposal_digest).unwrap(),
        vec![ruling]
    );
}

#[test]
fn the_audit_event_carries_the_exact_bytes_as_padded_base64_and_the_owner_actor() {
    let fixture = admitted_mission("event");
    let proposal_digest = fixture.submit("Add a regression test.");
    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "The declared targets reach outside the Repository.",
            RulingSupersedes::None,
        )
        .unwrap();

    let payload: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'ruling.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let encoded = value["ruling"]["canonicalBytesBase64"].as_str().unwrap();
    assert_eq!(encoded, BASE64.encode(ruling.canonical_bytes()));
    assert_eq!(
        BASE64.decode(encoded.as_bytes()).unwrap(),
        ruling.canonical_bytes()
    );
    assert_eq!(value["ruling"]["actor"], "owner");
    assert_eq!(value["ruling"]["digest"]["value"], ruling.digest().value);
    assert_eq!(
        value["ruling"]["digest"]["canonicalizer"],
        "beacon.ruling.v1"
    );
    assert_eq!(
        value["ruling"]["ruledAtMs"].as_u64().unwrap(),
        ruling.payload().ruled_at_ms
    );
    fixture.restart().unwrap();
}

#[test]
fn recording_a_ruling_creates_no_run_receipt_worktree_or_external_effect() {
    // A Ruling records that a person decided. It is not execution authority, and this is the
    // measurable form of that claim.
    let fixture = admitted_mission("inert");
    let proposal_digest = fixture.submit("Add a regression test.");
    let before_runs = fixture.count("SELECT count(*) FROM runs");
    let before_receipts = fixture.count("SELECT count(*) FROM receipts");
    let before_security = fixture.count("SELECT count(*) FROM security_events");
    let before_events = fixture.count("SELECT count(*) FROM events");

    fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    assert_eq!(fixture.count("SELECT count(*) FROM runs"), before_runs);
    assert_eq!(
        fixture.count("SELECT count(*) FROM receipts"),
        before_receipts
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM security_events"),
        before_security
    );
    // Exactly one new Event, and it is the Ruling's own.
    assert_eq!(
        fixture.count("SELECT count(*) FROM events"),
        before_events + 1
    );
    assert_eq!(fixture.ruling_events(), 1);
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
fn unauthenticated_recording_returns_only_the_account_refusal() {
    // Both the digest and the reason are malformed. An implementation that validated either
    // before authenticating would hand an unauthenticated caller a validation oracle.
    let fixture = admitted_mission("locked");
    fixture.accounts.lock().unwrap();

    let error = fixture
        .rulings
        .rule(
            "not-a-digest",
            RulingDecision::Approve,
            "",
            RulingSupersedes::None,
        )
        .unwrap_err();

    assert!(
        matches!(
            error,
            RulingError::Account(OperatorAccountError::NotAuthenticated)
        ),
        "{error:?}"
    );
    assert!(malformed(&error).is_none());
    assert_eq!(fixture.ruling_rows(), 0);
    assert_eq!(fixture.ruling_events(), 0);
}

#[test]
fn a_malformed_reason_or_digest_writes_neither_row_nor_event() {
    let fixture = admitted_mission("malformed");
    let proposal_digest = fixture.submit("Add a regression test.");

    for (reason, expected) in [
        ("", MalformedRuling::Reason),
        ("   ", MalformedRuling::Reason),
        (" untrimmed", MalformedRuling::Reason),
        ("a NUL\u{0}inside", MalformedRuling::Reason),
    ] {
        let error = fixture
            .rulings
            .rule(
                &proposal_digest,
                RulingDecision::Approve,
                reason,
                RulingSupersedes::None,
            )
            .unwrap_err();
        assert_eq!(malformed(&error), Some(expected), "{reason:?}: {error:?}");
    }
    let over_long = "x".repeat(2_001);
    assert_eq!(
        malformed(
            &fixture
                .rulings
                .rule(
                    &proposal_digest,
                    RulingDecision::Approve,
                    &over_long,
                    RulingSupersedes::None
                )
                .unwrap_err()
        ),
        Some(MalformedRuling::Reason)
    );
    // The boundary itself is accepted, so the refusal is the rule rather than an off-by-one.
    let at_limit = "x".repeat(2_000);
    for (digest, expected) in [
        (
            proposal_digest.to_uppercase(),
            MalformedRuling::ProposalDigestValue,
        ),
        ("deadbeef".to_owned(), MalformedRuling::ProposalDigestValue),
    ] {
        let error = fixture
            .rulings
            .rule(
                &digest,
                RulingDecision::Approve,
                APPROVE_REASON,
                RulingSupersedes::None,
            )
            .unwrap_err();
        assert_eq!(malformed(&error), Some(expected), "{digest}: {error:?}");
    }
    assert_eq!(
        malformed(
            &fixture
                .rulings
                .rule(
                    &proposal_digest,
                    RulingDecision::Approve,
                    APPROVE_REASON,
                    RulingSupersedes::Ruling("not-a-digest".to_owned())
                )
                .unwrap_err()
        ),
        Some(MalformedRuling::SupersedesDigest)
    );

    assert_eq!(fixture.ruling_rows(), 0);
    assert_eq!(fixture.ruling_events(), 0);
    // And the accepted boundary really is accepted.
    fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            &at_limit,
            RulingSupersedes::None,
        )
        .unwrap();
    assert_eq!(fixture.ruling_rows(), 1);
}

// ---------------------------------------------------------------------------------------------
// Binding
// ---------------------------------------------------------------------------------------------

#[test]
fn ruling_on_a_digest_that_names_no_proposal_writes_neither_row_nor_event() {
    let fixture = admitted_mission("unknown-proposal");
    fixture.submit("Add a regression test.");

    let error = fixture
        .rulings
        .rule(
            &"f".repeat(64),
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::ProposalNotFound),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 0);
    assert_eq!(fixture.ruling_events(), 0);
}

#[test]
fn a_proposal_outside_this_services_workspace_is_not_found() {
    // Scope is not a filter applied after the fact: a Ruling service for another Workspace cannot
    // see this Proposal at all, so it cannot bind it.
    let fixture = admitted_mission("other-workspace");
    let proposal_digest = fixture.submit("Add a regression test.");

    let elsewhere = RulingService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    let error = elsewhere
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::ProposalNotFound),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 0);
}

#[test]
fn the_copied_revisions_are_the_proposals_not_the_current_ones() {
    // The wrong answer this kills: copying whatever is current at ruling time, which would make a
    // Ruling silently agree with a policy version its Proposal was never written against -- and
    // would make drift undetectable, because the copies could never disagree with the world.
    let fixture = admitted_mission("copies-not-current");
    let proposal_digest = fixture.submit("Add a regression test.");

    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .publish(fixture.project_id, 1, Some(1), policy_draft(2))
        .unwrap();

    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();

    assert_eq!(ruling.payload().bindings.project_policy_revision, 1);
    // And because the copy is the Proposal's, the drift is visible the moment it is recorded.
    assert_eq!(
        ruling.force().drift(),
        [UnmetPrecondition::ProjectPolicyRevisionMoved]
    );
}

// ---------------------------------------------------------------------------------------------
// Force: drift and supersession, always reported together
// ---------------------------------------------------------------------------------------------

fn ruled(label: &str) -> (Fixture, String, String) {
    let fixture = admitted_mission(label);
    let proposal_digest = fixture.submit("Add a regression test for the invariant checker.");
    let ruling = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap();
    let digest = ruling.digest().value.clone();
    (fixture, proposal_digest, digest)
}

#[test]
fn a_fresh_ruling_reports_neither_drift_nor_supersession() {
    // The control every refusal below is measured against: without it, a force derivation that
    // reported everything as drifted would pass every drift test.
    let (fixture, _, digest) = ruled("in-force");
    let ruling = fixture.rulings.get(&digest).unwrap();
    assert_eq!(ruling.force().drift(), []);
    assert_eq!(ruling.force().superseded_by(), None);
}

#[test]
fn a_moved_charter_is_named_alone_and_the_record_does_not_change() {
    let (fixture, _, digest) = ruled("charter-moved");
    let before = fixture.rulings.get(&digest).unwrap();

    ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .revise_charter(
            fixture.project_id,
            1,
            charter_draft("Prove the local agent office, revised."),
        )
        .unwrap();

    let after = fixture.rulings.get(&digest).unwrap();
    assert_eq!(
        after.force().drift(),
        [UnmetPrecondition::CharterRevisionMoved]
    );
    // Drift is derived, never stored: the decision itself is untouched and still attributable.
    assert_eq!(after.canonical_bytes(), before.canonical_bytes());
    assert_eq!(after.digest(), before.digest());
    assert_eq!(after.payload(), before.payload());
    // And a historical Ruling does not become corrupt merely because the world moved.
    fixture.restart().unwrap();
}

#[test]
fn a_moved_policy_revision_is_named_alone() {
    let (fixture, _, digest) = ruled("policy-moved");
    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .publish(fixture.project_id, 1, Some(1), policy_draft(2))
        .unwrap();

    assert_eq!(
        fixture.rulings.get(&digest).unwrap().force().drift(),
        [UnmetPrecondition::ProjectPolicyRevisionMoved]
    );
}

#[test]
fn a_new_repository_revision_with_the_same_base_reports_only_the_revision_as_moved() {
    // Repository and base are separate facts. A rebind that keeps the same pinned commit must not
    // also claim the base moved -- a second, false diagnosis riding on a true one. This is the
    // shape of the bug round two found in Proposal readiness.
    let (fixture, _, digest) = ruled("repository-moved-same-base");
    ProjectRepositoryService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .bind(
            fixture.project_id,
            1,
            Some(1),
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: "/home/operator/projects/sample-tracker-relocated".to_owned(),
                base_revision: BASE_REVISION.to_owned(),
                declared_checks: declared_checks(),
            },
        )
        .unwrap();

    assert_eq!(
        fixture.rulings.get(&digest).unwrap().force().drift(),
        [UnmetPrecondition::RepositoryRevisionMoved]
    );
}

#[test]
fn a_rebind_that_moves_the_pinned_commit_names_both_facts() {
    let (fixture, _, digest) = ruled("base-moved");
    ProjectRepositoryService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .bind(
            fixture.project_id,
            1,
            Some(1),
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: REPOSITORY_SOURCE.to_owned(),
                base_revision: OTHER_BASE_REVISION.to_owned(),
                declared_checks: declared_checks(),
            },
        )
        .unwrap();

    assert_eq!(
        fixture.rulings.get(&digest).unwrap().force().drift(),
        [
            UnmetPrecondition::RepositoryRevisionMoved,
            UnmetPrecondition::BaseRevisionMoved
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// Supersession
// ---------------------------------------------------------------------------------------------

#[test]
fn a_second_ruling_supersedes_the_first_and_both_remain_readable() {
    let (fixture, proposal_digest, first) = ruled("supersede");

    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "On reflection the declared targets are too broad.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();

    assert_eq!(
        second.payload().supersedes,
        RulingSupersedes::Ruling(first.clone())
    );
    // Both readable, and the first is not rewritten: it is superseded, not erased.
    let first_record = fixture.rulings.get(&first).unwrap();
    assert_eq!(first_record.payload().decision, RulingDecision::Approve);
    assert_eq!(
        first_record.force().superseded_by(),
        Some(second.digest().value.as_str())
    );
    assert_eq!(second.force().superseded_by(), None);

    // The effective Ruling is the LAST link -- not the first, and not the approve.
    let chain = fixture.rulings.list_for_proposal(&proposal_digest).unwrap();
    assert_eq!(chain.len(), 2);
    assert_eq!(chain[0].digest().value, first);
    assert_eq!(chain[1].digest().value, second.digest().value);
    assert_eq!(
        chain.last().unwrap().payload().decision,
        RulingDecision::Refuse
    );
    assert_eq!(fixture.ruling_rows(), 2);
    assert_eq!(fixture.ruling_events(), 2);
    fixture.restart().unwrap();
}

#[test]
fn any_decision_may_follow_any_decision_and_every_link_carries_its_own_reason() {
    // An approve after an approve with a new reason is a legal re-ruling, not a contradiction:
    // the chain is a readable history, not a state machine over the decision.
    let (fixture, proposal_digest, first) = ruled("approve-after-approve");
    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            "Re-approved after the Repository was rebound to the same commit.",
            RulingSupersedes::Ruling(first),
        )
        .unwrap();
    assert_eq!(second.payload().decision, RulingDecision::Approve);
    assert_ne!(second.payload().reason, APPROVE_REASON);
    assert_eq!(
        fixture
            .rulings
            .list_for_proposal(&proposal_digest)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn a_later_ruling_that_cites_nothing_is_refused_as_not_the_tail() {
    // This is also what "identical re-ruling is refused rather than duplicated" looks like from a
    // caller: the same call twice, refused the second time with nothing written.
    let (fixture, proposal_digest, _) = ruled("cites-nothing");

    let error = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::None,
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::NotTheCurrentTail),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 1);
    assert_eq!(fixture.ruling_events(), 1);
}

#[test]
fn a_first_ruling_that_cites_an_unrecorded_digest_is_refused_as_unrecorded() {
    let fixture = admitted_mission("cites-unrecorded");
    let proposal_digest = fixture.submit("Add a regression test.");

    let error = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::Ruling("c".repeat(64)),
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::SupersedesNotRecorded),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 0);
}

#[test]
fn citing_another_proposals_ruling_is_refused_as_that_and_not_as_unrecorded() {
    // Three different mistakes, three different answers. A digest that names nothing, one that
    // names another Proposal's decision, and one that names an already-superseded link of this
    // chain lead a caller to three different corrections.
    let (fixture, _, other_ruling) = ruled("cites-other-proposal");
    let second_proposal = fixture.submit("A second, different Proposal in the same Mission.");

    let error = fixture
        .rulings
        .rule(
            &second_proposal,
            RulingDecision::Approve,
            APPROVE_REASON,
            RulingSupersedes::Ruling(other_ruling),
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::SupersedesOtherProposal),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 1);
}

#[test]
fn citing_a_link_that_is_no_longer_the_tail_is_refused() {
    // Without this the graph forks and "the effective Ruling is the last link" stops having an
    // answer: two Rulings would each claim to supersede the same one.
    let (fixture, proposal_digest, first) = ruled("not-the-tail");
    fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Refused on second thought.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();

    let error = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Approve,
            "A third opinion, citing the first link again.",
            RulingSupersedes::Ruling(first),
        )
        .unwrap_err();

    assert_eq!(
        inconsistent(&error),
        Some(InconsistentRuling::NotTheCurrentTail),
        "{error:?}"
    );
    assert_eq!(fixture.ruling_rows(), 2);
    assert_eq!(fixture.ruling_events(), 2);
}

#[test]
fn a_ruling_can_be_both_superseded_and_drifted_and_says_so() {
    // Force is two facts reported together, so no caller has to learn which one wins. A design
    // that made it an enum would have to choose, and would hide the other.
    let (fixture, proposal_digest, first) = ruled("both-facts");
    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Refused on second thought.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();
    ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .revise_charter(
            fixture.project_id,
            1,
            charter_draft("Prove the local agent office, revised."),
        )
        .unwrap();

    let superseded = fixture.rulings.get(&first).unwrap();
    assert_eq!(
        superseded.force().superseded_by(),
        Some(second.digest().value.as_str())
    );
    assert_eq!(
        superseded.force().drift(),
        [UnmetPrecondition::CharterRevisionMoved]
    );
}

// ---------------------------------------------------------------------------------------------
// Queries and store integrity
// ---------------------------------------------------------------------------------------------

#[test]
fn get_refuses_a_digest_that_names_nothing_and_list_separates_unknown_from_undecided() {
    let fixture = admitted_mission("queries");
    let proposal_digest = fixture.submit("Add a regression test.");

    assert!(matches!(
        fixture.rulings.get(&"a".repeat(64)).unwrap_err(),
        RulingError::NotFound
    ));
    // A recorded Proposal with no decision yet is an empty list...
    assert_eq!(
        fixture.rulings.list_for_proposal(&proposal_digest).unwrap(),
        vec![]
    );
    // ...and a Proposal that was never recorded is a refusal, because "no such Proposal" and "no
    // decision yet" are different facts a caller acts differently on.
    assert_eq!(
        inconsistent(
            &fixture
                .rulings
                .list_for_proposal(&"a".repeat(64))
                .unwrap_err()
        ),
        Some(InconsistentRuling::ProposalNotFound)
    );
}

#[test]
fn ruling_rows_refuse_update_and_delete() {
    let (fixture, _, digest) = ruled("immutable");
    let connection = fixture.store.connect();

    for statement in [
        format!("UPDATE rulings SET reason = 'rewritten' WHERE digest_value = '{digest}'"),
        format!("DELETE FROM rulings WHERE digest_value = '{digest}'"),
    ] {
        let error = connection.execute_batch(&statement).unwrap_err();
        assert!(
            error.to_string().contains("Rulings are immutable"),
            "{error}"
        );
    }
    assert_eq!(fixture.ruling_rows(), 1);
}

#[test]
fn a_stored_ruling_that_does_not_name_its_bytes_is_refused_by_every_read() {
    let (fixture, proposal_digest, digest) = ruled("altered-bytes");
    let mut altered = fixture
        .rulings
        .get(&digest)
        .unwrap()
        .canonical_bytes()
        .to_vec();
    let last = altered.len() - 1;
    altered[last] ^= 0xff;
    fixture.tamper(&format!(
        "UPDATE rulings SET canonical_bytes = x'{}' WHERE digest_value = '{digest}'",
        HEXLOWER.encode(&altered)
    ));

    assert!(matches!(
        fixture.rulings.get(&digest).unwrap_err(),
        RulingError::InvalidStore
    ));
    assert!(matches!(
        fixture
            .rulings
            .list_for_proposal(&proposal_digest)
            .unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn an_indexed_column_that_disagrees_with_the_bytes_is_refused() {
    // The bytes are untouched, so the digest still names them and they still re-encode. Only
    // comparing every indexed column against the parsed value catches a row whose index says one
    // thing and whose record says another -- which is what a query filtering on those columns
    // would believe.
    let (fixture, _, digest) = ruled("column-disagrees");
    fixture.tamper(&format!(
        "UPDATE rulings SET charter_revision = 9 WHERE digest_value = '{digest}'"
    ));

    assert!(matches!(
        fixture.rulings.get(&digest).unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn a_supersession_row_that_disagrees_with_the_bytes_is_refused() {
    // The relation and the byte tag are two recordings of one fact, and ADR 0037 promises they
    // agree. A planted link whose Ruling's bytes say `supersedes = none` is the wrong answer:
    // without this check the chain would show a supersession the decision never recorded.
    let (fixture, proposal_digest, first) = ruled("relation-disagrees");
    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Refused on second thought.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();
    // Both links are real; only the relation is moved, so the bytes of the FIRST Ruling -- which
    // say `none` -- now disagree with a row claiming it cites the second.
    fixture.tamper(&format!(
        "DELETE FROM ruling_supersessions;
         INSERT INTO ruling_supersessions (
             workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
         ) VALUES ('{}', '{proposal_digest}', '{first}', '{}');",
        fixture.workspace_id,
        second.digest().value
    ));

    assert!(matches!(
        fixture.rulings.get(&first).unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn a_forked_chain_has_no_last_link_and_is_refused() {
    // The store's primary key and unique index each forbid one shape of fork, but neither forbids
    // two independent chains under one Proposal. Re-deriving the chain is what catches that, and
    // it is the same function recording uses to find the tail.
    let (fixture, proposal_digest, first) = ruled("forked");
    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Refused on second thought.",
            RulingSupersedes::Ruling(first),
        )
        .unwrap();
    // Remove the only link, leaving two Rulings that each claim to be the head.
    fixture.tamper("DELETE FROM ruling_supersessions;");

    assert!(matches!(
        fixture
            .rulings
            .list_for_proposal(&proposal_digest)
            .unwrap_err(),
        RulingError::InvalidStore
    ));
    // And the second Ruling's own bytes still say it supersedes the first, so reading it directly
    // is refused too: the row and the relation disagree.
    assert!(matches!(
        fixture.rulings.get(&second.digest().value).unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn a_row_wearing_another_rulings_digest_is_refused_by_the_digest_it_claims() {
    // The columns can be made to agree with planted bytes -- every payload field has one -- so
    // `columns_match` alone would accept a whole different decision wearing this digest. Only
    // re-deriving the digest from the stored bytes on every read refuses it.
    let (fixture, _, digest) = ruled("wrong-digest");
    let recorded = fixture.rulings.get(&digest).unwrap();
    let impostor = RulingRecord {
        proposal_digest: recorded.payload().proposal.value.clone(),
        charter_revision: 1,
        policy_revision: 1,
        repository_revision: 1,
        base_revision: BASE_REVISION.to_owned(),
        decision_tag: 0x01,
        // No apostrophe: this reason is interpolated into a SQL literal below, and a test that
        // has to escape its own fixture is a test about escaping.
        reason: "A different decision, wearing a digest it did not produce.".to_owned(),
        decider_tag: 0x01,
        supersedes: None,
        ruled_at_ms: recorded.payload().ruled_at_ms,
    };
    assert_ne!(impostor.digest(), digest);
    fixture.tamper(&format!(
        "UPDATE rulings SET canonical_bytes = x'{}', reason = '{}' WHERE digest_value = '{digest}'",
        HEXLOWER.encode(&impostor.bytes()),
        impostor.reason
    ));

    assert!(matches!(
        fixture.rulings.get(&digest).unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn a_supersession_cycle_has_no_first_link_and_is_refused() {
    // Every Ruling citing a predecessor is storable: the primary key allows one citation each and
    // the unique index allows each to be superseded once, so a two-link cycle satisfies both. What
    // it does not have is a first link, and a chain with no beginning has no last one either.
    let (fixture, proposal_digest, first) = ruled("cycle");
    let second = fixture
        .rulings
        .rule(
            &proposal_digest,
            RulingDecision::Refuse,
            "Refused on second thought.",
            RulingSupersedes::Ruling(first.clone()),
        )
        .unwrap();
    fixture.tamper(&format!(
        "INSERT INTO ruling_supersessions (
             workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
         ) VALUES ('{}', '{proposal_digest}', '{first}', '{}');",
        fixture.workspace_id,
        second.digest().value
    ));

    assert!(matches!(
        fixture
            .rulings
            .list_for_proposal(&proposal_digest)
            .unwrap_err(),
        RulingError::InvalidStore
    ));
}

#[test]
fn a_ruling_event_that_does_not_match_its_row_refuses_startup() {
    // The per-Event half of reconciliation, which lands with its writer. The whole-table half --
    // one Event per Ruling, no orphan, the chain re-derived at startup -- is the next slice.
    let (fixture, _, _) = ruled("event-mismatch");
    let payload: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'ruling.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
    let encoded = value["ruling"]["canonicalBytesBase64"].as_str().unwrap();
    let mut bytes = BASE64.decode(encoded.as_bytes()).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    value["ruling"]["canonicalBytesBase64"] = BASE64.encode(&bytes).into();
    fixture
        .store
        .connect()
        .execute(
            "UPDATE events SET payload_json = ?1 WHERE kind = 'ruling.recorded'",
            [value.to_string()],
        )
        .unwrap();

    let error = fixture.restart().unwrap_err();
    assert!(
        matches!(error, SupervisorError::RulingEventMismatch(_)),
        "{error:?} / {error}"
    );
}
