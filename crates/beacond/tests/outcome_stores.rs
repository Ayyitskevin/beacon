//! Outcome slice 2-A-1: the spend and classification stores, and their verified readers.
//!
//! The fixture chain down to `mission_draft` is `tests/mission.rs`'s, reused verbatim: both stores
//! have a foreign key to `missions`, so a row cannot exist without the admission chain that
//! produces one. Copied rather than shared because the two suites must be able to disagree about
//! what a Mission is -- a shared fixture is how two tests agree about something wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, ClassificationStoreError, DiagnosticStatus,
    LocalModelActivationService, LocalModelEndpoint, MeasuredSpendV1, MissionDraft,
    MissionImpactClassificationV1, MissionMemberDraft, MissionService, OperatorAccountService,
    OperatorSessionPhase, PredecessorFault, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProjectPolicyService, ProjectService, ProjectView, ProviderFeature, ResidentConfiguration,
    ResidentConfigurationService, ResidentLifecycleService, SecretString, SpendStoreError,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule,
    TeamLeadSelectionPolicy, TeamMemberDraft, TeamService, TeamView, TrackRecordEvidenceKind,
    TrackRecordRequirement, open_store_connection, verified_classification_for_citation,
    verified_classification_within, verified_spend_for_citation, verified_spend_within,
};
use rusqlite::{Connection, params};
use serde_json::json;
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
            "beacond-mission-{label}-{}.sqlite3",
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
// Store fixtures: the records this slice stores, and the rows that carry them
// ---------------------------------------------------------------------------------------------

use beacond::{
    ClassificationActor, ClassificationBasis, ClassificationRecordError, ClassificationSupersedes,
    ImpactLevel, MeasuredSpendService, MissionImpactService, PolicyRevisionRef, SpendLine,
    SpendMeasurement, SpendOrigin, SpendRecordError, SpendSupersedes, SpendTotals,
};

struct Stored {
    store: TestStore,
    accounts: Arc<OperatorAccountService>,
    mission_id: Uuid,
    project_id: Uuid,
    charter_revision: u32,
    /// The last Event sequence this Workspace had when the fixture was built -- the honest end of a
    /// coverage window, and the value the writer now checks a record against.
    coverage_to: u64,
}

/// A Workspace with one admitted Mission, a published Project Policy, and nothing else.
fn stored(label: &str) -> Stored {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 2);
    let mission = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .admit(mission_draft(&prepared))
        .expect("the Mission is admitted");
    let coverage_to = Connection::open(&store.path)
        .unwrap()
        .query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            params![store.workspace_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).unwrap())
        .unwrap();
    Stored {
        mission_id: mission.id,
        project_id: prepared.project.id,
        charter_revision: prepared.project.charter.revision,
        accounts,
        coverage_to,
        store,
    }
}

impl Stored {
    fn connection(&self) -> Connection {
        Connection::open(&self.store.path).unwrap()
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
                    self.project_id.to_string(),
                    self.charter_revision
                ],
            )
            .unwrap();
        1
    }

    fn spend_record(&self, spend_id: Uuid, version: u32) -> MeasuredSpendV1 {
        MeasuredSpendV1 {
            workspace_id: self.store.workspace_id,
            mission_id: self.mission_id,
            spend_id,
            version,
            origin: SpendOrigin::DaemonLedger,
            measured_at_ms: 1_788_400_000_000,
            coverage_from_event_seq: 1,
            coverage_to_event_seq: self.coverage_to,
            measurement: SpendMeasurement::Measured(SpendTotals {
                counted_units: 412,
                lines: vec![SpendLine {
                    unit: "USD".to_owned(),
                    amount_minor: 123_456,
                    minor_units_exponent: 2,
                }],
            }),
            supersedes: SpendSupersedes::None,
        }
    }

    /// Insert a spend row. `bytes` and the projections are supplied separately on purpose: the decoy
    /// tests need a row whose columns and bytes disagree, and no write path would let them build one.
    #[allow(clippy::too_many_arguments)]
    fn insert_spend(&self, bytes: &[u8], digest: &str, spend_id: Uuid, version: u32, origin: &str) {
        self.connection()
            .execute(
                "INSERT INTO measured_spend (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, spend_id, mission_id, version, origin, measurement,
                     coverage_from_event_seq, coverage_to_event_seq, measured_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.spend.v1', ?2, ?3, ?4, ?5, ?6, ?7, 'measured',
                           1, ?8, 1788400000000)",
                params![
                    self.store.workspace_id.to_string(),
                    digest,
                    bytes,
                    spend_id.to_string(),
                    self.mission_id.to_string(),
                    version,
                    origin,
                    i64::try_from(self.coverage_to).unwrap(),
                ],
            )
            .unwrap();
    }

    fn classification_record(
        &self,
        id: Uuid,
        policy_revision: u32,
    ) -> MissionImpactClassificationV1 {
        MissionImpactClassificationV1 {
            workspace_id: self.store.workspace_id,
            mission_id: self.mission_id,
            classification_id: id,
            version: 1,
            impact: ImpactLevel::High,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: self.project_id,
                policy_revision,
                charter_revision: self.charter_revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes: ClassificationSupersedes::None,
        }
    }

    fn insert_classification(
        &self,
        bytes: &[u8],
        digest: &str,
        id: Uuid,
        version: u32,
        policy_revision: u32,
        impact_column: &str,
    ) {
        self.connection()
            .execute(
                "INSERT INTO mission_impact_classifications (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, classification_id, mission_id, project_id, version, impact,
                     basis, policy_revision, charter_revision, classified_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                           'operator_declared', ?9, ?10, 1788400000000)",
                params![
                    self.store.workspace_id.to_string(),
                    digest,
                    bytes,
                    id.to_string(),
                    self.mission_id.to_string(),
                    self.project_id.to_string(),
                    version,
                    impact_column,
                    policy_revision,
                    self.charter_revision,
                ],
            )
            .unwrap();
    }
}

// ---------------------------------------------------------------------------------------------
// One test per slice-2 bullet
// ---------------------------------------------------------------------------------------------

