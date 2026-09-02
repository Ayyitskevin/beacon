//! Check Preflight — ADR 0027.
//!
//! Preflight verifies and then refuses. These tests assert the refusal is typed,
//! that a negative answer, an error, and an unfinished step stay three separate
//! facts, and that every Receipt says what it did not verify.
//!
//! The product never spawns a process. These tests do, to build repositories
//! whose object layout only Git can produce.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, CheckPreflightError, CheckPreflightService, DeclaredCheck,
    OperatorAccountService, OperatorSessionPhase, PreflightOutcome, PreflightReceipt,
    ProjectCharterDraft, ProjectDraft, ProjectRepositoryService, ProjectService, Recorded,
    RepositoryDraft, RepositorySourceKind, SecretString, StartupConfig, Supervisor,
    SupervisorError, detect_sandbox,
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
                "beacond-check-preflight-{label}-{}.sqlite3",
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
        let path =
            std::env::temp_dir().join(format!("beacond-preflight-{label}-{}", Uuid::new_v4()));
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

/// A repository with one commit and a `checks/` subdirectory.
fn fixture_repository(label: &str) -> (TempDir, String) {
    let directory = TempDir::new(label);
    git(&directory.path, &["init", "--quiet"]);
    fs::create_dir_all(directory.path.join("checks")).unwrap();
    fs::write(directory.path.join("checks/keep.txt"), "kept").unwrap();
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

fn enrolled_services(
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
        CheckPreflightService::new(accounts, report.workspace.id),
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

fn check_at(working_directory: &str) -> DeclaredCheck {
    DeclaredCheck {
        name: "pytest".to_owned(),
        argv: vec!["pytest".to_owned(), "-q".to_owned()],
        working_directory: working_directory.to_owned(),
    }
}

fn draft(source_kind: RepositorySourceKind, source: &str, revision: &str) -> RepositoryDraft {
    RepositoryDraft {
        source_kind,
        source: source.to_owned(),
        base_revision: revision.to_owned(),
        declared_checks: vec![check_at(".")],
    }
}

/// Bind a repository to a fresh Project and preflight it.
fn bind_and_preflight(
    label: &str,
    repository_draft: RepositoryDraft,
) -> (TestStore, PreflightReceipt, Uuid) {
    let (store, projects, repositories, preflight) = enrolled_services(label);
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(project.id, project.charter.revision, None, repository_draft)
        .unwrap();
    let receipt = preflight.preflight(project.id).unwrap();
    (store, receipt, project.id)
}

fn runs(store: &TestStore) -> Vec<(String, String, Option<i64>)> {
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare("SELECT id, state, ended_at_ms FROM runs ORDER BY started_at_ms")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn preflight_events(store: &TestStore) -> Vec<String> {
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare("SELECT payload_json FROM events WHERE kind = 'run.preflighted' ORDER BY sequence")
        .unwrap();
    statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

const REACHABILITY_GAP: &str =
    "base revision reachability from a ref (object presence verified only)";

#[test]
fn a_binding_that_satisfies_every_step_is_admissible_when_the_sandbox_is_proven() {
    let (repository, revision) = fixture_repository("admissible");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, receipt, project_id) = bind_and_preflight(
        "admissible",
        draft(RepositorySourceKind::LocalPath, &source, &revision),
    );

    // Every verification step held. Whether that means execution is admissible
    // depends on what this host can prove it enforces, so the assertion follows
    // the capability rather than assuming one — a kernel without Landlock is a
    // legitimate host that simply may not run checks.
    let capability = detect_sandbox();
    if capability.admits_execution() {
        assert_eq!(receipt.outcome, PreflightOutcome::Verified);
        assert!(
            !receipt
                .not_verified
                .iter()
                .any(|entry| entry.starts_with("sandbox enforcement")),
            "a proven sandbox must leave the residual-uncertainty list"
        );
        // What the sandbox still does not bound stays named.
        assert!(
            receipt
                .not_verified
                .iter()
                .any(|entry| entry.contains("CPU, memory, and wall-clock"))
        );
        assert!(
            receipt
                .not_verified
                .iter()
                .any(|entry| entry.contains("UDP"))
        );
    } else {
        assert_eq!(receipt.outcome, PreflightOutcome::SandboxUnavailable);
        assert!(
            receipt
                .not_verified
                .iter()
                .any(|entry| entry.starts_with("sandbox enforcement"))
        );
    }
    // The Receipt records the capability it ran under, never the bare word that
    // preceded this slice.
    assert_eq!(receipt.sandbox_version, capability.receipt_version());
    assert_ne!(receipt.sandbox_version, "none");
    assert_eq!(receipt.version, 1);
    assert_eq!(receipt.project_id, project_id);
    assert_eq!(receipt.base_revision, revision);

    // The check resolved, so containment was decided on a real path.
    assert_eq!(receipt.resolved_checks.len(), 1);
    let resolved = &receipt.resolved_checks[0];
    assert_eq!(resolved.declared_working_directory, ".");
    assert!(matches!(
        resolved.resolved_working_directory,
        Recorded::Present { .. }
    ));

    // The Receipt names what it did not verify, including the reachability gap.
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry == REACHABILITY_GAP)
    );

    // Spec fields a preflight cannot honour are absent, never empty.
    assert!(matches!(
        receipt.unavailable.proposal_digest,
        Recorded::Absent { .. }
    ));
    assert!(matches!(
        receipt.unavailable.resulting_revision,
        Recorded::Absent { .. }
    ));
    assert!(matches!(
        receipt.unavailable.cost_usd_micros,
        Recorded::Absent { .. }
    ));

    // One Run, terminal and refused; one Receipt; one event carrying the whole
    // Receipt rather than a delta.
    let rows = runs(&store);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].1,
        if capability.admits_execution() {
            "verified"
        } else {
            "refused"
        }
    );
    assert!(rows[0].2.is_some());
    let events = preflight_events(&store);
    assert_eq!(events.len(), 1);
    let replayed: PreflightReceipt = serde_json::from_str(&events[0]).unwrap();
    assert_eq!(replayed, receipt);
}

