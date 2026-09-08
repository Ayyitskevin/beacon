//! Running a Project's Declared Checks — the composition of ADR 0027 and 0029.
//!
//! These tests run real commands, in real Git worktrees, under a real sandbox.
//! The point of the slice is that three separately-proven pieces compose, so a
//! test that stubbed any of them would prove nothing.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, Ceiling, CheckPreflightService, CleanupOutcome, DeclaredCheck,
    DeclaredCheckService, DiagnosticStatus, ExecutionBounds, ExecutionOutcome, ExecutionSummary,
    KeepReason, OperatorAccountService, OperatorSessionPhase, PreflightOutcome,
    ProjectCharterDraft, ProjectDraft, ProjectRepositoryService, ProjectService, Recorded,
    RepositoryDraft, RepositorySourceKind, SecretString, StartupConfig, Supervisor,
    SupervisorError, Worktree, detect_sandbox, run_bounded_for_test,
};
use rusqlite::{Connection, OptionalExtension};
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-declared-checks-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
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

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!("beacond-dc-{label}-{}", Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn git(repository: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(arguments)
        .current_dir(repository)
        .env("GIT_AUTHOR_NAME", "Beacon Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Beacon Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .status()
        .expect("git is available in the test environment");
    assert!(status.success(), "git {arguments:?} failed");
}

/// A repository whose committed content the checks can observe.
fn fixture_repository(label: &str) -> (TempDir, String) {
    let directory = TempDir::new(label);
    git(&directory.path, &["init", "--quiet"]);
    fs::write(directory.path.join("marker.txt"), "committed-content\n").unwrap();
    fs::create_dir_all(directory.path.join("nested")).unwrap();
    fs::write(directory.path.join("nested/where.txt"), "nested\n").unwrap();
    git(&directory.path, &["add", "."]);
    git(&directory.path, &["commit", "--quiet", "-m", "one"]);
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&directory.path)
        .output()
        .unwrap();
    let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    (directory, revision)
}

fn revision(repository: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(repository)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn enrolled(
    label: &str,
) -> (
    TestStore,
    ProjectService,
    ProjectRepositoryService,
    DeclaredCheckService,
) {
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
    (
        store,
        ProjectService::new(Arc::clone(&accounts), report.workspace.id),
        ProjectRepositoryService::new(Arc::clone(&accounts), report.workspace.id),
        DeclaredCheckService::new(accounts, report.workspace.id),
    )
}

fn enrolled_v15(
    label: &str,
) -> (
    TestStore,
    ProjectService,
    ProjectRepositoryService,
    CheckPreflightService,
) {
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
    accounts.complete_operator_setup(true).unwrap();
    (
        store,
        ProjectService::new(Arc::clone(&accounts), workspace_id),
        ProjectRepositoryService::new(Arc::clone(&accounts), workspace_id),
        CheckPreflightService::new(accounts, workspace_id),
    )
}

/// Hand-unwinds the store to v15 so a real v15 store can be migrated forward again.
///
/// Every schema version added since v15 has to be undone here, newest first: v19's supersession
/// relation and `rulings` table (the relation first, since it keys into `rulings`) and the index
/// 0019 added to `proposals`, then v18's `proposals` table and the four unique
/// indexes it added to other tables so composite foreign keys could state Proposal scope, then
/// v17's Sample Project state, then v16's Run and Receipt rebuild.
/// A version that forgets to unwind leaves objects behind and the schema comparison below fails.
fn downgrade_execution_schema_to_v15(store_path: &Path) {
    let expected = Connection::open_in_memory().unwrap();
    for migration in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_operator_account.sql"),
        include_str!("../migrations/0003_session_policy.sql"),
        include_str!("../migrations/0004_starter_organization.sql"),
        include_str!("../migrations/0005_projects.sql"),
        include_str!("../migrations/0006_project_charter_revisions.sql"),
        include_str!("../migrations/0007_local_model_activations.sql"),
        include_str!("../migrations/0008_resident_lifecycle.sql"),
        include_str!("../migrations/0009_resident_configurations.sql"),
        include_str!("../migrations/0010_teams.sql"),
        include_str!("../migrations/0011_missions.sql"),
        include_str!("../migrations/0012_project_policy.sql"),
        include_str!("../migrations/0013_project_repository.sql"),
        include_str!("../migrations/0014_run_receipt.sql"),
        include_str!("../migrations/0015_declared_check_runs.sql"),
    ] {
        expected.execute_batch(migration).unwrap();
    }
    let schema_sql = |name: &str| -> String {
        expected
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    };
    let runs_table = schema_sql("runs");
    let receipts_table = schema_sql("receipts");
    let events_table = schema_sql("events");
    let team_roster_trigger = schema_sql("team_formation_members_no_insert_after_formation");
    let mission_roster_trigger = schema_sql("mission_admission_members_no_insert_after_admission");
    let remaining_objects = [
        "runs_project_started",
        "runs_open_lease",
        "runs_no_delete",
        "runs_single_terminal_transition",
        "receipts_no_update",
        "receipts_no_delete",
    ]
    .map(schema_sql);

    let connection = Connection::open(store_path).unwrap();
    let read_events = |connection: &Connection| {
        let mut statement = connection
            .prepare(
                "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
                 FROM events ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let events_before = read_events(&connection);
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             -- Records come off innermost first, newest migration first. Migration 0031's
             -- publication relation is newest: it keys into `artifacts`, `evaluations` and
             -- `missions`, so it comes off before all three. Migration 0029's
             -- Outcome manifest store is newest: it keys into `artifacts`, `mission_completions`
             -- and `missions`, so it comes off before any of them. Migration 0027's Artifact
             -- store is next (the supersession relation first, since it keys into `artifacts`);
             -- migration 0026's completion store is next; migration 0025's spend
             -- and classification tables reference missions and Project Policy revisions;
             -- migration 0024's condition identity is next (ADR 0041); migration 0023's
             -- Evaluation tables reference proposals, and 0022's Ruling/Candidate binding
             -- references both rulings and candidates, which reference proposals in turn.
             DROP TRIGGER outcome_publications_no_update;
             DROP TRIGGER outcome_publications_no_delete;
             DROP INDEX outcome_publications_by_mission;
             DROP TABLE outcome_publications;
             DROP TRIGGER outcome_manifests_no_update;
             DROP TRIGGER outcome_manifests_no_delete;
             DROP INDEX outcome_manifests_by_mission;
             DROP TABLE outcome_manifests;
             DROP TRIGGER artifact_supersessions_no_update;
             DROP TRIGGER artifact_supersessions_no_delete;
             DROP TABLE artifact_supersessions;
             DROP TRIGGER artifacts_no_update;
             DROP TRIGGER artifacts_no_delete;
             DROP TABLE artifacts;
             DROP TRIGGER mission_completion_conditions_no_update;
             DROP TRIGGER mission_completion_conditions_no_delete;
             DROP TABLE mission_completion_conditions;
             DROP TRIGGER mission_completions_no_update;
             DROP TRIGGER mission_completions_no_delete;
             DROP TABLE mission_completions;
             DROP TRIGGER mission_impact_classifications_no_update;
             DROP TRIGGER mission_impact_classifications_no_delete;
             DROP TABLE mission_impact_classifications;
             DROP TABLE measured_spend_lines;
             DROP TRIGGER measured_spend_no_update;
             DROP TRIGGER measured_spend_no_delete;
             DROP TABLE measured_spend;
             -- Migration 0024's condition identity: its triggers first, then the table (ADR 0041).
             DROP TRIGGER mission_success_conditions_no_update;
             DROP TRIGGER mission_success_conditions_no_delete;
             DROP TABLE mission_success_conditions;
             DROP TABLE evaluation_supersessions;
             DROP TABLE evaluation_cited_runs;
             DROP TABLE evaluation_findings;
             DROP TABLE evaluations;
             DROP TRIGGER beacon_repositories_no_update;
             DROP TABLE beacon_repositories;
             DROP TRIGGER dispatch_preparations_no_update;
             -- R58: the chain's own state and trigger, which 0035 added. A rollback fixture that
             -- does not undo the newest migration leaves objects behind and compares a v17 store
             -- against a signature that has them -- which is how this fixture caught the omission.
             DROP TRIGGER event_chain_state_no_delete;
             DROP TABLE event_chain_state;
             DROP TRIGGER dispatch_preparations_no_delete;
             DROP INDEX dispatch_preparations_one_per_ruling;
             DROP TABLE dispatch_preparations;
             DROP TRIGGER ruling_candidates_no_update;
             DROP TRIGGER ruling_candidates_no_delete;
             DROP TABLE ruling_candidates;
             DROP TRIGGER candidates_no_update;
             DROP TRIGGER candidates_no_delete;
             DROP INDEX candidates_one_per_proposal;
             DROP INDEX candidates_workspace_digest;
             DROP TABLE candidates;
             DROP TABLE ruling_supersessions;
             DROP TABLE rulings;
             DROP INDEX proposals_workspace_digest;
             DROP TABLE proposals;
             DROP INDEX project_repository_revisions_charter_base;
             DROP INDEX project_policy_revisions_charter;
             DROP INDEX mission_admission_members_configuration;
             DROP INDEX missions_workspace_project_id;
             DROP TRIGGER sample_project_installations_no_update;
             DROP TRIGGER sample_project_installations_no_delete;
             DROP TABLE sample_project_installations;
             DROP TRIGGER team_formation_members_no_insert_after_formation;
             DROP TRIGGER mission_admission_members_no_insert_after_admission;
             ALTER TABLE events RENAME TO events_v17;",
        )
        .unwrap();
    connection.execute_batch(&events_table).unwrap();
    connection
        .execute_batch(
            // R58: this fixture rolls the store back to v17, where `events` has no chain columns
            // at all -- the table it copies INTO is the v17 shape it just created. Adding the
            // columns here would be describing a schema this fixture exists to leave behind.
            "INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
             SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
             FROM events_v17 ORDER BY sequence;
             DROP TABLE events_v17;",
        )
        .unwrap();
    connection.execute_batch(&team_roster_trigger).unwrap();
    connection.execute_batch(&mission_roster_trigger).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER runs_no_delete;
             DROP TRIGGER runs_single_terminal_transition;
             DROP TRIGGER receipts_no_update;
             DROP TRIGGER receipts_no_delete;
             DROP INDEX runs_project_started;
             DROP INDEX runs_open_lease;
             DROP INDEX runs_workspace_started;
             DROP TABLE installation_pause;
             ALTER TABLE receipts RENAME TO receipts_v16;
             ALTER TABLE runs RENAME TO runs_v16;",
        )
        .unwrap();
    connection.execute_batch(&runs_table).unwrap();
    connection.execute_batch(&receipts_table).unwrap();
    for sql in remaining_objects {
        connection.execute_batch(&sql).unwrap();
    }
    connection
        .execute_batch(
            "INSERT INTO runs SELECT * FROM runs_v16;
             INSERT INTO receipts SELECT * FROM receipts_v16;
             DROP TABLE receipts_v16;
             DROP TABLE runs_v16;
             UPDATE schema_meta SET version = 15 WHERE singleton = 1;
             COMMIT;",
        )
        .unwrap();
    assert_eq!(read_events(&connection), events_before);
    let read_schema = |connection: &Connection| {
        let mut statement = connection
            .prepare(
                r"SELECT type, name, tbl_name, sql
                 FROM sqlite_schema
                 WHERE name NOT LIKE 'sqlite\_%' ESCAPE '\'
                 ORDER BY type, name",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(read_schema(&connection), read_schema(&expected));
    assert_eq!(
        connection
            .query_row("PRAGMA foreign_key_check", [], |row| row
                .get::<_, String>(0))
            .optional()
            .unwrap(),
        None
    );
}

fn project_draft() -> ProjectDraft {
    ProjectDraft {
        name: "Beacon Alpha".to_owned(),
        charter: ProjectCharterDraft {
            purpose: "Build a trustworthy local software product.".to_owned(),
            desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
            constraints: vec!["Keep Workspace data local".to_owned()],
            prohibitions: vec!["Do not deploy publicly".to_owned()],
            autonomy_profile: AutonomyProfile::Supervised,
        },
    }
}

fn check(name: &str, argv: &[&str], working_directory: &str) -> DeclaredCheck {
    DeclaredCheck {
        name: name.to_owned(),
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        working_directory: working_directory.to_owned(),
    }
}

fn bounds() -> ExecutionBounds {
    ExecutionBounds {
        wall_clock_ms: 10_000,
        cpu_seconds: 5,
        address_space_bytes: 1 << 30,
        file_size_bytes: 1 << 20,
        output_bytes: 8192,
    }
}

fn enforcing_or_skip() -> bool {
    if detect_sandbox().admits_execution() {
        return true;
    }
    eprintln!("skipped: this host reports no enforcing sandbox");
    false
}

fn runs_of_kind(store: &TestStore, kind: &str) -> Vec<(String, String)> {
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, state FROM runs WHERE kind = ?1 ORDER BY started_at_ms")
        .unwrap();
    statement
        .query_map([kind], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn run_pin(store: &TestStore, run_id: Uuid) -> (u32, u32) {
    let connection = Connection::open(&store.path).unwrap();
    connection
        .query_row(
            "SELECT repository_revision, charter_revision FROM runs WHERE id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get::<_, u32>(0)?, row.get::<_, u32>(1)?)),
        )
        .unwrap()
}

fn run_kinds_in_open_order(store: &TestStore) -> Vec<String> {
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare("SELECT kind FROM runs ORDER BY rowid")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn a_declared_check_runs_in_a_worktree_at_the_pinned_revision() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("happy");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("happy");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision.clone(),
                declared_checks: vec![check("marker", &["/bin/cat", "marker.txt"], ".")],
            },
        )
        .unwrap();

    let configured_bounds = bounds();
    let receipt = checks.execute(project.id, configured_bounds).unwrap();

    assert_eq!(receipt.version, 2);
    assert_eq!(receipt.per_check_bounds, configured_bounds);
    assert_eq!(receipt.summary, ExecutionSummary::Completed);
    assert_eq!(receipt.checks.len(), 1);
    let record = &receipt.checks[0];
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 0 });
    // The check read the repository's committed content, so the worktree was
    // real and checked out at the pinned revision.
    assert_eq!(record.stdout.trim(), "committed-content");
    // And it ran in a worktree, not in the source repository.
    assert_ne!(record.working_directory, repository.path.to_string_lossy());

    // The preflight that admitted it travels with the Receipt.
    assert_eq!(receipt.admitted_by.base_revision, revision);
    // Two Runs: the preflight and the execution, each with its own Receipt.
    assert_eq!(runs_of_kind(&store, "check_preflight").len(), 1);
    let executions = runs_of_kind(&store, "declared_checks");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].1, "verified");
    // The worktree was cleaned up: nothing was left behind by a passing run.
    assert_eq!(receipt.worktree, Some(CleanupOutcome::Removed {}));
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision reachability from a ref (object presence verified only)",
            "UDP egress (Landlock restricts TCP only)",
            "complete filesystem effects and diff digest (not recorded)",
        ]
    );
}