/// The verified reader proves the stored bytes are the identity the row is keyed by, and that they
/// round-trip. Bytes that decode but do not re-encode to themselves are refused.
#[test]
fn the_verified_reader_requires_an_exact_byte_round_trip_of_the_stored_slice() {
    let stored = stored("round-trip");
    let spend_id = Uuid::new_v4();
    let record = stored.spend_record(spend_id, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    stored.insert_spend(&bytes, &digest, spend_id, 1, "daemon_ledger");

    let connection = stored.connection();
    let found = verified_spend_within(&connection, stored.store.workspace_id, &digest)
        .expect("verifies")
        .expect("stored");
    assert_eq!(found.payload(), &record);
    assert_eq!(found.canonical_bytes(), bytes.as_slice());
    assert_eq!(found.digest().canonicalizer, "beacon.spend.v1");

    // A row whose bytes do not hash to its key is refused, not returned.
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    let other = Uuid::new_v4();
    stored.insert_spend(&tampered, &"a".repeat(64), other, 2, "daemon_ledger");
    assert_eq!(
        verified_spend_within(&connection, stored.store.workspace_id, &"a".repeat(64))
            .expect_err("refuses"),
        SpendStoreError::DigestMismatch
    );
}

/// A projection column that disagrees with the canonical bytes refuses the read. The bytes win, and
/// the reader does not quietly prefer them -- it stops, because a store in that state is corrupt.
#[test]
fn the_verified_reader_refuses_a_row_whose_columns_disagree_with_its_bytes() {
    let stored = stored("decoy-columns");
    let spend_id = Uuid::new_v4();
    let record = stored.spend_record(spend_id, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;

    // The bytes say `daemon_ledger` and version 1; the columns say otherwise. Only a direct insert
    // can build this, which is why the decoy tests do not go through a write path.
    stored.insert_spend(&bytes, &digest, spend_id, 7, "daemon_ledger");
    let connection = stored.connection();
    assert_eq!(
        verified_spend_within(&connection, stored.store.workspace_id, &digest)
            .expect_err("refuses"),
        SpendStoreError::ColumnsDisagree
    );
}

/// The same property on the classification's own projection, where the decoy is the one that
/// matters most: an `impact` column reading `ordinary` over bytes that say `high` is exactly the
/// disagreement that would make an Outcome omit a mandatory verification appendix.
#[test]
fn the_classification_reader_refuses_a_row_whose_impact_column_disagrees_with_its_bytes() {
    let stored = stored("decoy-impact");
    let revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let classification = stored.classification_record(id, revision);
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    stored.insert_classification(&bytes, &digest, id, 1, revision, "ordinary");
    let connection = stored.connection();
    assert_eq!(
        verified_classification_within(&connection, stored.store.workspace_id, &digest)
            .expect_err("refuses"),
        ClassificationStoreError::ColumnsDisagree
    );
}

/// The classification reader's twin of the spend round-trip test.
///
/// It exists because the twin did not: opus's O2 deleted `mission_impact.rs`'s digest recomputation
/// and all fourteen tests stayed green, while deleting the identical three lines from `spend.rs` was
/// caught. A guard that is real on one half and untested on the other is one half of a guard.
#[test]
fn the_classification_reader_refuses_a_row_whose_bytes_do_not_hash_to_its_digest() {
    let stored = stored("impact-digest");
    let revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let classification = stored.classification_record(id, revision);
    let bytes = classification.canonical_bytes().unwrap();

    // The bytes decode, re-encode to themselves, and agree with every projection column. The only
    // thing wrong with this row is the identity it is filed under -- which is the one fault this
    // check exists for, and the reason it has to be its own recomputation rather than a column read.
    let wrong = "b".repeat(64);
    assert_ne!(wrong, classification.digest().unwrap().value);
    stored.insert_classification(&bytes, &wrong, id, 1, revision, "high");

    let connection = stored.connection();
    assert_eq!(
        verified_classification_within(&connection, stored.store.workspace_id, &wrong)
            .expect_err("refuses"),
        ClassificationStoreError::DigestMismatch
    );
}

/// A citation whose reference identity and content identity disagree is refused (R15).
#[test]
fn a_citation_whose_id_and_digest_disagree_is_refused() {
    let stored = stored("citation");
    let spend_id = Uuid::new_v4();
    let record = stored.spend_record(spend_id, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    stored.insert_spend(&bytes, &digest, spend_id, 1, "daemon_ledger");
    let connection = stored.connection();
    let workspace = stored.store.workspace_id;

    // The honest citation resolves.
    assert!(
        verified_spend_for_citation(&connection, workspace, spend_id, 1, &digest)
            .expect("verifies")
            .is_some()
    );
    // A different id against the same digest is refused, not silently resolved by digest alone.
    assert_eq!(
        verified_spend_for_citation(&connection, workspace, Uuid::new_v4(), 1, &digest)
            .expect_err("refuses"),
        SpendStoreError::CitedIdentityMismatch
    );
    // So is a different version.
    assert_eq!(
        verified_spend_for_citation(&connection, workspace, spend_id, 2, &digest)
            .expect_err("refuses"),
        SpendStoreError::CitedIdentityMismatch
    );
}

/// The cited identity is read from the decoded bytes, never from the projection column.
///
/// The row's `spend_id` column is made to agree with a citation that the *bytes* disagree with. A
/// reader that compared against the column would accept it.
#[test]
fn the_cited_id_is_read_from_the_decoded_bytes_not_the_projection_column() {
    let stored = stored("cited-from-bytes");
    let in_bytes = Uuid::new_v4();
    let in_column = Uuid::new_v4();
    assert_ne!(in_bytes, in_column);
    let record = stored.spend_record(in_bytes, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    stored.insert_spend(&bytes, &digest, in_column, 1, "daemon_ledger");

    let connection = stored.connection();
    // The columns disagree with the bytes, so the read stops before the citation check -- one
    // refusal for one fault, and the corrupt store is named rather than the citation.
    assert_eq!(
        verified_spend_for_citation(
            &connection,
            stored.store.workspace_id,
            in_column,
            1,
            &digest
        )
        .expect_err("refuses"),
        SpendStoreError::ColumnsDisagree
    );
}

/// Both stores refuse UPDATE and DELETE. Corrections are new records, never edits.
#[test]
fn store_rows_are_immutable_under_update_and_delete() {
    let stored = stored("immutable");
    let spend_id = Uuid::new_v4();
    let record = stored.spend_record(spend_id, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    stored.insert_spend(&bytes, &digest, spend_id, 1, "daemon_ledger");

    let revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let classification = stored.classification_record(id, revision);
    let impact_bytes = classification.canonical_bytes().unwrap();
    let impact_digest = classification.digest().unwrap().value;
    stored.insert_classification(&impact_bytes, &impact_digest, id, 1, revision, "high");

    let connection = stored.connection();
    for (statement, subject) in [
        ("UPDATE measured_spend SET version = 9", "spend update"),
        ("DELETE FROM measured_spend", "spend delete"),
        (
            "UPDATE mission_impact_classifications SET impact = 'ordinary'",
            "classification update",
        ),
        (
            "DELETE FROM mission_impact_classifications",
            "classification delete",
        ),
    ] {
        let error = connection
            .execute(statement, [])
            .expect_err(&format!("{subject} must be refused"));
        assert!(
            error.to_string().contains("immutable"),
            "{subject}: {error}"
        );
    }
}

#[test]
fn insert_or_replace_cannot_bypass_the_immutable_store_triggers() {
    let stored = stored("immutable-replace");
    let spend_id = Uuid::new_v4();
    let record = stored.spend_record(spend_id, 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    stored.insert_spend(&bytes, &digest, spend_id, 1, "daemon_ledger");

    // Through the crate's opener, the way every production connection is opened (R21): the
    // pragma it sets is what makes REPLACE meet the delete trigger. A bare `Connection::open`
    // here would measure SQLite's default, not Beacon's.
    let connection = open_store_connection(&stored.store.path).unwrap();
    let result = connection.execute(
        "INSERT OR REPLACE INTO measured_spend (
             workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
             canonical_bytes, spend_id, mission_id, version, origin, measurement,
             coverage_from_event_seq, coverage_to_event_seq, measured_at_ms
         ) VALUES (?1, 'sha256', 'beacon.spend.v1', ?2, ?3, ?4, ?5, 1,
                   'daemon_ledger', 'broken', 1, ?6, 1788400000000)",
        params![
            stored.store.workspace_id.to_string(),
            digest,
            bytes,
            spend_id.to_string(),
            stored.mission_id.to_string(),
            i64::try_from(stored.coverage_to).unwrap(),
        ],
    );

    assert!(
        result.is_err(),
        "INSERT OR REPLACE bypassed UPDATE/DELETE immutability triggers: {result:?}"
    );
}

/// R13.7's re-check, on the reader rather than in a caller.
#[test]
fn the_classification_reader_rechecks_the_copied_bit() {
    let stored = stored("recheck");
    let revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let classification = stored.classification_record(id, revision);
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    stored.insert_classification(&bytes, &digest, id, 1, revision, "high");

    let connection = stored.connection();
    let found = verified_classification_for_citation(
        &connection,
        stored.store.workspace_id,
        id,
        1,
        &digest,
    )
    .expect("verifies")
    .expect("stored");

    // The record says high-impact, so a manifest that copied `true` agrees and one that copied
    // `false` is refused by name. The copy and the check are one call, so a caller cannot do the
    // first without the second.
    assert!(found.payload().appendix_is_mandatory());
    found.recheck_copied_bit(true).expect("agrees");
    assert_eq!(
        found.recheck_copied_bit(false).expect_err("disagrees"),
        ClassificationStoreError::CopiedBitDisagrees
    );
}

// ---------------------------------------------------------------------------------------------
// The write path
// ---------------------------------------------------------------------------------------------

impl Stored {
    fn spend_service(&self) -> MeasuredSpendService {
        MeasuredSpendService::new(Arc::clone(&self.accounts), self.store.workspace_id)
    }

    fn impact_service(&self) -> MissionImpactService {
        MissionImpactService::new(Arc::clone(&self.accounts), self.store.workspace_id)
    }
}

#[test]
fn a_classification_cannot_bind_one_mission_to_another_projects_policy() {
    let stored = stored("classification-cross-project-policy");
    let own_policy = stored.policy_revision();
    assert_eq!(own_policy, 1);

    let other_project =
        ProjectService::new(Arc::clone(&stored.accounts), stored.store.workspace_id)
            .create(ProjectDraft {
                name: "Unrelated Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Prove policy scoping.".to_owned(),
                    desired_outcomes: vec!["Stay unrelated".to_owned()],
                    constraints: vec!["Do not borrow another Mission".to_owned()],
                    prohibitions: vec!["No cross-Project binding".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
    stored
        .connection()
        .execute(
            r#"INSERT INTO project_policy_revisions (
                 workspace_id, project_id, revision, charter_revision,
                 required_capabilities_json, availability_rule, track_record_requirements_json,
                 conflict_rule, ambiguity_rule, authored_by, authored_at_ms
             ) VALUES (?1, ?2, 1, 1, '["work.coordinate_mission"]',
                       'current_team_eligibility', '[{"evidenceKind":"completed_mission"}]',
                       'require_no_declared_conflict', 'require_operator_decision', 'operator', 0)"#,
            params![
                stored.store.workspace_id.to_string(),
                other_project.id.to_string(),
            ],
        )
        .unwrap();

    let mut classification = stored.classification_record(Uuid::new_v4(), 1);
    classification.policy.project_id = other_project.id;
    classification.policy.charter_revision = other_project.charter.revision;
    let result = stored
        .impact_service()
        .classify(&classification.canonical_bytes().unwrap());
    assert!(
        result.is_err(),
        "classification for Mission {} accepted unrelated Project {} policy: {result:?}",
        stored.mission_id,
        other_project.id,
    );
}

/// The submit contract: a caller supplies bytes, and the digest names exactly those bytes.
///
/// There is no `record(payload)` and no `record(payload, digest)` to test, because neither exists --
/// that is the property, and it is enforced by the signature rather than by a test. What is
/// testable is the other half: bytes that are not exactly a canonical record are refused, so the
/// stored digest can never name something the caller did not send.
#[test]
fn record_requires_the_exact_canonical_bytes_of_the_supplied_slice() {
    let stored = stored("record-round-trip");
    let service = stored.spend_service();
    let record = stored.spend_record(Uuid::new_v4(), 1);
    let bytes = record.canonical_bytes().unwrap();

    let recorded = service
        .record(&bytes)
        .expect("canonical bytes are recorded");
    assert_eq!(recorded.canonical_bytes(), bytes.as_slice());
    assert_eq!(recorded.payload(), &record);
    assert_eq!(
        recorded.digest().value,
        record.digest().unwrap().value,
        "the stored digest must name the bytes the caller supplied"
    );

    // One byte appended is a different slice, and it is refused rather than truncated to the part
    // that parses -- a decoder that stopped early would store a digest over bytes nobody sent.
    let mut extended = bytes.clone();
    extended.push(0x00);
    assert!(matches!(
        service.record(&extended),
        Err(SpendRecordError::Malformed(_))
    ));
    let mut truncated = bytes.clone();
    truncated.pop();
    assert!(matches!(
        service.record(&truncated),
        Err(SpendRecordError::Malformed(_))
    ));
}

/// One record, one Event, and the Event carries identity rather than money.
#[test]
fn recording_writes_one_event_that_cites_the_record_and_states_no_amount() {
    let stored = stored("record-event");
    let record = stored.spend_record(Uuid::new_v4(), 1);
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;

    stored.spend_service().record(&bytes).expect("recorded");

    let connection = stored.connection();
    let payloads: Vec<String> = connection
        .prepare("SELECT payload_json FROM events WHERE kind = 'spend.measured'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(payloads.len(), 1, "one record must write exactly one Event");
    let payload = &payloads[0];
    assert!(payload.contains(&digest), "the Event must cite the digest");
    assert!(payload.contains(&record.spend_id.to_string()));
    // The amount lives in the bytes, which the Event carries verbatim, and nowhere else: a second
    // copy in a queryable field is a number that can disagree with the ledger it came from.
    assert!(
        !payload.contains("123456") && !payload.contains("amount"),
        "the Event restated an amount: {payload}"
    );
}

/// Daemon and operator records are two ladders over one Mission, and neither is merged into the
/// other or into a single number.
#[test]
fn daemon_and_operator_records_coexist_and_never_merge() {
    let stored = stored("two-origins");
    let service = stored.spend_service();
    let mut daemon = stored.spend_record(Uuid::new_v4(), 1);
    let mut operator = stored.spend_record(Uuid::new_v4(), 1);
    operator.origin = SpendOrigin::OperatorDeclared;
    daemon.measurement = SpendMeasurement::Measured(SpendTotals {
        counted_units: 1,
        lines: vec![SpendLine {
            unit: "USD".to_owned(),
            amount_minor: 100,
            minor_units_exponent: 2,
        }],
    });

    service.record(&daemon.canonical_bytes().unwrap()).unwrap();
    service
        .record(&operator.canonical_bytes().unwrap())
        .unwrap();

    let both = service
        .for_mission(stored.mission_id)
        .expect("both read back");
    assert_eq!(both.len(), 2);
    assert_eq!(both[0].payload().origin, SpendOrigin::DaemonLedger);
    assert_eq!(both[1].payload().origin, SpendOrigin::OperatorDeclared);
    assert_ne!(both[0].digest().value, both[1].digest().value);
    // Two records at version 1, and the service offers no way to add them: `for_mission` returns
    // the ladders, and there is no `total`, so a caller that wants one number has to say which
    // origin it means.
}

/// A version that is already taken is refused, not merged and not overwritten.
#[test]
fn a_second_record_at_a_taken_identity_is_refused_rather_than_updated() {
    let stored = stored("identity-taken");
    let service = stored.spend_service();
    let first = stored.spend_record(Uuid::new_v4(), 1);
    service.record(&first.canonical_bytes().unwrap()).unwrap();

    // Same (mission, origin, version), different record.
    let mut second = stored.spend_record(Uuid::new_v4(), 1);
    second.measured_at_ms += 1;
    assert!(matches!(
        service.record(&second.canonical_bytes().unwrap()),
        Err(SpendRecordError::IdentityTaken)
    ));

    // The next version is accepted, so the refusal is about the identity and not about the Mission.
    let mut next = stored.spend_record(Uuid::new_v4(), 2);
    next.measured_at_ms += 1;
    service
        .record(&next.canonical_bytes().unwrap())
        .expect("v2 records");
}

/// A record addressed at another Workspace is refused on the identity in its own bytes.
#[test]
fn recording_refuses_a_record_addressed_at_another_workspace() {
    let stored = stored("cross-workspace-write");
    let mut record = stored.spend_record(Uuid::new_v4(), 1);
    record.workspace_id = Uuid::new_v4();

    assert!(matches!(
        stored
            .spend_service()
            .record(&record.canonical_bytes().unwrap()),
        Err(SpendRecordError::CrossWorkspace)
    ));
    let connection = stored.connection();
    let rows: i64 = connection
        .query_row("SELECT count(*) FROM measured_spend", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "a refused record must leave no row");
}

/// A coverage window is a claim about a stretch of the store's own history, and the store is the
/// only thing that can confirm it.
///
/// The check is at RECORD time on purpose: `max(sequence)` is a fact then, and at read time the same
/// question cannot be answered without re-deriving a past fact from a world that has moved on. So a
/// window past the last Event is refused before the row exists, by name -- rather than stored and
/// rendered as verified by an Outcome that has no way to know better.
#[test]
fn a_coverage_window_past_the_stores_last_event_is_refused_at_record_time() {
    let stored = stored("coverage");
    let service = stored.spend_service();

    let mut beyond = stored.spend_record(Uuid::new_v4(), 1);
    beyond.coverage_to_event_seq = stored.coverage_to + 1;
    match service.record(&beyond.canonical_bytes().unwrap()) {
        Err(SpendRecordError::CoverageBeyondStore {
            claimed_to_event_seq,
            last_event_seq,
        }) => {
            assert_eq!(claimed_to_event_seq, stored.coverage_to + 1);
            assert_eq!(last_event_seq, stored.coverage_to);
        }
        other => panic!("a window past the last Event must be refused by name: {other:?}"),
    }
    let rows: i64 = stored
        .connection()
        .query_row("SELECT count(*) FROM measured_spend", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0, "a refused window must leave no row");

    // A window that ends exactly at the last Event is the boundary case and is accepted -- the
    // check is `>`, not `>=`, because a measurement may cover everything that has happened.
    let at_the_edge = stored.spend_record(Uuid::new_v4(), 1);
    assert_eq!(at_the_edge.coverage_to_event_seq, stored.coverage_to);
    service
        .record(&at_the_edge.canonical_bytes().unwrap())
        .expect("a window ending at the last Event is honest");

    // ...and no record can cover its own Event. The window is checked before the row is written, so
    // the `spend.measured` Event that record produced lands strictly after the window it declares.
    // (A LATER record may cover it -- by then it is history like any other Event, which is the
    // honest reading and the reason this is a record-time check rather than a rule about which
    // Events may be covered.)
    let after: i64 = stored
        .connection()
        .query_row(
            "SELECT max(sequence) FROM events WHERE workspace_id = ?1",
            params![stored.store.workspace_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        u64::try_from(after).unwrap(),
        at_the_edge.coverage_to_event_seq + 1,
        "the record's own Event must fall outside the window it declared"
    );
}

/// The classification's composite foreign key answers "is there such a Policy revision?", so it is
/// not a check a caller can forget -- and the refusal names the Policy revision rather than
/// collapsing into the uniqueness refusal beside it.
#[test]
fn classifying_under_a_policy_revision_that_does_not_exist_is_refused_by_name() {
    let stored = stored("policy-fk");
    let service = stored.impact_service();
    let revision = stored.policy_revision();

    let absent = stored.classification_record(Uuid::new_v4(), revision + 7);
    assert!(matches!(
        service.classify(&absent.canonical_bytes().unwrap()),
        Err(ClassificationRecordError::PolicyRevisionNotFound)
    ));

    let present = stored.classification_record(Uuid::new_v4(), revision);
    let recorded = service
        .classify(&present.canonical_bytes().unwrap())
        .expect("the published revision is accepted");
    assert_eq!(recorded.payload(), &present);

    // And the ladder is what a caller gets: no `is_high_impact` and no "current", because which
    // record is current depends on the event cursor being resolved at.
    let history = service.history(stored.mission_id).expect("history reads");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].digest().value, recorded.digest().value);
}

/// Cross-Workspace refusal is keyed on the record's own Workspace, not on the query predicate.
///
/// The `WHERE workspace_id = ?` in the SQL is a filter, and a filter only ever says which rows were
/// *looked at*. What makes a Workspace boundary a boundary is the check after the decode: the
/// Workspace the bytes name must be the Workspace being read. This plants a row filed under one
/// Workspace whose bytes are addressed at another -- the shape a restore, a merge or a bad backfill
/// produces -- and both readers refuse it.
#[test]
fn the_readers_refuse_a_record_whose_bytes_name_another_workspace() {
    let spend_side = stored("cross-workspace-spend");
    let elsewhere = Uuid::new_v4();
    spend_side
        .connection()
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
            params![elsewhere.to_string()],
        )
        .unwrap();
    let spend_id = Uuid::new_v4();
    let mut record = spend_side.spend_record(spend_id, 1);
    record.workspace_id = elsewhere;
    let bytes = record.canonical_bytes().unwrap();
    let digest = record.digest().unwrap().value;
    // Filed under this Workspace -- the row the FK allows -- with bytes addressed at the other.
    spend_side.insert_spend(&bytes, &digest, spend_id, 1, "daemon_ledger");

    let connection = spend_side.connection();
    assert_eq!(
        verified_spend_within(&connection, spend_side.store.workspace_id, &digest)
            .expect_err("refuses"),
        SpendStoreError::CrossWorkspace
    );
    // And it is not readable from the Workspace its bytes name either: that one has no such row.
    assert_eq!(
        verified_spend_within(&connection, elsewhere, &digest).expect("no row"),
        None
    );

    let impact_side = stored("cross-workspace-impact");
    let other = Uuid::new_v4();
    impact_side
        .connection()
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
            params![other.to_string()],
        )
        .unwrap();
    let revision = impact_side.policy_revision();
    let id = Uuid::new_v4();
    let mut classification = impact_side.classification_record(id, revision);
    classification.workspace_id = other;
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    impact_side.insert_classification(&bytes, &digest, id, 1, revision, "high");

    let connection = impact_side.connection();
    assert_eq!(
        verified_classification_within(&connection, impact_side.store.workspace_id, &digest)
            .expect_err("refuses"),
        ClassificationStoreError::CrossWorkspace
    );
}

#[test]
fn a_store_holding_spend_or_classification_events_restarts() {
    // Regression for the gap kimi's 0027 kind-list tripwire exposed: 0025 admitted
    // 'spend.measured' and 'mission.impact_classified' into the events CHECK without teaching
    // the replay vocabulary to parse them, so a store holding either Event refused every
    // subsequent startup with InvalidEventKind. These arms still verify nothing beyond
    // admission; startup verification for the two tables is the spend lane's follow-on.
    let store = TestStore::new("spend-event-restart");
    let connection = Connection::open(&store.path).unwrap();
    for (kind, marker) in [
        ("spend.measured", "{\"marker\":\"spend\"}"),
        ("mission.impact_classified", "{\"marker\":\"impact\"}"),
    ] {
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, ?2, ?3, 1)",
                params![store.workspace_id.to_string(), kind, marker],
            )
            .unwrap();
    }
    drop(connection);
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("a store holding spend and classification Events must restart");
}

/// The SCHEMA half of sol1's #2 (R20.1): a classification naming a Project that does not own its
/// Mission cannot be STORED, whatever wrote it.
///
/// The Command refuses it and so does the reader -- and both are code, which a restore, an older
/// binary or a hand-run `INSERT` walks straight past. This row satisfies every other foreign key
/// the table declares: the Mission exists, the other Project exists, and its Policy revision 1
/// exists, so nothing but a key binding the Mission to its Project can refuse it. The insert goes
/// through a raw connection with foreign keys ON, which is what a restore looks like.
#[test]
fn the_schema_refuses_a_classification_whose_project_does_not_own_its_mission() {
    let stored = stored("classification-cross-project-schema");
    stored.policy_revision();
    let other_project =
        ProjectService::new(Arc::clone(&stored.accounts), stored.store.workspace_id)
            .create(ProjectDraft {
                name: "Unrelated Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Prove the schema's own scoping.".to_owned(),
                    desired_outcomes: vec!["Stay unrelated".to_owned()],
                    constraints: vec!["Do not borrow another Mission".to_owned()],
                    prohibitions: vec!["No cross-Project binding".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
    let connection = stored.connection();
    connection
        .execute(
            r#"INSERT INTO project_policy_revisions (
                 workspace_id, project_id, revision, charter_revision,
                 required_capabilities_json, availability_rule, track_record_requirements_json,
                 conflict_rule, ambiguity_rule, authored_by, authored_at_ms
             ) VALUES (?1, ?2, 1, ?3, '["work.coordinate_mission"]',
                       'current_team_eligibility', '[{"evidenceKind":"completed_mission"}]',
                       'require_no_declared_conflict', 'require_operator_decision', 'operator', 0)"#,
            params![
                stored.store.workspace_id.to_string(),
                other_project.id.to_string(),
                other_project.charter.revision,
            ],
        )
        .expect("the other Project's Policy revision is real");

    let id = Uuid::new_v4();
    let mut classification = stored.classification_record(id, 1);
    classification.policy.project_id = other_project.id;
    classification.policy.charter_revision = other_project.charter.revision;
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    let error = connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1, 'high',
                       'operator_declared', 1, ?7, ?8)",
            params![
                stored.store.workspace_id.to_string(),
                digest,
                bytes,
                id.to_string(),
                stored.mission_id.to_string(),
                other_project.id.to_string(),
                other_project.charter.revision,
                i64::try_from(classification.classified_at_ms).unwrap(),
            ],
        )
        .expect_err(
            "a Mission of one Project classified under another Project's row must not store",
        );
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "the row was refused, but not by a key binding the Mission to its Project: {error}"
    );

    // The control: the SAME row under the Project that does own the Mission stores.
    let honest = stored.classification_record(Uuid::new_v4(), 1);
    let honest_bytes = honest.canonical_bytes().unwrap();
    let honest_digest = honest.digest().unwrap().value;
    connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1, 'high',
                       'operator_declared', 1, ?7, ?8)",
            params![
                stored.store.workspace_id.to_string(),
                honest_digest,
                honest_bytes,
                honest.classification_id.to_string(),
                stored.mission_id.to_string(),
                stored.project_id.to_string(),
                stored.charter_revision,
                i64::try_from(honest.classified_at_ms).unwrap(),
            ],
        )
        .expect("the Mission's own Project stores");
}