#[test]
fn preflight_finishes_against_its_pinned_binding_and_the_next_run_pins_the_new_one() {
    let (repository, revision) = fixture_repository("binding-moved-during-preflight");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, preflight) =
        enrolled_services("binding-moved-during-preflight");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            draft(RepositorySourceKind::LocalPath, &source, &revision),
        )
        .unwrap();
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER supersede_binding_when_preflight_opens
             AFTER INSERT ON runs
             WHEN NEW.kind = 'check_preflight'
             BEGIN
                 INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind,
                     source, base_revision, declared_checks_json, author_kind, authored_at_ms
                 )
                 SELECT workspace_id, project_id, revision + 1, charter_revision, source_kind,
                        source, base_revision, declared_checks_json, author_kind, authored_at_ms
                 FROM project_repository_revisions
                 WHERE workspace_id = NEW.workspace_id
                   AND project_id = NEW.project_id
                   AND revision = NEW.repository_revision;
             END;",
        )
        .unwrap();
    drop(connection);

    let first = preflight.preflight(project.id).unwrap();

    assert_eq!(first.repository_revision, 1);
    assert_eq!(first.outcome, PreflightOutcome::Verified);

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute("DROP TRIGGER supersede_binding_when_preflight_opens", [])
        .unwrap();
    drop(connection);

    let second = preflight.preflight(project.id).unwrap();
    assert_eq!(second.repository_revision, 2);
    assert_eq!(second.outcome, PreflightOutcome::Verified);
}

#[test]
fn a_git_url_is_unsupported_and_no_filesystem_read_is_attempted() {
    let (_store, receipt, _project) = bind_and_preflight(
        "git-url",
        draft(
            RepositorySourceKind::GitUrl,
            "https://example.invalid/does-not-resolve.git",
            &"a".repeat(40),
        ),
    );
    assert_eq!(receipt.outcome, PreflightOutcome::SourceKindUnsupported);
    // The step is refused before the repository is touched, so nothing resolved.
    assert!(
        receipt
            .resolved_checks
            .iter()
            .all(|check| matches!(check.resolved_working_directory, Recorded::Absent { .. }))
    );
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry.contains("network fetch"))
    );
}