#[test]
fn a_committed_relative_symlink_cannot_escape_the_execution_worktree() {
    if !enforcing_or_skip() {
        return;
    }

    // The source tree is deliberately one directory deeper than Beacon's
    // `scratch/tree` layout. This relative symlink resolves back inside the
    // source repository during preflight, but into the independently created
    // outside directory when interpreted from the disposable worktree.
    let container = TempDir::new("committed-relative-escape-source");
    let unique = format!("beacon-escape-{}", Uuid::new_v4());
    let repository_path = container.path.join("parent").join(&unique).join("source");
    let outside = TempDir {
        path: std::env::temp_dir().join(&unique),
    };
    let outside_path = outside.path.join("source/inside");
    fs::create_dir_all(repository_path.join("nested")).unwrap();
    fs::create_dir_all(repository_path.join("inside")).unwrap();
    fs::create_dir_all(&outside_path).unwrap();
    symlink(
        format!("../../../{unique}/source/inside"),
        repository_path.join("nested/cwd"),
    )
    .unwrap();
    git(&repository_path, &["init", "--quiet"]);
    git(&repository_path, &["add", "."]);
    git(&repository_path, &["commit", "--quiet", "-m", "one"]);
    let revision = revision(&repository_path);

    let source = repository_path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("committed-relative-escape");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("escaped", &["/bin/pwd"], "nested/cwd")],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    let record = &receipt.checks[0];
    let expected_reason = format!(
        "unsafe or raced working-directory resolution (errno {})",
        libc::EXDEV
    );
    assert!(matches!(
        &record.outcome,
        ExecutionOutcome::NotStarted { reason }
            if reason == &expected_reason
    ));
    assert!(record.stdout.is_empty(), "the escaped target program ran");
    assert!(
        record.working_directory.ends_with("nested/cwd"),
        "the Receipt lost the attempted worktree locator"
    );
    assert_eq!(
        receipt.admitted_by.resolved_checks[0].resolved_working_directory,
        Recorded::Present {
            value: repository_path
                .join("inside")
                .to_string_lossy()
                .into_owned()
        }
    );
}

