//! Outcome Document slice 2-B: the verified collector's skeleton.
//!
//! The tests are §7 slice 2's names, restricted to the ones the plan marks buildable against `main`
//! today. Sources whose owning reader does not exist are typed refusals here, never fixtures, so a
//! test that appears to collect a spend record cannot exist yet.

use std::path::PathBuf;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, CollectionRequest, Collector, CollectorRefusal, MissingReader,
    OUTCOME_RECEIPT_CHECKSUM_LABEL, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectService, SecretString, StartupConfig, Supervisor,
};
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "beacond-outcome-collector-{label}-{}.sqlite3",
            Uuid::new_v4()
        ));
        Self { path }
    }

    fn connection(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

struct Fixture {
    workspace_id: Uuid,
    projects: ProjectService,
    store: TestStore,
}

fn enrolled(label: &str) -> Fixture {
    let store = TestStore::new(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
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
    Fixture {
        workspace_id: report.workspace.id,
        projects: ProjectService::new(Arc::clone(&accounts), report.workspace.id),
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

/// §7 slice 2, for the readers that exist. The collector goes to the owning module and reports what
/// it said -- it never answers from a row it read itself.
#[test]
fn collector_reads_each_source_through_its_owning_verified_reader() {
    let fixture = enrolled("readers");
    let connection = fixture.store.connection();
    let collector = Collector::new(&connection, fixture.workspace_id);
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
    ] {
        match collector.collect(&request).expect_err("nothing is stored") {
            CollectorRefusal::Absent { kind, id } => {
                assert_eq!(kind, label);
                assert_eq!(id, absent);
            }
            other => panic!("{label}: expected the owning reader to answer, got {other}"),
        }
    }

    // The three readers that do not exist are typed refusals, never stubs that return a value. A
    // stub is indistinguishable from a verified read at the call site, which is the whole reason
    // this module exists.
    for (probe, expected) in [
        (
            collector.spend().expect_err("no reader"),
            MissingReader::MeasuredSpend,
        ),
        (
            collector.classification().expect_err("no reader"),
            MissingReader::MissionImpactClassification,
        ),
        (
            collector.artifact().expect_err("no reader"),
            MissingReader::Artifact,
        ),
        (
            collector.completion().expect_err("no reader"),
            MissingReader::MissionCompletion,
        ),
    ] {
        assert_eq!(probe, CollectorRefusal::NotYetAvailable(expected));
    }
}