/// The read-side half of #2, which the probe could not reach: `classify` refuses a cross-Project
/// policy at the Command, so the reader's identical check had no test that failed when it was
/// deleted (my own mutant table found it). This row is written PAST the Command -- the shape a
/// restore or an older binary produces -- with every projection column agreeing with its bytes, so
/// the only thing left that can refuse it is the ownership check on the read.
///
/// Since 0032 the row is unrepresentable through an ordinary connection, so the arrangement turns
/// FOREIGN KEYS OFF to write it. That is not a workaround: it is the scenario. A store restored
/// from a backup taken before 0032, or written by a binary that predates it, holds exactly this
/// row with the key never consulted -- and the reader is what refuses it then. The schema is the
/// braces and this check is the belt; keeping both is the point, so this test keeps proving the
/// belt still works when the braces were never fastened.
#[test]
fn a_stored_classification_citing_another_projects_policy_is_refused_on_read() {
    let stored = stored("classification-cross-project-read");
    stored.policy_revision();
    let other_project =
        ProjectService::new(Arc::clone(&stored.accounts), stored.store.workspace_id)
            .create(ProjectDraft {
                name: "Unrelated Project".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Prove read-side policy scoping.".to_owned(),
                    desired_outcomes: vec!["Stay unrelated".to_owned()],
                    constraints: vec!["Do not borrow another Mission".to_owned()],
                    prohibitions: vec!["No cross-Project binding".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
    let connection = stored.connection();
    // The store a restore leaves behind: the key that would refuse this row is not consulted.
    connection
        .pragma_update(None, "foreign_keys", "OFF")
        .unwrap();
    connection
        .execute(
            r#"INSERT INTO project_policy_revisions (
                 workspace_id, project_id, revision, charter_revision,
                 required_capabilities_json, availability_rule, track_record_requirements_json,
                 conflict_rule, ambiguity_rule, authored_by, authored_at_ms
             ) VALUES (?1, ?2, 1, 1, '["work.coordinate_mission"]',
                       'current_team_eligibility', '[{"evidenceKind":"completed_mission"}]',
                       'require_no_declared_conflict', 'require_operator_decision', 'operator', 0)"#,
            params![
                stored.store.workspace_id.to_string(),
                other_project.id.to_string(),
            ],
        )
        .unwrap();

    let id = Uuid::new_v4();
    let mut classification = stored.classification_record(id, 1);
    classification.policy.project_id = other_project.id;
    classification.policy.charter_revision = other_project.charter.revision;
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    // Every column derived from these bytes -- including the OTHER Project -- so the row is honest
    // about itself and both foreign keys hold; nothing but the ownership check can refuse it.
    connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1, 'high',
                       'operator_declared', 1, ?7, ?8)",
            params![
                stored.store.workspace_id.to_string(),
                digest,
                bytes,
                id.to_string(),
                stored.mission_id.to_string(),
                other_project.id.to_string(),
                other_project.charter.revision,
                i64::try_from(classification.classified_at_ms).unwrap(),
            ],
        )
        .expect("with the key unfastened the row lands, which is what a restore does");

    assert_eq!(
        verified_classification_within(&connection, stored.store.workspace_id, &digest).expect_err(
            "a classification under a Project that does not own its Mission must not read"
        ),
        ClassificationStoreError::PolicyProjectIsNotTheMissions
    );
}

