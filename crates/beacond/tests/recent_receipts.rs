//! Recent Receipts for one Workspace, across every Run kind.
//!
//! Each test is named for the wrong answer it fails against. Two of them exist
//! because the desktop's version of this reading was held twice in review for
//! exactly these defects: a bound the caller could raise, and a count taken
//! from a different window than the rows beside it.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, ExecutionReceipt, MAX_RECENT_RUN_WINDOW, OperatorAccountService,
    OperatorSessionPhase, PreflightReceipt, ProjectCharterDraft, ProjectDraft, ProjectService,
    RecentReceipt, RecentReceiptError, RecentReceiptOutcome, RecentReceiptService, SecretString,
    StartupConfig, Supervisor,
};
use rusqlite::{Connection, params};
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-recent-receipts-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
        }
    }

    fn connection(&self) -> Connection {
        let connection = Connection::open(&self.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection
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
    workspace_id: Uuid,
    projects: ProjectService,
    recent: RecentReceiptService,
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
        recent: RecentReceiptService::new(accounts, report.workspace.id),
        store,
    }
}

impl Fixture {
    fn project(&self, name: &str) -> Uuid {
        let project = self
            .projects
            .create(ProjectDraft {
                name: name.to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Build a trustworthy local software product.".to_owned(),
                    desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not deploy publicly".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
        // Runs reference a bound repository revision. The write paths are
        // covered by their own suites; this one is about the read, so the row
        // is seeded rather than produced by Git.
        self.store
            .connection()
            .execute(
                "INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind, source,
                     base_revision, declared_checks_json, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 1, 'local_path', '/private/beacon/repo',
                           '0123456789012345678901234567890123456789',
                           '[{\"name\":\"unit\"}]', 'operator', 0)",
                params![self.workspace_id.to_string(), project.id.to_string()],
            )
            .unwrap();
        project.id
    }

    /// One `check_preflight` Run and a valid Receipt whose body is sized by the
    /// caller. Returns how many bytes were stored, so a test can own its own
    /// threshold instead of trusting a constant elsewhere in the file.
    fn run_with_body(&self, project_id: Uuid, started_at_ms: i64, padding: usize) -> usize {
        let run_id = Uuid::new_v4();
        let connection = self.store.connection();
        connection
            .execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'verified', 0, ?4, ?5)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                    started_at_ms,
                    started_at_ms + 1
                ],
            )
            .unwrap();
        let body = receipt_body(
            self.workspace_id,
            project_id,
            run_id,
            "check_preflight",
            1,
            "verified",
            started_at_ms,
            padding,
        );
        connection
            .execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, 'verified', ?3, ?4)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    body,
                    started_at_ms + 1
                ],
            )
            .unwrap();
        body.len()
    }

    /// One Run, and optionally its Receipt, written straight to the store.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        project_id: Uuid,
        kind: &str,
        state: &str,
        started_at_ms: i64,
        receipt: Option<(i64, &str)>,
    ) -> Uuid {
        let run_id = Uuid::new_v4();
        let connection = self.store.connection();
        let ended = if state == "preflighting" {
            None
        } else {
            Some(started_at_ms + 1)
        };
        connection
            .execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, 0, ?6, ?7)",
                params![
                    self.workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string(),
                    kind,
                    state,
                    started_at_ms,
                    ended
                ],
            )
            .unwrap();
        if let Some((version, outcome)) = receipt {
            connection
                .execute(
                    "INSERT INTO receipts (
                         workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        self.workspace_id.to_string(),
                        run_id.to_string(),
                        version,
                        outcome,
                        // A VALID, LARGE body -- not `{}`.
                        //
                        // A malformed body as positive test data would make
                        // "this Query tolerates an unreadable Receipt" an
                        // interface promise, the opposite of R48's owner gate.
                        // Large, because the public row must remain bounded even
                        // though its owner now reads the complete stored body.
                        large_receipt_body(
                            self.workspace_id,
                            project_id,
                            run_id,
                            kind,
                            version,
                            outcome,
                            started_at_ms
                        ),
                        started_at_ms + 1
                    ],
                )
                .unwrap();
        }
        run_id
    }
}