#[test]
fn a_prior_check_cannot_redirect_the_next_cwd_and_later_checks_continue() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, _first_revision) = fixture_repository("prior-check-cwd-swap");
    fs::create_dir_all(repository.path.join("victim")).unwrap();
    fs::write(repository.path.join("victim/tracked.txt"), "tracked\n").unwrap();
    git(&repository.path, &["add", "."]);
    git(&repository.path, &["commit", "--quiet", "-m", "two"]);
    let revision = revision(&repository.path);
    let outside = TempDir::new("prior-check-cwd-swap-outside");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("prior-check-cwd-swap");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check(
                        "a-swap",
                        &[
                            "/bin/sh",
                            "-c",
                            &format!(
                                "/bin/rm -rf victim && /bin/ln -s {} victim",
                                outside.path.display()
                            ),
                        ],
                        ".",
                    ),
                    check("b-victim", &["/bin/pwd"], "victim"),
                    check("c-continues", &["/bin/echo", "continued"], "."),
                ],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(
        receipt.checks[0].outcome,
        ExecutionOutcome::Exited { code: 0 }
    );
    assert!(matches!(
        receipt.checks[1].outcome,
        ExecutionOutcome::NotStarted { .. }
    ));
    assert!(
        receipt.checks[1].stdout.is_empty(),
        "the redirected target program ran"
    );
    assert_eq!(receipt.checks[2].stdout.trim(), "continued");

    let worktree_root = Path::new(&receipt.checks[0].working_directory);
    git(
        &repository.path,
        &[
            "worktree",
            "remove",
            "--force",
            worktree_root.to_string_lossy().as_ref(),
        ],
    );
    if let Some(scratch_root) = worktree_root.parent() {
        let _ = fs::remove_dir(scratch_root);
    }
}

#[test]
fn relative_internal_symlinks_run_while_absolute_targets_refuse() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, _first_revision) = fixture_repository("symlink-policy");
    fs::create_dir_all(repository.path.join("inside")).unwrap();
    fs::write(repository.path.join("inside/value.txt"), "inside\n").unwrap();
    symlink("inside", repository.path.join("relative-inside")).unwrap();
    symlink(&repository.path, repository.path.join("absolute-inside")).unwrap();
    git(&repository.path, &["add", "."]);
    git(&repository.path, &["commit", "--quiet", "-m", "two"]);
    let revision = revision(&repository.path);
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("symlink-policy");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check("a-absolute", &["/bin/pwd"], "absolute-inside"),
                    check("b-relative", &["/bin/cat", "value.txt"], "relative-inside"),
                ],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert!(matches!(
        &receipt.checks[0].outcome,
        ExecutionOutcome::NotStarted { reason }
            if reason.starts_with("unsafe or raced working-directory resolution (errno ")
    ));
    assert!(receipt.checks[0].stdout.is_empty());
    assert_eq!(
        receipt.checks[1].outcome,
        ExecutionOutcome::Exited { code: 0 }
    );
    assert_eq!(receipt.checks[1].stdout.trim(), "inside");
    assert!(
        receipt.checks[1]
            .working_directory
            .ends_with("relative-inside")
    );
    assert_eq!(
        receipt.admitted_by.resolved_checks[1].resolved_working_directory,
        Recorded::Present {
            value: repository
                .path
                .join("inside")
                .to_string_lossy()
                .into_owned()
        }
    );
}

#[test]
fn a_failing_check_completes_the_run_without_claiming_success() {
    if !enforcing_or_skip() {
        return;
    }
    // "Completed" means the checks ran. A successful process exit is not a
    // successful Mission, and neither is an unsuccessful one a failed Run.
    let (repository, revision) = fixture_repository("failing");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("failing");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("fails", &["/bin/sh", "-c", "exit 7"], ".")],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::Completed);
    assert_eq!(
        receipt.checks[0].outcome,
        ExecutionOutcome::Exited { code: 7 }
    );
}

#[test]
fn a_missing_executable_makes_the_run_incomplete_without_stopping_dispatch() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("mixed-not-started");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("mixed-not-started");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check("a-starts", &["/bin/sh", "-c", "printf before"], "."),
                    check("b-missing", &["/beacon-test/missing-executable"], "."),
                    check("c-starts", &["/bin/sh", "-c", "printf after"], "."),
                ],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();

    assert_eq!(receipt.summary, ExecutionSummary::Incomplete);
    assert_eq!(receipt.checks.len(), 3);
    assert_eq!(receipt.checks[0].stdout, "before");
    assert!(matches!(
        receipt.checks[1].outcome,
        ExecutionOutcome::NotStarted { .. }
    ));
    assert_eq!(receipt.checks[2].stdout, "after");
    assert_eq!(runs_of_kind(&store, "declared_checks")[0].1, "incomplete");
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
}

#[test]
fn all_not_started_records_use_the_same_incomplete_summary() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("all-not-started");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("all-not-started");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check("a-missing", &["/beacon-test/missing-a"], "."),
                    check("b-missing", &["/beacon-test/missing-b"], "."),
                ],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();

    assert_eq!(receipt.summary, ExecutionSummary::Incomplete);
    assert_eq!(receipt.checks.len(), 2);
    assert!(
        receipt
            .checks
            .iter()
            .all(|record| matches!(record.outcome, ExecutionOutcome::NotStarted { .. }))
    );
    assert_eq!(runs_of_kind(&store, "declared_checks")[0].1, "incomplete");
    assert!(!receipt.not_verified.iter().any(|gap| gap.contains("UDP")));
}

#[test]
fn output_incomplete_and_truncated_are_independent_observations() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("output-observations");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("output-observations");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check(
                        "a-incomplete",
                        &[
                            "/bin/sh",
                            "-c",
                            "/usr/bin/setsid /bin/sleep 5 & printf ok; exit 0",
                        ],
                        ".",
                    ),
                    check("b-truncated", &["/bin/sh", "-c", "printf 123456789"], "."),
                    check(
                        "c-both",
                        &[
                            "/bin/sh",
                            "-c",
                            "printf 123456789; /usr/bin/setsid /bin/sleep 5 1>&2 & exit 0",
                        ],
                        ".",
                    ),
                ],
            },
        )
        .unwrap();
    let mut configured_bounds = bounds();
    configured_bounds.output_bytes = 4;

    let receipt = checks.execute(project.id, configured_bounds).unwrap();

    assert_eq!(receipt.summary, ExecutionSummary::Completed);
    assert_eq!(
        receipt.checks[0].outcome,
        ExecutionOutcome::Exited { code: 0 }
    );
    assert!(!receipt.checks[0].output_truncated);
    assert!(receipt.checks[0].output_incomplete);
    assert!(receipt.checks[1].output_truncated);
    assert!(!receipt.checks[1].output_incomplete);
    assert!(receipt.checks[2].output_truncated);
    assert!(receipt.checks[2].output_incomplete);
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision reachability from a ref (object presence verified only)",
            "UDP egress (Landlock restricts TCP only)",
            "complete filesystem effects and diff digest (not recorded)",
            "Declared Check \"a-incomplete\" complete output (collection stopped before EOF)",
            "Declared Check \"b-truncated\" output beyond the configured capture ceiling",
            "Declared Check \"c-both\" output beyond the configured capture ceiling",
            "Declared Check \"c-both\" complete output (collection stopped before EOF)",
        ]
    );
}