#[test]
fn supersession_cannot_name_a_record_from_another_workspace() {
    let other = stored("supersession-other-workspace");
    let other_spend = other.spend_record(Uuid::new_v4(), 1);
    let other_spend =
        MeasuredSpendService::new(Arc::clone(&other.accounts), other.store.workspace_id)
            .record(&other_spend.canonical_bytes().unwrap())
            .expect("the other Workspace stores its predecessor spend");
    let other_revision = other.policy_revision();
    let other_classification = other.classification_record(Uuid::new_v4(), other_revision);
    let other_classification =
        MissionImpactService::new(Arc::clone(&other.accounts), other.store.workspace_id)
            .classify(&other_classification.canonical_bytes().unwrap())
            .expect("the other Workspace stores its predecessor classification");

    let local = stored("supersession-local-workspace");
    let spend_service =
        MeasuredSpendService::new(Arc::clone(&local.accounts), local.store.workspace_id);
    let base_spend = local.spend_record(Uuid::new_v4(), 1);
    spend_service
        .record(&base_spend.canonical_bytes().unwrap())
        .expect("the local spend ladder starts at version one");
    let mut successor_spend = local.spend_record(Uuid::new_v4(), 2);
    successor_spend.supersedes = SpendSupersedes::Record(other_spend.digest().value.clone());
    let spend_result = spend_service.record(&successor_spend.canonical_bytes().unwrap());

    let local_revision = local.policy_revision();
    let impact_service =
        MissionImpactService::new(Arc::clone(&local.accounts), local.store.workspace_id);
    let base_classification = local.classification_record(Uuid::new_v4(), local_revision);
    impact_service
        .classify(&base_classification.canonical_bytes().unwrap())
        .expect("the local classification ladder starts at version one");
    let mut successor_classification = local.classification_record(Uuid::new_v4(), local_revision);
    successor_classification.version = 2;
    successor_classification.supersedes =
        ClassificationSupersedes::Record(other_classification.digest().value.clone());
    let classification_result =
        impact_service.classify(&successor_classification.canonical_bytes().unwrap());

    assert!(
        spend_result.is_err() && classification_result.is_err(),
        "foreign predecessors were accepted: spend={spend_result:?}, classification={classification_result:?}"
    );
}