#[test]
fn republishing_the_charter_leaves_the_binding_stale() {
    let (repository, revision) = fixture_repository("stale-charter");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, preflight) = enrolled_services("stale-charter");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            draft(RepositorySourceKind::LocalPath, &source, &revision),
        )
        .unwrap();
    let revised = projects
        .revise_charter(
            project.id,
            project.charter.revision,
            ProjectCharterDraft {
                purpose: "Build a trustworthy local software product, revised.".to_owned(),
                desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
                constraints: vec!["Keep Workspace data local".to_owned()],
                prohibitions: vec!["Do not deploy publicly".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        )
        .unwrap();
    assert!(revised.charter.revision > project.charter.revision);

    let receipt = preflight.preflight(project.id).unwrap();
    assert_eq!(receipt.outcome, PreflightOutcome::BindingStaleForCharter);
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry.contains("repository readability"))
    );
}

#[test]
fn a_source_that_is_not_a_repository_is_unreadable() {
    let directory = TempDir::new("not-a-repository");
    let source = directory.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "not-a-repository",
        draft(RepositorySourceKind::LocalPath, &source, &"b".repeat(40)),
    );
    assert_eq!(receipt.outcome, PreflightOutcome::RepositoryUnreadable);
}

#[test]
fn a_dangling_gitdir_file_is_unreadable() {
    let directory = TempDir::new("dangling-gitdir");
    fs::write(directory.path.join(".git"), "gitdir: ../nowhere\n").unwrap();
    let source = directory.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "dangling-gitdir",
        draft(RepositorySourceKind::LocalPath, &source, &"c".repeat(40)),
    );
    assert_eq!(receipt.outcome, PreflightOutcome::RepositoryUnreadable);
}

#[test]
fn a_base_revision_absent_from_the_object_store_is_refused() {
    let (repository, _revision) = fixture_repository("absent-revision");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "absent-revision",
        draft(RepositorySourceKind::LocalPath, &source, &"d".repeat(40)),
    );
    assert_eq!(receipt.outcome, PreflightOutcome::BaseRevisionUnreachable);
    // Even a negative answer records the gap between presence and reachability.
    assert!(
        receipt
            .not_verified
            .iter()
            .any(|entry| entry == REACHABILITY_GAP)
    );
}

#[test]
fn a_base_revision_only_in_a_pack_still_satisfies_the_step() {
    let (repository, revision) = fixture_repository("packed-revision");
    git(&repository.path, &["gc", "--quiet", "--prune=now"]);
    let loose = repository
        .path
        .join(".git/objects")
        .join(&revision[..2])
        .join(&revision[2..]);
    assert!(!loose.exists(), "fixture did not pack the commit");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "packed-revision",
        draft(RepositorySourceKind::LocalPath, &source, &revision),
    );
    // The step under test is base-revision presence, so this asserts that the
    // run reached the end rather than which terminal outcome it reached.
    assert!(matches!(
        receipt.outcome,
        PreflightOutcome::Verified | PreflightOutcome::SandboxUnavailable
    ));
}

#[test]
fn a_lexically_clean_working_directory_that_resolves_outside_is_an_escape() {
    // Binding already refuses `..`, absolute paths, and backslashes, so a
    // *lexical* escape cannot be declared. The remaining way out is a symlink:
    // a path that is clean as written and resolves elsewhere. This is why
    // containment is decided on the resolved path and never on the declared
    // string — a guard that trusts the value the guarded path supplies can be
    // defeated by supplying a different one. The two layers are complementary,
    // and neither alone is sufficient.
    let (repository, revision) = fixture_repository("escape");
    let outside = TempDir::new("escape-target");
    std::os::unix::fs::symlink(&outside.path, repository.path.join("checks/elsewhere")).unwrap();
    let source = repository.path.to_string_lossy().into_owned();
    let mut repository_draft = draft(RepositorySourceKind::LocalPath, &source, &revision);
    repository_draft.declared_checks = vec![check_at("checks/elsewhere")];
    let (_store, receipt, _project) = bind_and_preflight("escape", repository_draft);
    assert_eq!(receipt.outcome, PreflightOutcome::WorkingDirectoryEscapes);
}