#[test]
fn every_positive_terminal_outcome_keeps_completed_distinct_from_success() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("positive-terminal");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("positive-terminal");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check("a-bounded", &["/bin/sleep", "1"], "."),
                    check("b-nonzero", &["/bin/sh", "-c", "exit 7"], "."),
                    check("c-signalled", &["/bin/sh", "-c", "kill -TERM $$"], "."),
                ],
            },
        )
        .unwrap();
    let mut configured_bounds = bounds();
    configured_bounds.wall_clock_ms = 50;

    let receipt = checks.execute(project.id, configured_bounds).unwrap();

    assert_eq!(receipt.summary, ExecutionSummary::Completed);
    assert_eq!(
        receipt.checks[0].outcome,
        ExecutionOutcome::Bounded {
            ceiling: Ceiling::WallClock
        }
    );
    assert_eq!(
        receipt.checks[1].outcome,
        ExecutionOutcome::Exited { code: 7 }
    );
    assert_eq!(
        receipt.checks[2].outcome,
        ExecutionOutcome::Signalled { signal: 15 }
    );
    assert_eq!(runs_of_kind(&store, "declared_checks")[0].1, "verified");
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision reachability from a ref (object presence verified only)",
            "UDP egress (Landlock restricts TCP only)",
            "complete filesystem effects and diff digest (not recorded)",
            "Declared Check \"a-bounded\" completion beyond the wall_clock ceiling",
            "Declared Check \"c-signalled\" normal completion and exit code (terminated by signal 15)",
        ]
    );
}

#[test]
fn kept_worktree_reasons_are_recorded_exactly_after_check_uncertainty() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("kept-worktree");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("kept-worktree");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check(
                    "writes",
                    &[
                        "/bin/sh",
                        "-c",
                        "printf changed > marker.txt; printf new > artifact.txt",
                    ],
                    ".",
                )],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();

    assert_eq!(
        receipt.worktree,
        Some(CleanupOutcome::Kept {
            reasons: vec![KeepReason::Dirty, KeepReason::UntrackedContent]
        })
    );
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision reachability from a ref (object presence verified only)",
            "UDP egress (Landlock restricts TCP only)",
            "complete filesystem effects and diff digest (not recorded)",
            "worktree removal (kept: dirty)",
            "worktree removal (kept: untracked_content)",
        ]
    );

    let worktree_path = receipt.checks[0].working_directory.clone();
    git(
        &repository.path,
        &["worktree", "remove", "--force", &worktree_path],
    );
    if let Some(scratch_root) = Path::new(&worktree_path).parent() {
        let _ = fs::remove_dir(scratch_root);
    }
}

#[test]
fn every_declared_check_runs_in_its_own_declared_directory() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("multi");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("multi");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![
                    check("root", &["/bin/cat", "marker.txt"], "."),
                    check("nested", &["/bin/cat", "where.txt"], "nested"),
                ],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.checks.len(), 2);

    // ADR 0026 stores Declared Checks in canonical *name* order, not in the
    // order they were written, so "nested" precedes "root". Asserting by name
    // tests the property that matters -- each check ran where it declared --
    // without re-asserting the binding's ordering rule from the wrong end.
    let by_name = |name: &str| {
        receipt
            .checks
            .iter()
            .find(|record| record.argv.iter().any(|argument| argument.contains(name)))
            .unwrap_or_else(|| panic!("no record ran {name}"))
    };
    assert_eq!(by_name("marker.txt").stdout.trim(), "committed-content");
    let nested = by_name("where.txt");
    assert_eq!(nested.stdout.trim(), "nested");
    assert!(nested.working_directory.ends_with("nested"));

    // And the canonical order itself, asserted once and deliberately.
    assert!(
        receipt.checks[0].working_directory.ends_with("nested"),
        "checks are stored in canonical name order, so `nested` runs before `root`"
    );
}

#[test]
fn a_check_may_write_inside_its_worktree_and_not_outside_it() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("containment");
    let outside = TempDir::new("containment-target");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, checks) = enrolled("containment");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check(
                    "writes",
                    &[
                        "/bin/sh",
                        "-c",
                        &format!(
                            "echo in > ./inside.txt && echo out > {}/outside.txt",
                            outside.path.display()
                        ),
                    ],
                    ".",
                )],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    // The write inside succeeded and the one outside did not, so the shell
    // failed overall.
    assert_ne!(
        receipt.checks[0].outcome,
        ExecutionOutcome::Exited { code: 0 }
    );
    assert!(
        !outside.path.join("outside.txt").exists(),
        "containment failed: a check wrote outside its worktree"
    );
    // The source repository is untouched: a Run mutates its worktree, never
    // the repository it was cut from.
    assert!(!repository.path.join("inside.txt").exists());
}

#[test]
fn an_inadmissible_preflight_refuses_before_anything_runs() {
    if !enforcing_or_skip() {
        return;
    }
    // A binding whose pinned revision is not in the repository cannot be
    // preflighted, so no worktree is created and no check runs.
    let (repository, _revision) = fixture_repository("inadmissible");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("inadmissible");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: "b".repeat(40),
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::NotAdmissible);
    assert_eq!(
        receipt.admitted_by.outcome,
        PreflightOutcome::BaseRevisionUnreachable,
        "NotAdmissible carries the preflight refusal, never a Verified preflight"
    );
    assert!(receipt.checks.is_empty(), "a refused Run ran nothing");
    assert_eq!(receipt.worktree, None, "no worktree was created");
    // The refusal is still a Run with a Receipt: it happened, and it is recorded.
    let executions = runs_of_kind(&store, "declared_checks");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].1, "refused");
    assert_eq!(receipt.per_check_bounds, bounds());
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision presence in the repository object store",
            "base revision reachability from a ref (object presence verified only)",
            "Declared Check working-directory containment",
            "sandbox enforcement",
            "worktree creation (preflight did not admit execution)",
            "every Declared Check (preflight did not admit execution, so no check ran)",
        ]
    );
}

#[test]
fn execution_uses_its_pinned_binding_and_the_next_run_pins_the_new_one() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, first_revision) = fixture_repository("binding-moved-after-open");
    fs::write(repository.path.join("marker.txt"), "next-binding\n").unwrap();
    git(&repository.path, &["add", "marker.txt"]);
    git(&repository.path, &["commit", "--quiet", "-m", "two"]);
    let second_revision = revision(&repository.path);
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("binding-moved-after-open");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: first_revision.clone(),
                declared_checks: vec![check("marker", &["/bin/cat", "marker.txt"], ".")],
            },
        )
        .unwrap();
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER supersede_binding_when_execution_opens
             AFTER INSERT ON runs
             WHEN NEW.kind = 'declared_checks'
             BEGIN
                 INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind,
                     source, base_revision, declared_checks_json, author_kind, authored_at_ms
                 )
                 SELECT workspace_id, project_id, revision + 1, charter_revision, source_kind,
                        source, '{second_revision}', declared_checks_json, author_kind, authored_at_ms
                 FROM project_repository_revisions
                 WHERE workspace_id = NEW.workspace_id
                   AND project_id = NEW.project_id
                   AND revision = NEW.repository_revision;
             END;"
        ))
        .unwrap();
    drop(connection);

    let first = checks.execute(project.id, bounds()).unwrap();

    assert_eq!(first.summary, ExecutionSummary::Completed);
    assert_eq!(first.admitted_by.outcome, PreflightOutcome::Verified);
    assert_eq!(first.admitted_by.repository_revision, 1);
    assert_eq!(first.admitted_by.policy_repository_revision, 1);
    assert_eq!(first.admitted_by.base_revision, first_revision);
    assert_eq!(first.checks[0].stdout.trim(), "committed-content");
    assert_eq!(run_pin(&store, first.run_id), (1, 1));
    assert_eq!(
        &run_kinds_in_open_order(&store)[..2],
        ["declared_checks", "check_preflight"],
        "execution must pin its revisions before its preflight opens"
    );

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute("DROP TRIGGER supersede_binding_when_execution_opens", [])
        .unwrap();
    drop(connection);

    let second = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(second.summary, ExecutionSummary::Completed);
    assert_eq!(second.admitted_by.repository_revision, 2);
    assert_eq!(second.admitted_by.policy_repository_revision, 2);
    assert_eq!(second.admitted_by.base_revision, second_revision);
    assert_eq!(second.checks[0].stdout.trim(), "next-binding");
    assert_eq!(run_pin(&store, second.run_id), (2, 1));
}

