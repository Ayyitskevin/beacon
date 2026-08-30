//! Running a Project's Declared Checks — the composition of ADR 0027 and 0029.
//!
//! These tests run real commands, in real Git worktrees, under a real sandbox.
//! The point of the slice is that three separately-proven pieces compose, so a
//! test that stubbed any of them would prove nothing.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, CleanupOutcome, DeclaredCheck, DeclaredCheckService, ExecutionBounds,
    ExecutionOutcome, ExecutionSummary, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectRepositoryService, ProjectService, RepositoryDraft,
    RepositorySourceKind, SecretString, StartupConfig, Supervisor, detect_sandbox,
};
use rusqlite::Connection;
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

    let receipt = checks.execute(project.id, bounds()).unwrap();

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
    assert!(receipt.checks.is_empty(), "a refused Run ran nothing");
    assert_eq!(receipt.worktree, None, "no worktree was created");
    // The refusal is still a Run with a Receipt: it happened, and it is recorded.
    let executions = runs_of_kind(&store, "declared_checks");
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].1, "refused");
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry.contains("preflight did not admit"))
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