#[test]
fn binding_refuses_a_lexical_escape_before_preflight_ever_sees_it() {
    // The complement of the test above: the declaration layer holds too.
    let (repository, revision) = fixture_repository("lexical-escape");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, projects, repositories, _preflight) = enrolled_services("lexical-escape");
    let project = projects.create(project_draft()).unwrap();
    let mut repository_draft = draft(RepositorySourceKind::LocalPath, &source, &revision);
    repository_draft.declared_checks = vec![check_at("../..")];
    assert!(
        repositories
            .bind(project.id, project.charter.revision, None, repository_draft)
            .is_err()
    );
}

#[test]
fn a_working_directory_that_does_not_exist_is_an_error_not_an_escape() {
    let (repository, revision) = fixture_repository("missing-directory");
    let source = repository.path.to_string_lossy().into_owned();
    let mut repository_draft = draft(RepositorySourceKind::LocalPath, &source, &revision);
    repository_draft.declared_checks = vec![check_at("does-not-exist")];
    let (_store, receipt, _project) = bind_and_preflight("missing-directory", repository_draft);
    // Nothing was found outside anything; the declaration is simply wrong.
    assert_eq!(receipt.outcome, PreflightOutcome::PreflightFailed);
}

#[test]
fn a_corrupt_pack_index_fails_rather_than_answering_no() {
    use std::os::unix::fs::PermissionsExt;

    let (repository, revision) = fixture_repository("corrupt-index");
    git(&repository.path, &["gc", "--quiet", "--prune=now"]);
    let pack = repository.path.join(".git/objects/pack");
    let index = fs::read_dir(&pack)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|extension| extension == "idx"))
        .expect("gc produced a pack index");
    let mut permissions = fs::metadata(&index).unwrap().permissions();
    permissions.set_mode(0o644);
    fs::set_permissions(&index, permissions).unwrap();
    fs::write(&index, b"not an index").unwrap();

    let source = repository.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "corrupt-index",
        draft(RepositorySourceKind::LocalPath, &source, &revision),
    );
    // An unreadable store is an error, never "the revision is absent".
    assert_eq!(receipt.outcome, PreflightOutcome::PreflightFailed);
    assert_ne!(receipt.outcome, PreflightOutcome::BaseRevisionUnreachable);
}

#[test]
fn a_project_without_a_binding_has_nothing_to_preflight() {
    let (store, projects, _repositories, preflight) = enrolled_services("unbound");
    let project = projects.create(project_draft()).unwrap();
    assert!(matches!(
        preflight.preflight(project.id),
        Err(CheckPreflightError::RepositoryNotBound)
    ));
    // No Run was opened: there was nothing to preflight.
    assert!(runs(&store).is_empty());
}

#[test]
fn an_expired_lease_leaves_the_run_unconfirmed_rather_than_absent() {
    let (repository, revision) = fixture_repository("expired-lease");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, projects, repositories, _preflight) = enrolled_services("expired-lease");
    let project = projects.create(project_draft()).unwrap();
    let bound = repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            draft(RepositorySourceKind::LocalPath, &source, &revision),
        )
        .unwrap();

    // A Run whose process died mid-inspection: opened, never terminal.
    let abandoned = Uuid::new_v4();
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (
                 (SELECT id FROM workspaces LIMIT 1), ?1, ?2, 'check_preflight', ?3, ?4,
                 'preflighting', 1, 1, NULL
             )",
            rusqlite::params![
                abandoned.to_string(),
                project.id.to_string(),
                i64::from(bound.revision),
                i64::from(bound.charter_revision),
            ],
        )
        .unwrap();
    drop(connection);

    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();

    let rows = runs(&store);
    assert_eq!(rows.len(), 1, "the Run must survive reconciliation");
    assert_eq!(rows[0].0, abandoned.to_string());
    assert_eq!(rows[0].1, "unconfirmed");
    assert!(rows[0].2.is_some(), "an unconfirmed Run has an end");
}