#[test]
fn execution_uses_its_pinned_charter_and_the_next_run_pins_the_new_one() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("charter-moved-after-open");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("charter-moved-after-open");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("marker", &["/bin/cat", "marker.txt"], ".")],
            },
        )
        .unwrap();
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER revise_charter_when_execution_opens
             AFTER INSERT ON runs
             WHEN NEW.kind = 'declared_checks'
             BEGIN
                 INSERT INTO project_charter_revisions (
                     workspace_id, project_id, revision, purpose, desired_outcomes_json,
                     constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                     external_effects, author_kind, authored_at_ms
                 )
                 SELECT workspace_id, project_id, revision + 1, purpose || ' revised',
                        desired_outcomes_json, constraints_json, prohibitions_json,
                        autonomy_profile, budget_boundary, external_effects, author_kind,
                        authored_at_ms + 1
                 FROM project_charter_revisions
                 WHERE workspace_id = NEW.workspace_id
                   AND project_id = NEW.project_id
                   AND revision = NEW.charter_revision;
                 UPDATE projects
                 SET current_charter_revision = NEW.charter_revision + 1,
                     updated_at_ms = updated_at_ms + 1
                 WHERE workspace_id = NEW.workspace_id
                   AND id = NEW.project_id
                   AND current_charter_revision = NEW.charter_revision;
             END;",
        )
        .unwrap();
    drop(connection);

    let first = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(first.summary, ExecutionSummary::Completed);
    assert_eq!(first.admitted_by.outcome, PreflightOutcome::Verified);
    assert_eq!(first.admitted_by.charter_revision, 1);
    assert_eq!(first.admitted_by.policy_charter_revision, 1);
    assert_eq!(first.admitted_by.policy_repository_revision, 1);
    assert_eq!(run_pin(&store, first.run_id), (1, 1));
    assert_eq!(first.checks[0].stdout.trim(), "committed-content");
    assert_eq!(
        &run_kinds_in_open_order(&store)[..2],
        ["declared_checks", "check_preflight"],
        "execution must pin the Charter before its preflight opens"
    );

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute("DROP TRIGGER revise_charter_when_execution_opens", [])
        .unwrap();
    drop(connection);

    let second = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(second.summary, ExecutionSummary::NotAdmissible);
    assert_eq!(
        second.admitted_by.outcome,
        PreflightOutcome::BindingStaleForCharter,
        "NotAdmissible embeds the refusing preflight, never a Verified one"
    );
    assert_eq!(second.admitted_by.charter_revision, 2);
    assert_eq!(second.admitted_by.policy_charter_revision, 2);
    assert_eq!(second.admitted_by.policy_repository_revision, 1);
    assert_eq!(run_pin(&store, second.run_id), (1, 2));
    assert!(second.checks.is_empty());
    assert_eq!(second.worktree, None);
}

#[test]
fn an_unavailable_worktree_refuses_with_fixed_uncertainty_and_no_execution_evidence() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("worktree-unavailable");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("worktree-unavailable");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    // Beacon's object-store preflight does not depend on Git configuration,
    // while worktree creation invokes Git. A malformed config therefore
    // reaches the real, deterministic worktree-refusal seam after admission.
    fs::write(repository.path.join(".git/config"), "[malformed\n").unwrap();

    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::WorktreeUnavailable);
    assert!(receipt.checks.is_empty());
    assert_eq!(receipt.worktree, None);
    assert_eq!(runs_of_kind(&store, "declared_checks")[0].1, "refused");
    assert_eq!(
        receipt.not_verified,
        vec![
            "base revision reachability from a ref (object presence verified only)",
            "worktree creation and any partial checkout effects (creation was refused before a worktree handle was available)",
            "every Declared Check (worktree creation was refused, so no check ran)",
        ]
    );
}

#[test]
fn the_execution_event_replays_against_its_stored_receipt() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("replay");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("replay");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("ok", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    let receipt = checks.execute(project.id, bounds()).unwrap();

    let connection = Connection::open(&store.path).unwrap();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'run.executed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let replayed: beacond::ExecutionReceipt = serde_json::from_str(&payload).unwrap();
    assert_eq!(replayed, receipt);
    drop(connection);

    // And startup reconciliation accepts the store it produced.
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
}

#[test]
fn startup_rejects_execution_when_its_nested_preflight_differs_from_stored_evidence() {
    let (store, projects, repositories, checks) = enrolled("nested-preflight-provenance");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::GitUrl,
                source: "https://example.invalid/original.git".to_owned(),
                base_revision: "a".repeat(40),
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::NotAdmissible);

    let connection = Connection::open(&store.path).unwrap();
    let original: String = connection
        .query_row(
            "SELECT receipt_json FROM receipts WHERE run_id = ?1",
            [receipt.run_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let substituted = original.replacen("\"admittedBy\":{", "\"admittedBy\":{ ", 1);
    assert_ne!(substituted, original, "fixture did not locate admittedBy");
    replace_receipt_update_trigger(&connection, |connection| {
        connection.execute(
            "UPDATE receipts SET receipt_json = ?1 WHERE run_id = ?2",
            rusqlite::params![substituted, receipt.run_id.to_string()],
        )
    });
    connection
        .execute(
            "UPDATE events SET payload_json = ?1
             WHERE kind = 'run.executed'
               AND json_extract(payload_json, '$.runId') = ?2",
            rusqlite::params![substituted, receipt.run_id.to_string()],
        )
        .unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap_err();
    assert!(
        matches!(error, SupervisorError::RunEventMismatch(_)),
        "nested preflight substitution failed through the wrong guard: {error:?}"
    );
}

#[test]
fn startup_refuses_an_execution_event_with_an_unknown_serialized_outcome() {
    let (store, projects, repositories, checks) = enrolled("unknown-execution-outcome-event");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::GitUrl,
                source: "https://example.invalid/future-execution-outcome.git".to_owned(),
                base_revision: "a".repeat(40),
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::NotAdmissible);

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE events
                 SET payload_json = json_set(payload_json, '$.summary', 'future_outcome')
                 WHERE kind = 'run.executed'
                   AND json_extract(payload_json, '$.runId') = ?1",
                [receipt.run_id.to_string()],
            )
            .unwrap(),
        1
    );
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("an unknown execution outcome Event opened Ready");
    assert!(
        matches!(error, SupervisorError::RunEventMismatch(_)),
        "the execution owner returned the wrong typed replay error: {error:?}"
    );
}