/// A Receipt body that its own versioned decoder accepts, padded to roughly
/// 64 KiB through a field that legitimately holds many entries.
///
/// Validity is asserted here rather than assumed: each body is deserialized
/// into the real type before it is stored, so a fixture cannot drift into being
/// the malformed data this test set is not allowed to rely on.
/// The body is chosen by the owning Run kind. Tuple tests may put a different
/// version or outcome in the SQL projection; the body stays independently valid
/// so the owning read, not malformed fixture data, finds the disagreement.
fn large_receipt_body(
    workspace_id: Uuid,
    project_id: Uuid,
    run_id: Uuid,
    kind: &str,
    version: i64,
    outcome: &str,
    started_at_ms: i64,
) -> String {
    // ~600 entries of ~100 bytes: a real Receipt field, many times over.
    receipt_body(
        workspace_id,
        project_id,
        run_id,
        kind,
        version,
        outcome,
        started_at_ms,
        600,
    )
}

/// The same body at a caller-chosen size, so the invariance the Revised design
/// requires can be MEASURED rather than stated: the returned row must not
/// change as this grows.
#[allow(
    clippy::too_many_arguments,
    reason = "a Receipt fixture names every persisted identity and projection independently"
)]
fn receipt_body(
    workspace_id: Uuid,
    project_id: Uuid,
    run_id: Uuid,
    kind: &str,
    version: i64,
    outcome: &str,
    started_at_ms: i64,
    padding_entries: usize,
) -> String {
    let padding: Vec<String> = (0..padding_entries)
        .map(|index| {
            format!(
                "step {index} was not performed, and this Receipt says so rather than implying it"
            )
        })
        .collect();
    let preflight_outcome = if kind == "declared_checks" {
        "source_kind_unsupported"
    } else {
        match outcome {
            "verified"
            | "source_kind_unsupported"
            | "binding_stale_for_charter"
            | "repository_unreadable"
            | "base_revision_unreachable"
            | "declared_checks_changed"
            | "working_directory_escapes"
            | "sandbox_unavailable"
            | "preflight_failed"
            | "preflight_timed_out" => outcome,
            _ => "verified",
        }
    };
    let preflight = serde_json::json!({
        "version": 1,
        "runId": run_id,
        "workspaceId": workspace_id,
        "projectId": project_id,
        "repositoryRevision": 1,
        "charterRevision": 1,
        "outcome": preflight_outcome,
        "sourceKind": "local_path",
        "source": "/private/beacon/repo",
        "baseRevision": "0123456789012345678901234567890123456789",
        "resolvedChecks": [],
        "sandboxVersion": "landlock-v1",
        "policyCharterRevision": 1,
        "policyRepositoryRevision": 1,
        "startedAtMs": started_at_ms,
        "endedAtMs": started_at_ms + 1,
        "notVerified": padding,
        "unavailable": {
            "proposalDigest": { "kind": "absent", "reason": "no Proposal surface exists" },
            "rulingDigest": { "kind": "absent", "reason": "no Proposal surface exists" },
            "resultingRevision": { "kind": "absent", "reason": "preflight runs nothing" },
            "filesystemEffects": { "kind": "absent", "reason": "preflight runs nothing" },
            "diffDigest": { "kind": "absent", "reason": "preflight runs nothing" },
            "modelInvocations": { "kind": "absent", "reason": "preflight infers nothing" },
            "costUsdMicros": { "kind": "absent", "reason": "preflight infers nothing" }
        }
    });
    if kind == "check_preflight" {
        let value: PreflightReceipt =
            serde_json::from_value(preflight.clone()).expect("a valid v1 preflight Receipt");
        assert_eq!(value.version, 1);
        return serde_json::to_string(&preflight).unwrap();
    }
    let summary = if outcome == "not_admissible" {
        outcome
    } else {
        "not_admissible"
    };
    let not_verified = vec![
        "repository source support for local execution",
        "repository readability",
        "base revision presence in the repository object store",
        "base revision reachability from a ref (object presence verified only)",
        "Declared Check working-directory containment",
        "sandbox enforcement",
        "worktree creation (preflight did not admit execution)",
        "every Declared Check (preflight did not admit execution, so no check ran)",
    ];
    let execution = serde_json::json!({
        "version": version,
        "runId": run_id,
        "workspaceId": workspace_id,
        "projectId": project_id,
        "summary": summary,
        "perCheckBounds": {
            "wallClockMs": 300000u64,
            "cpuSeconds": 240u64,
            "addressSpaceBytes": 2147483648u64,
            "fileSizeBytes": 67108864u64,
            "outputBytes": 1048576u64
        },
        "admittedBy": preflight,
        "checks": [],
        "worktree": null,
        "startedAtMs": started_at_ms,
        "endedAtMs": started_at_ms + 1,
        "notVerified": not_verified
    });
    let value: ExecutionReceipt =
        serde_json::from_value(execution.clone()).expect("a valid v2 execution Receipt");
    assert_eq!(value.version, 2);
    serde_json::to_string(&execution).unwrap()
}