impl Stored {
    /// A second admitted Mission in this Workspace, cloned from the first so that only its
    /// identity differs: the same Project, Charter and Team, so every foreign key a record under it
    /// needs already holds.
    fn second_mission(&self) -> Uuid {
        let id = Uuid::new_v4();
        self.connection()
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
                    self.store.workspace_id.to_string(),
                    self.mission_id.to_string(),
                ],
            )
            .unwrap();
        id
    }

    /// A spend record whose BYTES name another, real Workspace of this store, filed under this
    /// Workspace's columns (the row the foreign key allows) -- the Elsewhere pattern the reader's
    /// own cross-Workspace test uses. What a `supersedes` link to it must see is the bytes.
    fn spend_of_another_workspace(&self) -> String {
        let elsewhere = Uuid::new_v4();
        self.connection()
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
                params![elsewhere.to_string()],
            )
            .unwrap();
        let spend_id = Uuid::new_v4();
        let mut record = self.spend_record(spend_id, 1);
        record.workspace_id = elsewhere;
        record.origin = SpendOrigin::OperatorDeclared;
        let bytes = record.canonical_bytes().unwrap();
        let digest = record.digest().unwrap().value;
        self.insert_spend(&bytes, &digest, spend_id, 1, "operator_declared");
        digest
    }

    /// The classification twin of `spend_of_another_workspace`, at the given policy revision.
    fn classification_of_another_workspace(&self, policy_revision: u32) -> String {
        let elsewhere = Uuid::new_v4();
        self.connection()
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Elsewhere', 1)",
                params![elsewhere.to_string()],
            )
            .unwrap();
        let id = Uuid::new_v4();
        let mut record = self.classification_record(id, policy_revision);
        record.workspace_id = elsewhere;
        let bytes = record.canonical_bytes().unwrap();
        let digest = record.digest().unwrap().value;
        self.insert_classification(&bytes, &digest, id, 1, policy_revision, "high");
        digest
    }
}

