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
    AutonomyProfile, MAX_RECENT_RUN_WINDOW, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectService, RecentReceiptError, RecentReceiptOutcome,
    RecentReceiptService, SecretString, StartupConfig, Supervisor,
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
                        // Deliberately not a Receipt body. Nothing in this Query
                        // reads it, and a test that stored a real one could not
                        // tell the difference.
                        "{}",
                        started_at_ms + 1
                    ],
                )
                .unwrap();
        }
        run_id
    }
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
        "verified",
        20,
        Some((2, "completed")),
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
fn the_query_never_reads_a_receipt_body() {
    // The property "no payload crosses this Query" proved without promising
    // that a malformed Receipt is ignored: every Receipt above stores `{}`,
    // which no versioned contract would accept, and every reading still
    // succeeds -- because `receipt_json` is not in the projection at all.
    let fixture = enrolled("no-payload");
    let project = fixture.project("Beacon Alpha");
    fixture.run(
        project,
        "check_preflight",
        "verified",
        10,
        Some((1, "verified")),
    );

    let page = fixture.recent.recent(50).unwrap();
    assert_eq!(page.receipts.len(), 1);

    // Stated as a fact about the statement, not only about the result: a
    // projection that grew a payload column would fail here.
    let serialized = serde_json::to_string(&page).unwrap();
    assert!(!serialized.contains("receiptJson"));
    assert!(!serialized.contains("receipt_json"));
}

#[test]
fn the_reading_walks_the_workspace_index_and_sorts_nothing() {
    // The wrong answer that no row assertion can see: the same rows, in the
    // same order, produced by sorting every Run in the Workspace. Without
    // `runs_workspace_started` SQLite plans `USE TEMP B-TREE FOR ORDER BY`,
    // and the cost grows with the Workspace rather than with the window -- a
    // Query that returns a bounded answer without being bounded.
    let fixture = enrolled("query-plan");
    let connection = fixture.store.connection();
    let plan: Vec<String> = connection
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT runs.id, runs.project_id, projects.name, runs.kind,
                    runs.state, runs.started_at_ms, runs.ended_at_ms,
                    receipts.version, receipts.outcome, receipts.recorded_at_ms
             FROM runs
             JOIN projects
               ON projects.workspace_id = runs.workspace_id AND projects.id = runs.project_id
             LEFT JOIN receipts
               ON receipts.workspace_id = runs.workspace_id AND receipts.run_id = runs.id
             WHERE runs.workspace_id = ?1
             ORDER BY runs.started_at_ms DESC, runs.id DESC
             LIMIT ?2",
        )
        .unwrap()
        .query_map(params![fixture.workspace_id.to_string(), 51], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let plan = plan.join("\n");

    assert!(
        plan.contains("runs_workspace_started"),
        "the ordering index is not used:\n{plan}"
    );
    assert!(
        !plan.contains("TEMP B-TREE"),
        "the plan sorts the Workspace before the window:\n{plan}"
    );
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