#[test]
fn receipts_are_immutable_and_runs_advance_only_once() {
    let (repository, revision) = fixture_repository("immutable");
    let source = repository.path.to_string_lossy().into_owned();
    let (store, receipt, _project) = bind_and_preflight(
        "immutable",
        draft(RepositorySourceKind::LocalPath, &source, &revision),
    );
    let connection = Connection::open(&store.path).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE receipts SET outcome = 'verified' WHERE run_id = ?1",
                [receipt.run_id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM receipts WHERE run_id = ?1",
                [receipt.run_id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM runs WHERE id = ?1",
                [receipt.run_id.to_string()],
            )
            .is_err()
    );
    // A terminal Run cannot be reopened.
    assert!(
        connection
            .execute(
                "UPDATE runs SET state = 'preflighting', ended_at_ms = NULL WHERE id = ?1",
                [receipt.run_id.to_string()],
            )
            .is_err()
    );
}

#[test]
fn a_receipt_round_trips_and_rejects_an_unknown_field() {
    let (repository, revision) = fixture_repository("round-trip");
    let source = repository.path.to_string_lossy().into_owned();
    let (_store, receipt, _project) = bind_and_preflight(
        "round-trip",
        draft(RepositorySourceKind::LocalPath, &source, &revision),
    );
    let json = serde_json::to_string(&receipt).unwrap();
    let decoded: PreflightReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, receipt);

    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("surprise".to_owned(), serde_json::Value::Bool(true));
    assert!(serde_json::from_value::<PreflightReceipt>(value).is_err());

    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    value.as_object_mut().unwrap().remove("notVerified");
    assert!(
        serde_json::from_value::<PreflightReceipt>(value).is_err(),
        "a Receipt without notVerified is not evidence"
    );
}

#[test]
fn startup_reconciles_every_preflight_identity_field_with_its_run_row() {
    for field in [
        "workspaceId",
        "projectId",
        "repositoryRevision",
        "policyRepositoryRevision",
        "charterRevision",
        "policyCharterRevision",
        "startedAtMs",
        "endedAtMs",
    ] {
        let (store, receipt, _project) = bind_and_preflight(
            &format!("run-row-{field}"),
            draft(
                RepositorySourceKind::GitUrl,
                "https://example.invalid/replay-identity.git",
                &"a".repeat(40),
            ),
        );
        let replacement = match field {
            "workspaceId" | "projectId" => rusqlite::types::Value::Text(Uuid::new_v4().to_string()),
            "repositoryRevision" | "policyRepositoryRevision" => {
                rusqlite::types::Value::Integer(i64::from(receipt.repository_revision) + 1)
            }
            "charterRevision" | "policyCharterRevision" => {
                rusqlite::types::Value::Integer(i64::from(receipt.charter_revision) + 1)
            }
            "startedAtMs" => rusqlite::types::Value::Integer(
                i64::try_from(receipt.started_at_ms.saturating_sub(1)).unwrap(),
            ),
            "endedAtMs" => rusqlite::types::Value::Integer(
                i64::try_from(receipt.ended_at_ms.saturating_add(1)).unwrap(),
            ),
            _ => unreachable!(),
        };
        let json_path = format!("$.{field}");
        let connection = Connection::open(&store.path).unwrap();
        replace_receipt_update_trigger(&connection, |connection| {
            connection.execute(
                "UPDATE receipts
                 SET receipt_json = json_set(receipt_json, ?1, ?2)
                 WHERE run_id = ?3",
                rusqlite::params![json_path, replacement, receipt.run_id.to_string()],
            )
        });
        connection
            .execute(
                "UPDATE events
                 SET payload_json = json_set(payload_json, ?1, ?2)
                 WHERE kind = 'run.preflighted'
                   AND json_extract(payload_json, '$.runId') = ?3",
                rusqlite::params![json_path, replacement, receipt.run_id.to_string()],
            )
            .unwrap();
        if field == "endedAtMs" {
            connection
                .execute(
                    "UPDATE events SET occurred_at_ms = ?1
                     WHERE kind = 'run.preflighted'
                       AND json_extract(payload_json, '$.runId') = ?2",
                    rusqlite::params![replacement, receipt.run_id.to_string()],
                )
                .unwrap();
        }
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        assert!(
            matches!(error, SupervisorError::RunEventMismatch(_)),
            "{field} disagreed with the run row but failed through the wrong guard: {error:?}"
        );
    }
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