#[test]
fn startup_replay_requires_not_admissible_to_carry_a_refusing_preflight() {
    if !enforcing_or_skip() {
        return;
    }

    // Start with a real v2 refusal whose embedded preflight is Verified. Relabel
    // the mutually consistent Receipt row and event as NotAdmissible while
    // preserving the nested preflight bytes: startup must reject that pairing.
    let (repository, revision) = fixture_repository("startup-verified-not-admissible");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("startup-verified-not-admissible");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    fs::write(repository.path.join(".git/config"), "[malformed\n").unwrap();
    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::WorktreeUnavailable);
    assert_eq!(receipt.admitted_by.outcome, PreflightOutcome::Verified);
    assert!(receipt.checks.is_empty());
    assert_eq!(receipt.worktree, None);

    let connection = Connection::open(&store.path).unwrap();
    let stored: String = connection
        .query_row(
            "SELECT receipt_json FROM receipts WHERE run_id = ?1",
            [receipt.run_id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let impossible = stored.replacen(
        "\"summary\":\"worktree_unavailable\"",
        "\"summary\":\"not_admissible\"",
        1,
    );
    assert_ne!(
        impossible, stored,
        "fixture did not locate the top-level summary"
    );
    replace_receipt_update_trigger(&connection, |connection| {
        connection.execute(
            "UPDATE receipts
             SET outcome = 'not_admissible', receipt_json = ?1
             WHERE run_id = ?2",
            rusqlite::params![impossible, receipt.run_id.to_string()],
        )
    });
    connection
        .execute(
            "UPDATE events SET payload_json = ?1
             WHERE kind = 'run.executed'
               AND json_extract(payload_json, '$.runId') = ?2",
            rusqlite::params![impossible, receipt.run_id.to_string()],
        )
        .unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap_err();
    assert!(
        matches!(error, SupervisorError::RunEventMismatch(_)),
        "NotAdmissible with a Verified preflight failed through the wrong guard: {error:?}"
    );

    // A genuine NotAdmissible Receipt carries the refusal that prevented
    // dispatch, and the same startup replay path accepts it as Ready.
    let (repository, _revision) = fixture_repository("startup-refusing-not-admissible");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("startup-refusing-not-admissible");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: "b".repeat(40),
                declared_checks: vec![check("never", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    let receipt = checks.execute(project.id, bounds()).unwrap();
    assert_eq!(receipt.summary, ExecutionSummary::NotAdmissible);
    assert_ne!(receipt.admitted_by.outcome, PreflightOutcome::Verified);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
}

#[test]
fn startup_requires_exactly_one_execution_event_for_every_terminal_execution() {
    if !enforcing_or_skip() {
        return;
    }
    for tamper in ["duplicate-event", "missing-event"] {
        let (repository, revision) = fixture_repository(tamper);
        let source = repository.path.to_string_lossy().into_owned();
        let (store, projects, repositories, checks) = enrolled(tamper);
        let project = projects.create(project_draft()).unwrap();
        repositories
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source,
                    base_revision: revision,
                    declared_checks: vec![check("ok", &["/bin/true"], ".")],
                },
            )
            .unwrap();
        let receipt = checks.execute(project.id, bounds()).unwrap();

        let connection = Connection::open(&store.path).unwrap();
        if tamper == "duplicate-event" {
            connection
                .execute(
                    // R58: a deliberate duplicate, planted around the append path and saying so.
                    "INSERT INTO events (
                         workspace_id, kind, payload_json, occurred_at_ms,
                         predecessor_kind, predecessor_digest, link_digest
                     )
                     SELECT workspace_id, kind, payload_json, occurred_at_ms,
                            'unchained', NULL, NULL
                     FROM events WHERE kind = 'run.executed'",
                    [],
                )
                .unwrap();
        } else {
            connection
                .execute("DELETE FROM events WHERE kind = 'run.executed'", [])
                .unwrap();
        }
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        let SupervisorError::RunStateMismatch { run_id, .. } = &error else {
            panic!("{tamper} failed through the wrong guard: {error:?}");
        };
        assert_eq!(run_id, &receipt.run_id.to_string());
    }
}

#[test]
fn startup_rejects_execution_relabelled_as_preflight_without_a_preflight_event() {
    // R44: BOTH ENDS. Run ids are random UUIDs, so which end the tampered Run lands on is
    // luck -- measured: one sampling had it interior (both end-mutants died) and the next
    // had it last. Two runs, targeting the first- and last-sorting `declared_checks` Run,
    // make it structural: a mutant naming one fixed end is wrong in at least one of them.
    for end in ["last", "first"] {
        if !enforcing_or_skip() {
            return;
        }
        let (repository, revision) = fixture_repository(&format!("relabelled-execution-{end}"));
        let source = repository.path.to_string_lossy().into_owned();
        let (store, projects, repositories, checks) =
            enrolled(&format!("relabelled-execution-{end}"));
        let project = projects.create(project_draft()).unwrap();
        repositories
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source,
                    base_revision: revision,
                    declared_checks: vec![check("ok", &["/bin/true"], ".")],
                },
            )
            .unwrap();
        // R39.3 (kimi's HOLD): TWO Runs, and the tampered one sorts LAST under the pass's own
        // `ORDER BY runs.workspace_id, runs.id`. With one Run "names the failing row" and "names row
        // 0" are the same claim, and a mutant naming `rows[0]` survives.
        let receipts = [
            checks.execute(project.id, bounds()).unwrap(),
            checks.execute(project.id, bounds()).unwrap(),
        ];
        let connection = Connection::open(&store.path).unwrap();
        // R44.6: CALL the pass's own enumeration rather than copying its query. The pass walks every
        // Run; this bar's tamper only fits a `declared_checks` one, so the filter stays here while the
        // ORDER is the pass's.
        let declared: Vec<String> = beacond::enumerate_runs(&connection)
            .expect("the pass's enumeration reads")
            .into_iter()
            .map(|row| row.1)
            .filter(|id| {
                connection
                    .query_row("SELECT kind FROM runs WHERE id = ?1", [id], |row| {
                        row.get::<_, String>(0)
                    })
                    .map(|kind| kind == "declared_checks")
                    .unwrap_or(false)
            })
            .collect();
        let tampered: String = if end == "last" {
            declared.last()
        } else {
            declared.first()
        }
        .expect("the fixture recorded two Runs")
        .clone();
        let receipt = receipts
            .iter()
            .find(|receipt| receipt.run_id.to_string() == tampered)
            .expect("the last-sorting Run is one the fixture made");
        replace_run_update_trigger(&connection, |connection| {
            connection.execute(
                "UPDATE runs SET kind = 'check_preflight' WHERE id = ?1",
                [receipt.run_id.to_string()],
            )
        });
        connection
            .execute(
                "DELETE FROM events
             WHERE kind = 'run.executed'
               AND json_extract(payload_json, '$.runId') = ?1",
                [receipt.run_id.to_string()],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        let SupervisorError::RunStateMismatch { run_id, .. } = &error else {
            panic!("expected a named Run-state refusal, got {error:?}");
        };
        assert_eq!(run_id, &receipt.run_id.to_string());
        // The LINE, not just the variant: R28.4's operator message carries the row and the owning
        // reader's fault NAME, so a Display that drops either is caught here rather than by reading.
        // This bar and not the state-column one below, because this tamper is what reaches the pass.
        let line = error.to_string();
        assert!(
            line.contains(&receipt.run_id.to_string()) && line.contains("InvalidStore"),
            "the operator's line names neither the Run nor the fault it hit: {line}"
        );
    }
}