#[test]
fn mixed_kinds_are_ordered_by_one_key_not_grouped_by_kind() {
    // The wrong answer: return preflight Receipts and execution Receipts as two
    // runs of rows, or order them by when the Receipt was written rather than
    // by the Run's own clock.
    let fixture = enrolled("mixed-order");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((1, "verified")),
    );
    fixture.run(
        project,
        "declared_checks",
        "refused",
        20,
        Some((2, "not_admissible")),
    );
    fixture.run(
        project,
        "check_preflight",
        "refused",
        30,
        Some((1, "preflight_failed")),
    );
    fixture.run(
        project,
        "declared_checks",
        "refused",
        40,
        Some((2, "not_admissible")),
    );

    let page = fixture.recent.recent(50).unwrap();

    assert_eq!(page.receipts.len(), 4);
    let times: Vec<u64> = page.receipts.iter().map(|row| row.started_at_ms).collect();
    assert_eq!(times, vec![40, 30, 20, 10]);
    let kinds: Vec<&str> = page
        .receipts
        .iter()
        .map(|row| match row.outcome {
            RecentReceiptOutcome::CheckPreflight { .. } => "check_preflight",
            RecentReceiptOutcome::DeclaredChecks { .. } => "declared_checks",
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "declared_checks",
            "check_preflight",
            "declared_checks",
            "check_preflight"
        ],
        "the kinds are interleaved, so grouping by kind would be visible here"
    );
    assert!(!page.more_runs_exist);
}

#[test]
fn the_window_is_applied_by_the_daemon_not_the_caller() {
    // The wrong answer: return every Run and trust the interface to slice. That
    // is what the desktop did, and it is the whole reason this Query exists.
    let fixture = enrolled("daemon-window");
    let project = fixture.project("Beacon Alpha");
    for index in 0..60 {
        fixture.run(
            project,
            "check_preflight",
            "verified",
            index,
            Some((1, "verified")),
        );
    }

    let page = fixture.recent.recent(10).unwrap();

    assert_eq!(page.run_window_applied, 10);
    assert_eq!(page.receipts.len(), 10);
    assert!(
        page.more_runs_exist,
        "50 older Runs exist and are not claimed"
    );
    // Newest first: the window keeps the most recent Runs, not the first ten.
    assert_eq!(page.receipts[0].started_at_ms, 59);
    assert_eq!(page.receipts[9].started_at_ms, 50);
}