/// The three ways a predecessor can be wrong, at the Command: no such record at all, a real
/// record of another Mission in this Workspace, and a record whose own bytes name another
/// Workspace of this store. Each is refused by its own name, on both ladders.
///
/// sol1's probe above stores its "other Workspace" in a separate store FILE, so the fault it
/// reaches is `NotStored`; the Workspace arm needs a second Workspace in the SAME store, which is
/// what the two `*_of_another_workspace` rows are.
#[test]
fn a_predecessor_that_is_unknown_or_about_another_mission_is_refused_by_name() {
    let stored = stored("predecessor-unknown-or-other-mission");
    let second = stored.second_mission();
    let spend_service = stored.spend_service();
    let impact_service = stored.impact_service();
    let revision = stored.policy_revision();

    let mut other_missions_spend = stored.spend_record(Uuid::new_v4(), 1);
    other_missions_spend.mission_id = second;
    let other_missions_spend = spend_service
        .record(&other_missions_spend.canonical_bytes().unwrap())
        .expect("the second Mission's own spend record stores");
    let mut other_missions_classification = stored.classification_record(Uuid::new_v4(), revision);
    other_missions_classification.mission_id = second;
    let other_missions_classification = impact_service
        .classify(&other_missions_classification.canonical_bytes().unwrap())
        .expect("the second Mission's own classification stores");

    let unknown = "f".repeat(64);
    let foreign_spend = stored.spend_of_another_workspace();
    let foreign_classification = stored.classification_of_another_workspace(revision);
    for (label, predecessor, expected) in [
        ("unknown", unknown.clone(), PredecessorFault::NotStored),
        (
            "another Mission's",
            other_missions_spend.digest().value.clone(),
            PredecessorFault::ForAnotherMission,
        ),
        (
            "another Workspace's",
            foreign_spend,
            PredecessorFault::InAnotherWorkspace,
        ),
    ] {
        let mut successor = stored.spend_record(Uuid::new_v4(), 2);
        successor.supersedes = SpendSupersedes::Record(predecessor.clone());
        let error = spend_service
            .record(&successor.canonical_bytes().unwrap())
            .expect_err(&format!(
                "a spend record naming {label} predecessor is refused"
            ));
        assert!(
            matches!(&error, SpendRecordError::Predecessor { digest, fault } if *digest == predecessor && *fault == expected),
            "spend, {label} predecessor: {error:?}"
        );
    }
    for (label, predecessor, expected) in [
        ("unknown", unknown, PredecessorFault::NotStored),
        (
            "another Mission's",
            other_missions_classification.digest().value.clone(),
            PredecessorFault::ForAnotherMission,
        ),
        (
            "another Workspace's",
            foreign_classification,
            PredecessorFault::InAnotherWorkspace,
        ),
    ] {
        let mut successor = stored.classification_record(Uuid::new_v4(), revision);
        successor.version = 2;
        successor.supersedes = ClassificationSupersedes::Record(predecessor.clone());
        let error = impact_service
            .classify(&successor.canonical_bytes().unwrap())
            .expect_err(&format!(
                "a classification naming {label} predecessor is refused"
            ));
        assert!(
            matches!(&error, ClassificationRecordError::Predecessor { digest, fault } if *digest == predecessor && *fault == expected),
            "classification, {label} predecessor: {error:?}"
        );
    }
}