#[test]
fn startup_refuses_execution_only_state_on_a_preflight_run() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("preflight-incomplete");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, checks) = enrolled("preflight-incomplete");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision,
                declared_checks: vec![check("ok", &["/bin/true"], ".")],
            },
        )
        .unwrap();
    checks.execute(project.id, bounds()).unwrap();

    let connection = Connection::open(&store.path).unwrap();
    // Read the row's own id before tampering, so the assertion below compares the refusal against
    // the store rather than against a name the test made up.
    let tampered_run_id: String = connection
        .query_row(
            "SELECT id FROM runs WHERE kind = 'check_preflight'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    replace_run_update_trigger(&connection, |connection| {
        connection.execute(
            "UPDATE runs SET state = 'incomplete' WHERE kind = 'check_preflight'",
            [],
        )
    });
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap_err();
    // THE LOG WALK REACHES THIS FIRST, and that is the honest state, not a weaker refusal:
    // `verify_persisted_events` is `inspect_store`'s first line, so every `run.preflighted` event
    // is replayed against its Run before any state pass runs. This tamper moves only the `state`
    // column, which preserves the cardinalities `verify_run_state` compares, so the replay is the
    // guard that can see it. What R28 buys here is that the replay now NAMES the Run: before, its
    // refusal and the state pass's shared one nameless variant.
    let SupervisorError::RunEventStateMismatch { run_id, .. } = &error else {
        panic!("expected a named Run-event state refusal, got {error:?}");
    };
    assert_eq!(run_id.to_string(), tampered_run_id);
    let line = error.to_string();
    assert!(
        line.contains(&tampered_run_id) && line.contains("Receipt"),
        "the operator's line does not name the Run whose Receipt disagrees: {line}"
    );
}

#[test]
fn startup_fails_closed_when_execution_representations_disagree() {
    if !enforcing_or_skip() {
        return;
    }
    for tamper in [
        "sql-version",
        "sql-outcome",
        "run-state",
        "run-kind",
        "receipt-json",
        "event-json",
        "nested-both",
        "unknown-version-both",
    ] {
        let (repository, revision) = fixture_repository(tamper);
        let source = repository.path.to_string_lossy().into_owned();
        let (store, projects, repositories, checks) = enrolled(tamper);
        let project = projects.create(project_draft()).unwrap();
        repositories
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source,
                    base_revision: revision,
                    declared_checks: vec![check("ok", &["/bin/true"], ".")],
                },
            )
            .unwrap();
        let receipt = checks.execute(project.id, bounds()).unwrap();

        let connection = Connection::open(&store.path).unwrap();
        match tamper {
            "sql-version"
            | "sql-outcome"
            | "receipt-json"
            | "nested-both"
            | "unknown-version-both" => {
                replace_receipt_update_trigger(&connection, |connection| {
                    match tamper {
                        "sql-version" => connection.execute(
                            "UPDATE receipts SET version = 1 WHERE run_id = ?1",
                            [receipt.run_id.to_string()],
                        ),
                        "sql-outcome" => connection.execute(
                            "UPDATE receipts SET outcome = 'not_admissible' WHERE run_id = ?1",
                            [receipt.run_id.to_string()],
                        ),
                        "receipt-json" => connection.execute(
                            "UPDATE receipts
                         SET receipt_json = json_set(receipt_json, '$.notVerified', json_array('tampered'))
                         WHERE run_id = ?1",
                            [receipt.run_id.to_string()],
                        ),
                        "nested-both" => connection.execute(
                            "UPDATE receipts
                         SET receipt_json = json_set(receipt_json, '$.admittedBy.outcome', 'future_outcome')
                         WHERE run_id = ?1",
                            [receipt.run_id.to_string()],
                        ),
                        "unknown-version-both" => connection.execute(
                            "UPDATE receipts
                         SET receipt_json = json_set(receipt_json, '$.version', 99)
                         WHERE run_id = ?1",
                            [receipt.run_id.to_string()],
                        ),
                        _ => unreachable!(),
                    }
                });
                if tamper == "nested-both" {
                    connection
                        .execute(
                            "UPDATE events
                             SET payload_json = json_set(payload_json, '$.admittedBy.outcome', 'future_outcome')
                             WHERE kind = 'run.executed'",
                            [],
                        )
                        .unwrap();
                } else if tamper == "unknown-version-both" {
                    connection
                        .execute(
                            "UPDATE events
                             SET payload_json = json_set(payload_json, '$.version', 99)
                             WHERE kind = 'run.executed'",
                            [],
                        )
                        .unwrap();
                }
            }
            "run-state" | "run-kind" => {
                replace_run_update_trigger(&connection, |connection| {
                    if tamper == "run-state" {
                        connection.execute(
                            "UPDATE runs SET state = 'refused' WHERE id = ?1",
                            [receipt.run_id.to_string()],
                        )
                    } else {
                        connection.execute(
                            "UPDATE runs SET kind = 'check_preflight' WHERE id = ?1",
                            [receipt.run_id.to_string()],
                        )
                    }
                });
            }
            "event-json" => {
                connection
                    .execute(
                        "UPDATE events
                         SET payload_json = json_set(payload_json, '$.notVerified', json_array('tampered'))
                         WHERE kind = 'run.executed'",
                        [],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        assert!(
            matches!(error, SupervisorError::RunEventMismatch(_)),
            "{tamper} failed through the wrong guard: {error:?}"
        );
    }
}

#[test]
fn migration_preserves_real_v1_bytes_through_rollback_and_forward_startup() {
    if !enforcing_or_skip() {
        return;
    }
    let (repository, revision) = fixture_repository("v1-migration");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, preflights) = enrolled_v15("v1-migration");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source,
                base_revision: revision.clone(),
                declared_checks: vec![check("real", &["/bin/cat", "marker.txt"], ".")],
            },
        )
        .unwrap();
    let admitted_by = preflights.preflight(project.id).unwrap();
    drop(preflights);
    drop(repositories);
    drop(projects);
    downgrade_execution_schema_to_v15(&store.path);
    let worktree = Worktree::create(&repository.path, &revision).unwrap();
    let record = run_bounded_for_test(
        &["/bin/cat".to_owned(), "marker.txt".to_owned()],
        worktree.path(),
        bounds(),
        None,
    );
    assert_eq!(record.outcome, ExecutionOutcome::Exited { code: 0 });
    assert_eq!(record.stdout.trim(), "committed-content");
    let cleanup = worktree.close();
    assert_eq!(cleanup, CleanupOutcome::Removed {});

    let run_id = Uuid::new_v4();
    let started_at_ms = admitted_by.ended_at_ms + 1;
    let ended_at_ms = started_at_ms + record.elapsed_ms.max(1);
    let exact_preflight_json = serde_json::to_string(&admitted_by).unwrap();
    let normalized_preflight_json = serde_json::to_value(&admitted_by).unwrap().to_string();
    let v1_payload = serde_json::json!({
        "version": 1,
        "runId": run_id,
        "workspaceId": admitted_by.workspace_id,
        "projectId": project.id,
        "summary": "completed",
        "admittedBy": admitted_by,
        "checks": [record],
        "worktree": cleanup,
        "startedAtMs": started_at_ms,
        "endedAtMs": ended_at_ms,
        "notVerified": [
            "base revision reachability from a ref (object presence verified only)",
            "check execution, exit statuses, and their effects (preflight never runs a check)",
            "CPU, memory, and wall-clock bounds (the sandbox contains, it does not meter)",
            "UDP egress (Landlock restricts TCP only)"
        ]
    })
    .to_string();
    let normalized_nested = format!("\"admittedBy\":{normalized_preflight_json}");
    let exact_nested = format!("\"admittedBy\":{exact_preflight_json}");
    let v1_payload = v1_payload.replacen(&normalized_nested, &exact_nested, 1);
    assert!(
        v1_payload.contains(&exact_nested),
        "fixture did not preserve the original preflight Receipt bytes"
    );
    let connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'declared_checks', ?4, ?5, 'verified', ?6, ?7, ?8)",
            rusqlite::params![
                admitted_by.workspace_id.to_string(),
                run_id.to_string(),
                project.id.to_string(),
                i64::from(admitted_by.repository_revision),
                i64::from(admitted_by.charter_revision),
                i64::try_from(ended_at_ms).unwrap(),
                i64::try_from(started_at_ms).unwrap(),
                i64::try_from(ended_at_ms).unwrap(),
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO receipts (
                 workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
             ) VALUES (?1, ?2, 1, 'completed', ?3, ?4)",
            rusqlite::params![
                admitted_by.workspace_id.to_string(),
                run_id.to_string(),
                v1_payload,
                i64::try_from(ended_at_ms).unwrap(),
            ],
        )
        .unwrap();
    connection
        .execute(
            // R58: this plant runs against a store ROLLED BACK to v15, where `events` has no chain
            // columns. The chain's vocabulary belongs only where the chain's schema is.
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'run.executed', ?2, ?3)",
            rusqlite::params![
                admitted_by.workspace_id.to_string(),
                v1_payload,
                i64::try_from(ended_at_ms).unwrap(),
            ],
        )
        .unwrap();

    connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
    connection
        .execute_batch(include_str!("../migrations/0016_execution_receipt_v2.sql"))
        .unwrap();
    assert_eq!(schema_version(&connection), 16);
    assert_v1_bytes(&connection, run_id, &v1_payload);
    connection.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(schema_version(&connection), 15);
    assert_v1_bytes(&connection, run_id, &v1_payload);
    drop(connection);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.schema_version, 35);
    let connection = Connection::open(&store.path).unwrap();
    assert_v1_bytes(&connection, run_id, &v1_payload);
}