#[test]
fn a_window_larger_than_the_maximum_is_clamped_and_says_so() {
    // Two wrong answers: honour 10,000 silently, or refuse it with a new error
    // code. A clamp that does not report itself is a bounded answer wearing a
    // complete one.
    let fixture = enrolled("clamp");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        1,
        Some((1, "verified")),
    );

    let page = fixture.recent.recent(10_000).unwrap();
    assert_eq!(page.run_window_applied, MAX_RECENT_RUN_WINDOW);
    assert!(!page.more_runs_exist);

    // And zero is not a way to ask for nothing.
    assert_eq!(fixture.recent.recent(0).unwrap().run_window_applied, 1);
}

#[test]
fn recent_a_receiptless_dispatches_remain_recorded_open_or_unconfirmed() {
    let fixture = enrolled("recent-a-recorded-states");
    let project = fixture.project("Recorded dispatches");
    let reference_time = i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    // Before/equal/after this fixed observation instant. The query deliberately
    // does not compare its own clock with the lease or reconcile stored state.
    for lease in [reference_time - 1, reference_time, reference_time + 60_000] {
        fixture
            .store
            .connection()
            .execute(
                "INSERT INTO runs (workspace_id, id, project_id, kind, repository_revision,
             charter_revision, state, lease_expires_at_ms, started_at_ms, ended_at_ms)
             VALUES (?1, ?2, ?3, 'proposal_dispatch', 1, 1, 'dispatching', ?4, ?5, NULL)",
                params![
                    fixture.workspace_id.to_string(),
                    Uuid::new_v4().to_string(),
                    project.to_string(),
                    lease,
                    reference_time - 1_000
                ],
            )
            .unwrap();
    }
    fixture.run(project, "proposal_dispatch", "unconfirmed", 10, None);
    let page = fixture.recent.recent(50).unwrap();
    assert_eq!(page.runs_in_flight_in_window, 3);
    assert_eq!(page.runs_unconfirmed_in_window, 1);
    assert_eq!(page.runs_verified_excluded_in_window, 0);
    assert!(page.receipts.is_empty());
    assert_eq!(
        fixture.recent.recent(50).unwrap(),
        page,
        "a reading must not reconcile state"
    );
}

fn recent_a_bad_receiptless_tuple(
    kind: &str,
    state: &str,
    started: i64,
    ended: Option<i64>,
    lease: i64,
) {
    let fixture = enrolled("recent-a-bad-tuple");
    let project = fixture.project("Bad tuple");
    let connection = fixture.store.connection();
    connection
        .pragma_update(None, "ignore_check_constraints", "ON")
        .unwrap();
    connection.execute(
        "INSERT INTO runs (workspace_id, id, project_id, kind, repository_revision, charter_revision,
          state, lease_expires_at_ms, started_at_ms, ended_at_ms) VALUES (?1, ?2, ?3, ?4, 1, 1, ?5, ?6, ?7, ?8)",
        params![fixture.workspace_id.to_string(), Uuid::new_v4().to_string(), project.to_string(), kind, state, lease, started, ended],
    ).unwrap();
    assert!(
        matches!(
            fixture.recent.recent(50),
            Err(RecentReceiptError::InvalidStore)
        ),
        "{kind}/{state}/{started}/{ended:?}/{lease}: malformed tuple must not count"
    );
}

#[test]
fn recent_a_open_dispatch_with_end_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "dispatching", 10, Some(11), 100);
}
#[test]
fn recent_a_unconfirmed_dispatch_without_end_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "unconfirmed", 10, None, 0);
}
#[test]
fn recent_a_unconfirmed_dispatch_without_expired_lease_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "unconfirmed", 10, Some(20), 20);
}
#[test]
fn recent_a_dispatch_in_another_kinds_open_state_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "preflighting", 10, None, 100);
}
#[test]
fn recent_a_unknown_receiptless_kind_is_invalid() {
    recent_a_bad_receiptless_tuple("future_kind", "preflighting", 10, None, 100);
}
#[test]
fn recent_a_negative_dispatch_start_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "dispatching", -1, None, 100);
}
#[test]
fn recent_a_negative_dispatch_lease_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "dispatching", 10, None, -1);
}
#[test]
fn recent_a_unconfirmed_dispatch_ending_before_start_is_invalid() {
    recent_a_bad_receiptless_tuple("proposal_dispatch", "unconfirmed", 10, Some(9), 0);
}