/// The read-side half, in the R20.3 shape: rows written PAST the Command, every column agreeing
/// with bytes whose `supersedes` names a predecessor that is (a) in another Workspace, (b) not
/// stored at all, (c) a record of another Mission, or (d) stored under this digest with bytes
/// that no longer hash to it. Each is refused by the verified reader with the predecessor's own
/// fault, so a restore or an older binary cannot put a foreign history back.
#[test]
fn the_verified_readers_refuse_a_stored_successor_by_its_predecessors_own_bytes() {
    let local = stored("predecessor-reader-local-workspace");
    let revision = local.policy_revision();
    let foreign_spend = local.spend_of_another_workspace();
    let foreign_classification = local.classification_of_another_workspace(revision);
    let second = local.second_mission();
    let other_missions_spend = local
        .spend_service()
        .record(&{
            let mut record = local.spend_record(Uuid::new_v4(), 1);
            record.mission_id = second;
            record.canonical_bytes().unwrap()
        })
        .expect("the second Mission's own spend record stores");
    // (d) A predecessor row keyed by a real digest whose bytes were altered after the fact: one
    // byte of the stored slice flipped, the digest column left as it was.
    let corrupt_id = Uuid::new_v4();
    let corrupt = local.spend_record(corrupt_id, 1);
    let corrupt_digest = corrupt.digest().unwrap().value;
    let mut corrupt_bytes = corrupt.canonical_bytes().unwrap();
    let last = corrupt_bytes.len() - 1;
    corrupt_bytes[last] ^= 0x01;
    local.insert_spend(
        &corrupt_bytes,
        &corrupt_digest,
        corrupt_id,
        1,
        "daemon_ledger",
    );

    let connection = local.connection();
    for (label, predecessor, expected) in [
        (
            "another Workspace's",
            foreign_spend,
            PredecessorFault::InAnotherWorkspace,
        ),
        ("unknown", "e".repeat(64), PredecessorFault::NotStored),
        (
            "another Mission's",
            other_missions_spend.digest().value.clone(),
            PredecessorFault::ForAnotherMission,
        ),
        (
            "corrupt",
            corrupt_digest.clone(),
            PredecessorFault::Unverifiable("digest_mismatch".to_owned()),
        ),
    ] {
        let id = Uuid::new_v4();
        let mut successor = local.spend_record(id, 2);
        successor.supersedes = SpendSupersedes::Record(predecessor.clone());
        let bytes = successor.canonical_bytes().unwrap();
        let digest = successor.digest().unwrap().value;
        local.insert_spend(&bytes, &digest, id, 2, "daemon_ledger");
        assert_eq!(
            verified_spend_within(&connection, local.store.workspace_id, &digest).expect_err(
                &format!("a stored spend successor of {label} predecessor is refused on read")
            ),
            SpendStoreError::Predecessor {
                digest: predecessor,
                fault: expected
            },
            "spend, {label} predecessor"
        );
        // The spend ladder is (workspace, mission, origin, version)-unique; make room for the next.
        connection
            .execute_batch("DROP TRIGGER measured_spend_no_delete;")
            .unwrap();
        connection
            .execute(
                "DELETE FROM measured_spend WHERE digest_value = ?1",
                [&digest],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER measured_spend_no_delete BEFORE DELETE ON measured_spend
                 BEGIN SELECT RAISE(ABORT, 'Measured spend records are immutable'); END;",
            )
            .unwrap();
    }

    for (label, predecessor, expected) in [
        (
            "another Workspace's",
            foreign_classification,
            PredecessorFault::InAnotherWorkspace,
        ),
        ("unknown", "d".repeat(64), PredecessorFault::NotStored),
    ] {
        let id = Uuid::new_v4();
        let mut successor = local.classification_record(id, revision);
        successor.version = 2;
        successor.supersedes = ClassificationSupersedes::Record(predecessor.clone());
        let bytes = successor.canonical_bytes().unwrap();
        let digest = successor.digest().unwrap().value;
        local.insert_classification(&bytes, &digest, id, 2, revision, "high");
        assert_eq!(
            verified_classification_within(&connection, local.store.workspace_id, &digest)
                .expect_err(&format!(
                    "a stored classification successor of {label} predecessor is refused on read"
                )),
            ClassificationStoreError::Predecessor {
                digest: predecessor,
                fault: expected
            },
            "classification, {label} predecessor"
        );
        connection
            .execute_batch("DROP TRIGGER mission_impact_classifications_no_delete;")
            .unwrap();
        connection
            .execute(
                "DELETE FROM mission_impact_classifications WHERE digest_value = ?1",
                [&digest],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TRIGGER mission_impact_classifications_no_delete
                 BEFORE DELETE ON mission_impact_classifications
                 BEGIN SELECT RAISE(ABORT, 'Mission impact classifications are immutable'); END;",
            )
            .unwrap();
    }
}

#[test]
fn classification_refuses_a_charter_revision_the_cited_policy_does_not_have() {
    let stored = stored("classification-charter-mismatch");
    let policy_revision = stored.policy_revision();
    let mut classification = stored.classification_record(Uuid::new_v4(), policy_revision);
    classification.policy.charter_revision = stored.charter_revision + 1;

    let result = MissionImpactService::new(Arc::clone(&stored.accounts), stored.store.workspace_id)
        .classify(&classification.canonical_bytes().unwrap());

    assert!(
        result.is_err(),
        "classification accepted a Charter revision absent from its Policy: {result:?}"
    );
}