fn schema_version(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn assert_v1_bytes(connection: &Connection, run_id: Uuid, expected: &str) {
    let receipt: (i64, String) = connection
        .query_row(
            "SELECT version, receipt_json FROM receipts WHERE run_id = ?1",
            [run_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'run.executed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receipt.0, 1);
    assert_eq!(receipt.1.as_bytes(), expected.as_bytes());
    assert_eq!(event.as_bytes(), expected.as_bytes());
}

fn replace_receipt_update_trigger(
    connection: &Connection,
    tamper: impl FnOnce(&Connection) -> rusqlite::Result<usize>,
) {
    let trigger_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'trigger' AND name = 'receipts_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER receipts_no_update;")
        .unwrap();
    tamper(connection).unwrap();
    connection.execute_batch(&trigger_sql).unwrap();
}

fn replace_run_update_trigger(
    connection: &Connection,
    tamper: impl FnOnce(&Connection) -> rusqlite::Result<usize>,
) {
    let trigger_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'runs_single_terminal_transition'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER runs_single_terminal_transition;")
        .unwrap();
    tamper(connection).unwrap();
    connection.execute_batch(&trigger_sql).unwrap();
}

/// R45.1: the public Run identity is a UUID, so startup must refuse a stored primary key that the
/// owning reader cannot represent. The long case also proves the operator line is bounded.
#[test]
fn sol1_startup_refuses_untyped_run_identity_edges() {
    for (label, run_id) in [
        ("empty", String::new()),
        ("one-megabyte", "r".repeat(1 << 20)),
    ] {
        let (repository, revision) = fixture_repository(label);
        let source = repository.path.to_string_lossy().into_owned();
        let (store, projects, repositories, _checks) = enrolled(label);
        let project = projects.create(project_draft()).unwrap();
        repositories
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source,
                    base_revision: revision,
                    declared_checks: vec![check("ok", &["/bin/true"], ".")],
                },
            )
            .unwrap();
        let connection = Connection::open(&store.path).unwrap();
        let workspace_id: String = connection
            .query_row("SELECT id FROM workspaces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            connection
                .execute(
                    "INSERT INTO runs (
                        workspace_id, id, project_id, kind, repository_revision, charter_revision,
                        state, lease_expires_at_ms, started_at_ms, ended_at_ms
                     ) VALUES (?1, ?2, ?3, 'declared_checks', 1, 1, 'unconfirmed', 0, 0, 0)",
                    rusqlite::params![workspace_id, run_id, project.id.to_string()],
                )
                .unwrap(),
            1,
            "{label}: the edge row did not plant"
        );
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .expect_err("startup accepted an unrepresentable Run identity");
        let SupervisorError::StoredIdentityUnreadable { what, raw } = &error else {
            panic!("{label}: the Run identity was refused under the wrong name: {error:?}");
        };
        assert_eq!(*what, "a Run row's identity is not readable");
        assert!(
            raw.ends_with(&format!("Run {run_id}")),
            "{label}: the typed reader fault lost the raw Run key"
        );
        let line = error.to_string();
        assert!(
            !line.contains('\r') && !line.contains('\n') && !line.contains('\u{1b}'),
            "{label}: the one-line refusal obeyed stored control bytes: {line:?}"
        );
        assert!(
            line.chars().count() <= 256,
            "{label}: the one-line refusal was not bounded: {} characters",
            line.chars().count()
        );
        // R42 amended what this rung requires. "the line reports it truncated N characters" is a
        // claim about a bounded DISCLOSURE of stored text; the projection now carries none, so
        // there is nothing left to bound. The line is instead held to the whole closed
        // projection, which is the same for every edge here -- and retention is already asserted
        // above, on the TYPED `raw` field, which is where it belongs.
        assert_eq!(
            line, "a Run row's identity is not readable: StoredIdentityUnreadable",
            "{label}: the one-line refusal is not the closed projection"
        );
    }
}

#[test]
fn astra_run_identity_precedes_lease_state_judgement() {
    for (label, run_id) in [
        ("empty", String::new()),
        ("one-megabyte", "r".repeat(1 << 20)),
    ] {
        let (repository, revision) = fixture_repository(label);
        let source = repository.path.to_string_lossy().into_owned();
        let (store, projects, repositories, _checks) = enrolled(label);
        let project = projects.create(project_draft()).unwrap();
        repositories
            .bind(
                project.id,
                project.charter.revision,
                None,
                RepositoryDraft {
                    source_kind: RepositorySourceKind::LocalPath,
                    source,
                    base_revision: revision,
                    declared_checks: vec![check("ok", &["/bin/true"], ".")],
                },
            )
            .unwrap();
        let connection = Connection::open(&store.path).unwrap();
        let workspace_id: String = connection
            .query_row("SELECT id FROM workspaces", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            connection
                .execute(
                    "INSERT INTO runs (
                        workspace_id, id, project_id, kind, repository_revision, charter_revision,
                        state, lease_expires_at_ms, started_at_ms, ended_at_ms
                     ) VALUES (?1, ?2, ?3, 'declared_checks', 1, 1, 'preflighting', 0, 0, NULL)",
                    rusqlite::params![workspace_id, run_id, project.id.to_string()],
                )
                .unwrap(),
            1,
            "{label}: the edge row did not plant"
        );
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .expect_err("startup accepted an unrepresentable Run identity");
        let SupervisorError::StoredIdentityUnreadable { what, raw } = &error else {
            panic!("{label}: the Run identity was refused under the wrong name: {error:?}");
        };
        assert_eq!(*what, "a Run row's identity is not readable");
        assert!(
            raw.ends_with(&format!("Run {run_id}")),
            "{label}: the typed reader fault lost the raw Run key"
        );
        let connection = Connection::open(&store.path).unwrap();
        let after: (String, Option<i64>) = connection
            .query_row(
                "SELECT state, ended_at_ms FROM runs WHERE workspace_id = ?1 AND id = ?2",
                rusqlite::params![workspace_id, run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            after,
            ("preflighting".to_owned(), None),
            "R45.RUN.IDENTITY_BEFORE_LEASE_STATE: refusing an unrepresentable Run changed its stored state"
        );
        let line = error.to_string();
        assert!(
            !line.contains('\r') && !line.contains('\n') && !line.contains('\u{1b}'),
            "{label}: the one-line refusal obeyed stored control bytes: {line:?}"
        );
        assert!(
            line.chars().count() <= 256,
            "{label}: the one-line refusal was not bounded: {} characters",
            line.chars().count()
        );
        // R42 amended what this rung requires. "the line reports it truncated N characters" is a
        // claim about a bounded DISCLOSURE of stored text; the projection now carries none, so
        // there is nothing left to bound. The line is instead held to the whole closed
        // projection, which is the same for every edge here -- and retention is already asserted
        // above, on the TYPED `raw` field, which is where it belongs.
        assert_eq!(
            line, "a Run row's identity is not readable: StoredIdentityUnreadable",
            "{label}: the one-line refusal is not the closed projection"
        );
    }
}