#[test]
fn recent_a_open_dispatch_with_terminal_event_is_invalid() {
    let fixture = enrolled("recent-a-open-event");
    let project = fixture.project("Open with terminal event");
    let run = Uuid::new_v4();
    fixture.store.connection().execute(
        "INSERT INTO runs (workspace_id, id, project_id, kind, repository_revision, charter_revision,
          state, lease_expires_at_ms, started_at_ms, ended_at_ms) VALUES (?1, ?2, ?3, 'proposal_dispatch', 1, 1, 'dispatching', 100, 10, NULL)",
        params![fixture.workspace_id.to_string(), run.to_string(), project.to_string()],
    ).unwrap();
    assert_eq!(
        fixture.recent.recent(50).unwrap().runs_in_flight_in_window,
        1
    );
    fixture.store.connection().execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms, predecessor_kind)
         VALUES (?1, 'proposal.dispatched', ?2, 11, 'unchained')",
        params![fixture.workspace_id.to_string(), serde_json::json!({"runId": run}).to_string()],
    ).unwrap();
    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn a_run_without_a_receipt_is_counted_never_rendered_as_one() {
    // The wrong answer: a row with a null outcome. A Receipt is an account of a
    // Run; an unconfirmed Run recorded none, so a row claiming to be its
    // Receipt is evidence asserting itself where none exists.
    let fixture = enrolled("unconfirmed");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((1, "verified")),
    );
    fixture.run(project, "check_preflight", "unconfirmed", 20, None);
    fixture.run(project, "declared_checks", "preflighting", 30, None);

    let page = fixture.recent.recent(50).unwrap();

    assert_eq!(page.receipts.len(), 1);
    assert_eq!(page.receipts[0].started_at_ms, 10);
    // Two counts, not one: BLUEPRINT:385 reviews "stopping or unconfirmed" as
    // two conditions, and a live lease is not an expired one.
    assert_eq!(page.runs_unconfirmed_in_window, 1);
    assert_eq!(page.runs_in_flight_in_window, 1);
}

#[test]
fn receiptless_runs_are_counted_from_the_same_window_as_the_rows() {
    // The wrong answer, and one I hit while measuring the design: filter for
    // "has no Receipt" and then LIMIT. That draws the count from the whole
    // table rather than from the window the rows came from, so a Workspace with
    // old unconfirmed Runs reports them as if they were recent.
    let fixture = enrolled("same-window");
    let project = fixture.project("Beacon Alpha");
    for index in 0..30 {
        fixture.run(project, "check_preflight", "unconfirmed", index, None);
    }
    for index in 100..105 {
        fixture.run(
            project,
            "check_preflight",
            "verified",
            index,
            Some((1, "verified")),
        );
    }

    let page = fixture.recent.recent(5).unwrap();

    assert_eq!(page.receipts.len(), 5);
    assert_eq!(
        page.runs_unconfirmed_in_window, 0,
        "30 older unconfirmed Runs are outside this window and are not its count"
    );
    assert!(page.more_runs_exist);
}

#[test]
fn a_project_without_receipts_contributes_nothing_and_breaks_nothing() {
    // The wrong answer: an empty row for the quiet Project, or a failed read.
    let fixture = enrolled("quiet-project");
    let busy = fixture.project("Beacon Alpha");
    let _quiet = fixture.project("Beacon Quiet");
    fixture.run(
        busy,
        "check_preflight",
        "verified",
        5,
        Some((1, "verified")),
    );

    let page = fixture.recent.recent(50).unwrap();

    assert_eq!(page.receipts.len(), 1);
    assert_eq!(page.receipts[0].project_name, "Beacon Alpha");
    assert_eq!(page.runs_in_flight_in_window, 0);
    assert_eq!(page.runs_unconfirmed_in_window, 0);
}

