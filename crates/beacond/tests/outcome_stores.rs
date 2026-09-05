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
    AutonomyProfile, BeaconCapability, ClassificationStoreError, LocalModelActivationService,
    LocalModelEndpoint, MeasuredSpendV1, MissionDraft, MissionImpactClassificationV1,
    MissionMemberDraft, MissionService, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectService, ProjectView, ProviderFeature,
    ResidentConfiguration, ResidentConfigurationService, ResidentLifecycleService, SecretString,
    SpendStoreError, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    Supervisor, TeamDraft, TeamMemberDraft, TeamService, TeamView,
    verified_classification_for_citation, verified_classification_within,
    verified_spend_for_citation, verified_spend_within,
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
        policy_revision: u32,
        impact_column: &str,
    ) {
        self.connection()
            .execute(
                "INSERT INTO mission_impact_classifications (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, classification_id, mission_id, project_id, version, impact,
                     basis, policy_revision, charter_revision, classified_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.mission-impact.v1', ?2, ?3, ?4, ?5, ?6, 1, ?7,
                           'operator_declared', ?8, ?9, 1788400000000)",
                params![
                    self.store.workspace_id.to_string(),
                    digest,
                    bytes,
                    id.to_string(),
                    self.mission_id.to_string(),
                    self.project_id.to_string(),
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
    stored.insert_classification(&bytes, &digest, id, revision, "ordinary");
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
    stored.insert_classification(&bytes, &wrong, id, revision, "high");

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
    stored.insert_classification(&impact_bytes, &impact_digest, id, revision, "high");

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

/// R13.7's re-check, on the reader rather than in a caller.
#[test]
fn the_classification_reader_rechecks_the_copied_bit() {
    let stored = stored("recheck");
    let revision = stored.policy_revision();
    let id = Uuid::new_v4();
    let classification = stored.classification_record(id, revision);
    let bytes = classification.canonical_bytes().unwrap();
    let digest = classification.digest().unwrap().value;
    stored.insert_classification(&bytes, &digest, id, revision, "high");

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

    let other_project = ProjectService::new(
        Arc::clone(&stored.accounts),
        stored.store.workspace_id,
    )
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
    impact_side.insert_classification(&bytes, &digest, id, revision, "high");

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