/// The read-side half of #12, in the R20.3 shape: a classification row written PAST the Command,
/// its `charter_revision` column agreeing with bytes that cite a Charter revision the Policy
/// revision's own row does not record. The foreign key holds (it covers the Policy revision, not
/// the Charter) and every column matches its bytes, so nothing but the comparison with the Policy's
/// own record can refuse it.
#[test]
fn a_stored_classification_citing_a_charter_the_policy_never_had_is_refused_on_read() {
    let stored = stored("classification-charter-read");
    let policy_revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let mut classification = stored.classification_record(id, policy_revision);
    classification.policy.charter_revision = stored.charter_revision + 1;
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    let connection = stored.connection();
    connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1, 'high',
                       'operator_declared', ?7, ?8, 1788400000000)",
            params![
                stored.store.workspace_id.to_string(),
                digest,
                bytes,
                id.to_string(),
                stored.mission_id.to_string(),
                stored.project_id.to_string(),
                policy_revision,
                stored.charter_revision + 1,
            ],
        )
        .expect("the row is storable: the foreign key does not cover the Charter revision");

    assert_eq!(
        verified_classification_within(&connection, stored.store.workspace_id, &digest)
            .expect_err("a Charter revision the cited Policy never had must not read"),
        ClassificationStoreError::PolicyRevisionHasAnotherCharter
    );
}

/// kimi's P1 on #12: every fixture had Policy revision 1 under Charter revision 1, so a reader that
/// compared the copied Charter with the Policy's `revision` NUMBER instead of its `charter_revision`
/// column stayed GREEN. Two revisions of one Policy under one Charter tell the columns apart: at
/// Policy revision 2 the copied Charter is still 1 (accepted), and a copied Charter of 2 -- equal to
/// the revision number, unequal to the column -- is refused by name.
#[test]
fn the_charter_check_reads_the_policys_charter_column_not_its_revision_number() {
    let stored = stored("classification-charter-column");
    stored.policy_revision();
    stored
        .connection()
        .execute(
            "INSERT INTO project_policy_revisions (
                 workspace_id, project_id, revision, charter_revision,
                 required_capabilities_json, availability_rule, track_record_requirements_json,
                 conflict_rule, ambiguity_rule, authored_by, authored_at_ms
             ) VALUES (?1, ?2, 2, ?3, '[\"work.coordinate_mission\"]',
                       'current_team_eligibility', '[{\"evidenceKind\":\"completed_mission\"}]',
                       'require_no_declared_conflict', 'require_operator_decision', 'operator', 1)",
            params![
                stored.store.workspace_id.to_string(),
                stored.project_id.to_string(),
                stored.charter_revision,
            ],
        )
        .expect("a second Policy revision under the same Charter");
    let service = stored.impact_service();

    // Policy revision 2, Charter revision 1: the column says 1, so this is accepted -- and a reader
    // comparing against the revision number (2) would wrongly refuse it.
    let honest = stored.classification_record(Uuid::new_v4(), 2);
    assert_eq!(honest.policy.charter_revision, stored.charter_revision);
    service
        .classify(&honest.canonical_bytes().unwrap())
        .expect("Policy revision 2 records Charter revision 1, and the record copies 1");

    // Policy revision 2, copied Charter revision 2: equal to the revision number, not to the column.
    let mut lying = stored.classification_record(Uuid::new_v4(), 2);
    lying.version = 2;
    lying.policy.charter_revision = 2;
    let error = service
        .classify(&lying.canonical_bytes().unwrap())
        .expect_err("a copied Charter revision the Policy revision does not record is refused");
    assert!(
        matches!(
            error,
            ClassificationRecordError::PolicyRevisionHasAnotherCharter
        ),
        "{error:?}"
    );
}

/// 0032 rebuilds a table that already holds rows, so the migration is only correct if a store that
/// crossed it keeps them: same bytes, same digest, still readable through the verified reader, and
/// startup Ready afterwards.
///
/// The store is rolled BACK to 0025's classification shape and stamped 30 -- the shape and version
/// a store written before this migration actually has -- and then started, so the ladder applies
/// 0032 for real rather than a test re-running its SQL by hand.
#[test]
fn a_store_with_classifications_crosses_0032_and_keeps_them() {
    let stored = stored("classification-across-0032");
    // Published through its own service, not seeded: this test restarts the whole store, and
    // startup re-verifies Project Policy against its Events -- a hand-written row fails that.
    let revision =
        ProjectPolicyService::new(Arc::clone(&stored.accounts), stored.store.workspace_id)
            .publish(
                stored.project_id,
                stored.charter_revision,
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
            .expect("the Project Policy publishes")
            .revision;
    let recorded = stored
        .impact_service()
        .classify(
            &stored
                .classification_record(Uuid::new_v4(), revision)
                .canonical_bytes()
                .unwrap(),
        )
        .expect("a classification is recorded before the migration");
    let digest = recorded.digest().value.clone();
    let bytes = recorded.canonical_bytes().to_vec();

    // Back to the pre-0032 shape, from 0025's own text -- a rollback that retyped the DDL would be
    // testing the test. Stamped 31, the version a store arrives at from the migration below this
    // one: any other number describes a schema the store does not have.
    let migration_0025 = include_str!("../migrations/0025_spend_and_impact.sql");
    let classification_objects = &migration_0025[migration_0025
        .find("CREATE TABLE mission_impact_classifications (")
        .unwrap()
        ..migration_0025.find("UPDATE schema_meta").unwrap()];
    let connection = stored.connection();
    connection
        .execute_batch(
            "CREATE TEMP TABLE classifications_snapshot AS
                 SELECT * FROM mission_impact_classifications;
             DROP TRIGGER mission_impact_classifications_no_update;
             DROP TRIGGER mission_impact_classifications_no_delete;
             DROP TABLE mission_impact_classifications;",
        )
        .unwrap();
    connection.execute_batch(classification_objects).unwrap();
    connection
        .execute_batch(
            "INSERT INTO mission_impact_classifications
                 SELECT * FROM classifications_snapshot;
             UPDATE schema_meta SET version = 31 WHERE singleton = 1;",
        )
        .unwrap();
    drop(connection);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: stored.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("a store written before 0032 migrates forward");
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
    assert_eq!(report.diagnostics.schema_version, 33);

    let connection = stored.connection();
    let carried: Vec<u8> = connection
        .query_row(
            "SELECT canonical_bytes FROM mission_impact_classifications WHERE digest_value = ?1",
            [&digest],
            |row| row.get(0),
        )
        .expect("the row crossed the rebuild");
    assert_eq!(carried, bytes, "the rebuild changed the stored bytes");
    let reread = verified_classification_within(&connection, stored.store.workspace_id, &digest)
        .expect("the carried row still verifies")
        .expect("the carried row is still there");
    assert_eq!(reread.canonical_bytes(), bytes.as_slice());

    // And the key the migration exists for is fastened on the rebuilt table.
    let error = connection
        .execute(
            "INSERT INTO mission_impact_classifications (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, classification_id, mission_id, project_id, version, impact,
                 basis, policy_revision, charter_revision, classified_at_ms
             ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 2, 'high',
                       'operator_declared', 1, ?7, 1788400000000)",
            params![
                stored.store.workspace_id.to_string(),
                "b".repeat(64),
                bytes,
                Uuid::new_v4().to_string(),
                stored.mission_id.to_string(),
                Uuid::new_v4().to_string(),
                stored.charter_revision,
            ],
        )
        .expect_err("a Project that owns no Mission cannot be named after the migration");
    assert!(
        error.to_string().contains("FOREIGN KEY constraint failed"),
        "{error}"
    );
}