#[test]
fn an_outcome_outside_its_kinds_closed_set_is_refused() {
    // The wrong answer: render it. The `receipts.outcome` column is one CHECK
    // shared by both kinds, so the store admits `completed` on a preflight Run.
    // The type must not.
    let fixture = enrolled("wrong-kind-outcome");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((1, "completed")),
    );

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn an_unknown_serialized_outcome_is_refused_before_briefing_returns_a_value() {
    let fixture = enrolled("unknown-serialized-outcome");
    let project = fixture.project("Beacon Alpha");
    let run_id = fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((1, "verified")),
    );
    let connection = fixture.store.connection();
    let body: String = connection
        .query_row(
            "SELECT receipt_json FROM receipts WHERE run_id = ?1",
            [run_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let unknown = body.replacen(
        "\"outcome\":\"verified\"",
        "\"outcome\":\"future_outcome\"",
        1,
    );
    assert_ne!(
        unknown, body,
        "the fixture did not replace the outcome token"
    );
    connection
        .execute_batch("DROP TRIGGER receipts_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE receipts SET receipt_json = ?1 WHERE run_id = ?2",
            params![unknown, run_id.to_string()],
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

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn a_version_its_kind_never_writes_is_refused() {
    // The wrong answer: accept `version IN (1, 2)` for either kind, which is
    // all the shared column enforces. Preflight writes v1; a preflight v2 is
    // not admissible until its own versioned contract lands.
    let fixture = enrolled("wrong-version");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((2, "verified")),
    );

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn an_outcome_that_disagrees_with_its_run_state_is_refused() {
    // The wrong answer: check the kind/outcome pair and stop. ADR 0031 makes
    // the state pairing part of the Receipt contract, and `runs.state` is not
    // on the wire, so the adapter cannot check it -- it is checked here.
    let fixture = enrolled("state-disagrees");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "declared_checks",
        "verified",
        10,
        Some((2, "not_admissible")),
    );

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn a_receipt_attached_to_a_nonterminal_run_is_an_inconsistent_store() {
    // The reciprocal of the receiptless-terminal case: both writers insert the
    // Run's terminal state and its Receipt in one transaction, so a Receipt on
    // a `preflighting` Run cannot have been written by this reader.
    let fixture = enrolled("receipt-on-open-run");
    let project = fixture.project("Beacon Alpha");
    let run = fixture.run(project, "check_preflight", "preflighting", 10, None);
    fixture
        .store
        .connection()
        .execute(
            "INSERT INTO receipts (workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms)
             VALUES (?1, ?2, 1, 'verified', '{}', 11)",
            params![fixture.workspace_id.to_string(), run.to_string()],
        )
        .unwrap();

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn a_terminal_run_without_a_receipt_is_an_inconsistent_store() {
    // The wrong answer: count it as unconfirmed. A `refused` Run recorded a
    // Receipt in the same transaction that set its state; if one is missing,
    // the store is not one this Query can read honestly.
    let fixture = enrolled("terminal-without-receipt");
    let project = fixture.project("Beacon Alpha");
    fixture.run(project, "check_preflight", "refused", 10, None);

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn an_inconsistent_run_outside_the_window_does_not_fail_the_reading() {
    // The wrong answer: validate every Run in the Workspace, so one bad old row
    // makes Briefing unreadable forever. The window scopes validation as well as
    // rows.
    //
    // What this does NOT prove, stated so no reader assumes it: that the read is
    // bounded in WORK. I removed the SQL `LIMIT`, then the iterator bound, then
    // both, and this test passed every time -- because the classification loop
    // breaks at the window whatever the query returned. The memory bound is
    // structural (a lazy iterator that is stepped at most `probe` times, and a
    // loop that breaks), and nothing outside the process can observe it. The
    // guard that IS observable is the query plan, in the test below.
    let fixture = enrolled("beyond-window");
    let project = fixture.project("Beacon Alpha");
    // Inconsistent, and old: a terminal Run with no Receipt.
    fixture.run(project, "check_preflight", "refused", 1, None);
    for index in 100..110 {
        fixture.run(
            project,
            "check_preflight",
            "verified",
            index,
            Some((1, "verified")),
        );
    }

    let page = fixture.recent.recent(5).unwrap();

    assert_eq!(page.receipts.len(), 5);
    assert!(page.more_runs_exist);
}

#[test]
fn a_live_run_that_already_ended_is_an_inconsistent_store() {
    // gpt1's P1: a receiptless `preflighting` Run carrying an end time says two
    // things that cannot both be true -- it is still holding its lease, and it
    // finished. The table's CHECK only requires `ended >= started`, so the store
    // admits the row; the tuple validation this module promises is what refuses
    // it. Counting it as in flight turns a contradiction into a presence.
    let fixture = enrolled("live-run-with-end");
    let project = fixture.project("Beacon Alpha");
    let run_id = Uuid::new_v4();
    fixture
        .store
        .connection()
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'preflighting', 0, 10, 11)",
            params![
                fixture.workspace_id.to_string(),
                run_id.to_string(),
                project.to_string()
            ],
        )
        .unwrap();

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn an_unconfirmed_run_without_an_end_is_an_inconsistent_store() {
    // The other half of the same tuple. Lease reconciliation sets `unconfirmed`
    // and `ended_at_ms` in one statement (`check_preflight.rs`), so an
    // unconfirmed Run that never ended is not a row this reader can have
    // written -- and counting it would report an expired lease that has no end.
    let fixture = enrolled("unconfirmed-without-end");
    let project = fixture.project("Beacon Alpha");
    let run_id = Uuid::new_v4();
    fixture
        .store
        .connection()
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'declared_checks', 1, 1, 'unconfirmed', 0, 10, NULL)",
            params![
                fixture.workspace_id.to_string(),
                run_id.to_string(),
                project.to_string()
            ],
        )
        .unwrap();

    assert!(matches!(
        fixture.recent.recent(50),
        Err(RecentReceiptError::InvalidStore)
    ));
}

#[test]
fn the_returned_row_does_not_change_as_the_receipt_body_grows() {
    // Revised D-9's invariance, made executable. The previous fixture built one
    // fixed 600-entry payload and nothing observed its size, so shrinking it to
    // a single entry left all 16 tests green: the property was stated and never
    // measured. gpt1 proved that with the exact mutant.
    //
    // Two Runs identical in every field the Query returns except the identity
    // that has to differ for them to be two rows, and Receipt bodies two orders
    // of magnitude apart.
    let fixture = enrolled("payload-growth");
    let project = fixture.project("Beacon Alpha");
    let small = fixture.run_with_body(project, 100, 1);
    let large = fixture.run_with_body(project, 100, 600);

    // The thresholds belong to this test. A shrunk padding fails HERE, loudly,
    // rather than passing quietly somewhere downstream.
    assert!(small < 4 * 1024, "the small body is {small} bytes");
    assert!(large > 32 * 1024, "the large body is only {large} bytes");
    assert!(
        large > small * 8,
        "the two bodies are not materially different: {small} vs {large}"
    );

    let page = fixture.recent.recent(50).unwrap();
    assert_eq!(page.receipts.len(), 2);

    // Everything the Query returns is equal across a 100x payload difference.
    // Comparing whole rows rather than a chosen field is the point: a future
    // column that did vary with the body would fail this without anyone having
    // to predict it.
    let anonymised: Vec<_> = page
        .receipts
        .iter()
        .map(|receipt| RecentReceipt {
            run_id: Uuid::nil(),
            ..receipt.clone()
        })
        .collect();
    assert_eq!(anonymised[0], anonymised[1]);
}
