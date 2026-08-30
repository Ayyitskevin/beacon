use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use beacond::{
    AutonomyProfile, DeclaredCheck, InvalidRepositoryDraft, OperatorAccountService,
    OperatorSessionPhase, ProjectCharterDraft, ProjectDraft, ProjectRepositoryError,
    ProjectRepositoryService, ProjectService, RepositoryAuthor, RepositoryDraft,
    RepositorySourceKind, SecretString, StartupConfig, Supervisor, SupervisorError,
};
use rusqlite::Connection;
use uuid::Uuid;

/// A fabricated Repository Binding: a plausible local path and a synthetic commit id, neither of
/// which names anything that exists.
///
/// ADR 0026 makes a Binding a pure declaration, so these tests never open the directory, never run
/// `git`, and never run a declared check — confirming that a commit exists is a liveness claim this
/// slice deliberately does not make. Nothing here needs a real repository, and ADR 0028 requires
/// that a fixture be constructed by the test rather than borrowed from the machine that wrote it.
///
/// The revision is patterned rather than random so that no reader mistakes it for a commit that
/// could be looked up somewhere.
const SAMPLE_SOURCE: &str = "/home/operator/projects/sample-tracker";
const SAMPLE_BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-project-repository-{label}-{}.sqlite3",
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

fn enrolled_services(
    label: &str,
) -> (
    TestStore,
    Arc<OperatorAccountService>,
    ProjectService,
    ProjectRepositoryService,
) {
    let store = TestStore::new(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
    let recovery = accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert!(!recovery.expose().is_empty());
    assert_eq!(
        accounts.complete_operator_setup(true).unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    (
        store,
        Arc::clone(&accounts),
        ProjectService::new(Arc::clone(&accounts), report.workspace.id),
        ProjectRepositoryService::new(accounts, report.workspace.id),
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

fn pytest_check() -> DeclaredCheck {
    DeclaredCheck {
        name: "pytest".to_owned(),
        argv: vec![
            "pytest".to_owned(),
            "tests/test_invariants.py".to_owned(),
            "-q".to_owned(),
        ],
        working_directory: ".".to_owned(),
    }
}

fn ruff_check() -> DeclaredCheck {
    DeclaredCheck {
        name: "ruff".to_owned(),
        argv: vec!["ruff".to_owned(), "check".to_owned(), ".".to_owned()],
        working_directory: ".".to_owned(),
    }
}

fn repository_draft() -> RepositoryDraft {
    RepositoryDraft {
        source_kind: RepositorySourceKind::LocalPath,
        source: SAMPLE_SOURCE.to_owned(),
        base_revision: SAMPLE_BASE_REVISION.to_owned(),
        declared_checks: vec![pytest_check()],
    }
}

fn event_count(store: &TestStore) -> i64 {
    let connection = Connection::open(&store.path).unwrap();
    connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'project.repository_bound'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
}

#[test]
fn operator_binds_first_repository_revision_to_the_current_charter() {
    let (store, _accounts, projects, repositories) = enrolled_services("bind");
    let project = projects.create(project_draft()).unwrap();

    let repository = repositories
        .bind(
            project.id,
            project.charter.revision,
            None,
            repository_draft(),
        )
        .unwrap();

    assert_eq!(repository.workspace_id, project.workspace_id);
    assert_eq!(repository.project_id, project.id);
    assert_eq!(repository.revision, 1);
    assert_eq!(repository.charter_revision, project.charter.revision);
    assert_eq!(repository.source_kind, RepositorySourceKind::LocalPath);
    assert_eq!(repository.source, SAMPLE_SOURCE);
    assert_eq!(repository.base_revision, SAMPLE_BASE_REVISION);
    assert_eq!(repository.declared_checks, vec![pytest_check()]);
    assert_eq!(repository.author_kind, RepositoryAuthor::Operator);
    assert!(repository.applies_to_current_charter);
    assert_eq!(
        repositories.current(project.id).unwrap(),
        Some(repository.clone())
    );
    assert_eq!(
        repositories.history(project.id).unwrap(),
        vec![repository.clone()]
    );

    let connection = Connection::open(&store.path).unwrap();
    let event = connection
        .query_row(
            "SELECT json_extract(payload_json, '$.project.id'),
                    json_extract(payload_json, '$.repository.revision'),
                    json_extract(payload_json, '$.repository.sourceKind'),
                    json_extract(payload_json, '$.repository.source'),
                    json_extract(payload_json, '$.repository.baseRevision'),
                    json_extract(payload_json, '$.repository.declaredChecks[0].name'),
                    json_extract(payload_json, '$.repository.authorKind'),
                    occurred_at_ms
             FROM events WHERE kind = 'project.repository_bound'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        event,
        (
            project.id.to_string(),
            1,
            "local_path".to_owned(),
            SAMPLE_SOURCE.to_owned(),
            SAMPLE_BASE_REVISION.to_owned(),
            "pytest".to_owned(),
            "operator".to_owned(),
            i64::try_from(repository.authored_at_ms).unwrap(),
        )
    );
}

#[test]
fn repository_commands_authenticate_before_reading_or_validating_arguments() {
    let (_store, accounts, projects, repositories) = enrolled_services("locked");
    let project = projects.create(project_draft()).unwrap();
    accounts.lock().unwrap();

    let mut malformed = repository_draft();
    malformed.declared_checks.clear();
    assert!(matches!(
        repositories.current(Uuid::nil()),
        Err(ProjectRepositoryError::Account(_))
    ));
    assert!(matches!(
        repositories.history(Uuid::nil()),
        Err(ProjectRepositoryError::Account(_))
    ));
    assert!(matches!(
        repositories.bind(project.id, 0, None, malformed),
        Err(ProjectRepositoryError::Account(_))
    ));
}

#[test]
fn invalid_repository_drafts_are_refused_without_partial_state() {
    let (store, _accounts, projects, repositories) = enrolled_services("refusals");
    let project = projects.create(project_draft()).unwrap();

    let mut short_revision = repository_draft();
    short_revision.base_revision.truncate(39);
    let mut uppercase_revision = repository_draft();
    uppercase_revision.base_revision = SAMPLE_BASE_REVISION.to_uppercase();
    let mut nonhex_revision = repository_draft();
    nonhex_revision.base_revision = "z".repeat(40);
    for draft in [short_revision, uppercase_revision, nonhex_revision] {
        assert!(matches!(
            repositories.bind(project.id, 1, None, draft),
            Err(ProjectRepositoryError::InvalidDraft(
                InvalidRepositoryDraft::BaseRevision
            ))
        ));
    }

    let mut relative_path = repository_draft();
    relative_path.source = "projects/sample-tracker".to_owned();
    let mut trailing_slash = repository_draft();
    trailing_slash.source = format!("{SAMPLE_SOURCE}/");
    let mut schemeless_url = repository_draft();
    schemeless_url.source_kind = RepositorySourceKind::GitUrl;
    schemeless_url.source = "git.example.invalid/operator/sample-tracker.git".to_owned();
    let mut empty_source = repository_draft();
    empty_source.source = String::new();
    for draft in [relative_path, trailing_slash, schemeless_url, empty_source] {
        assert!(matches!(
            repositories.bind(project.id, 1, None, draft),
            Err(ProjectRepositoryError::InvalidDraft(
                InvalidRepositoryDraft::Source
            ))
        ));
    }

    let mut no_checks = repository_draft();
    no_checks.declared_checks.clear();
    let mut duplicate_name = repository_draft();
    duplicate_name.declared_checks.push(pytest_check());
    let mut empty_name = repository_draft();
    empty_name.declared_checks[0].name = String::new();
    let mut shouted_name = repository_draft();
    shouted_name.declared_checks[0].name = "PyTest".to_owned();
    let mut empty_argv = repository_draft();
    empty_argv.declared_checks[0].argv.clear();
    let mut empty_argument = repository_draft();
    empty_argument.declared_checks[0].argv.push(String::new());
    let mut shell_string = repository_draft();
    shell_string.declared_checks[0].argv = vec!["pytest tests -q && rm -rf /".to_owned()];
    shell_string.declared_checks[0].argv[0].push('\0');
    let mut absolute_directory = repository_draft();
    absolute_directory.declared_checks[0].working_directory = "/etc".to_owned();
    let mut escaping_directory = repository_draft();
    escaping_directory.declared_checks[0].working_directory = "tests/../..".to_owned();
    let mut empty_directory = repository_draft();
    empty_directory.declared_checks[0].working_directory = String::new();
    for draft in [
        no_checks,
        duplicate_name,
        empty_name,
        shouted_name,
        empty_argv,
        empty_argument,
        shell_string,
        absolute_directory,
        escaping_directory,
        empty_directory,
    ] {
        assert!(matches!(
            repositories.bind(project.id, 1, None, draft),
            Err(ProjectRepositoryError::InvalidDraft(
                InvalidRepositoryDraft::DeclaredChecks
            ))
        ));
    }

    assert!(matches!(
        repositories.bind(Uuid::new_v4(), 1, None, repository_draft()),
        Err(ProjectRepositoryError::ProjectNotFound)
    ));
    assert!(repositories.current(project.id).unwrap().is_none());
    assert!(repositories.history(project.id).unwrap().is_empty());
    assert_eq!(event_count(&store), 0);
}

#[test]
fn a_git_url_binding_is_accepted_without_contacting_the_remote() {
    let (_store, _accounts, projects, repositories) = enrolled_services("git-url");
    let project = projects.create(project_draft()).unwrap();

    let mut draft = repository_draft();
    draft.source_kind = RepositorySourceKind::GitUrl;
    draft.source = "https://git.example.invalid/operator/sample-tracker.git".to_owned();

    let repository = repositories.bind(project.id, 1, None, draft).unwrap();

    assert_eq!(repository.source_kind, RepositorySourceKind::GitUrl);
    assert_eq!(
        repository.source,
        "https://git.example.invalid/operator/sample-tracker.git"
    );
}

#[test]
fn declared_checks_are_stored_in_canonical_name_order() {
    let (_store, _accounts, projects, repositories) = enrolled_services("canonical-order");
    let project = projects.create(project_draft()).unwrap();

    let mut noncanonical = repository_draft();
    noncanonical.declared_checks = vec![ruff_check(), pytest_check()];

    let repository = repositories
        .bind(project.id, 1, None, noncanonical)
        .unwrap();

    assert_eq!(
        repository.declared_checks,
        vec![pytest_check(), ruff_check()]
    );
    assert_eq!(
        repositories
            .current(project.id)
            .unwrap()
            .unwrap()
            .declared_checks,
        vec![pytest_check(), ruff_check()]
    );
}

#[test]
fn repository_revision_is_stale_safe_and_charter_changes_require_rebinding() {
    let (_store, _accounts, projects, repositories) = enrolled_services("revision");
    let project = projects.create(project_draft()).unwrap();
    let first = repositories
        .bind(project.id, 1, None, repository_draft())
        .unwrap();

    let revised_project = projects
        .revise_charter(
            project.id,
            1,
            ProjectCharterDraft {
                purpose: "Build a trustworthy local control plane.".to_owned(),
                ..project_draft().charter
            },
        )
        .unwrap();
    assert_eq!(revised_project.charter.revision, 2);
    let stale_view = repositories.current(project.id).unwrap().unwrap();
    assert!(!stale_view.applies_to_current_charter);
    assert_eq!(stale_view.revision, 1);
    assert_eq!(repositories.history(project.id).unwrap().len(), 1);
    assert!(matches!(
        repositories.bind(project.id, 1, Some(1), repository_draft()),
        Err(ProjectRepositoryError::StaleCharterRevision {
            expected: 1,
            current: 2
        })
    ));

    let second = repositories
        .bind(project.id, 2, Some(first.revision), repository_draft())
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(second.charter_revision, 2);
    assert!(second.applies_to_current_charter);
    assert!(matches!(
        repositories.bind(project.id, 2, Some(1), repository_draft()),
        Err(ProjectRepositoryError::StaleRepositoryRevision)
    ));
    assert!(matches!(
        repositories.bind(project.id, 2, Some(2), repository_draft()),
        Err(ProjectRepositoryError::UnchangedRepository)
    ));
    assert_eq!(repositories.history(project.id).unwrap().len(), 2);
}

#[test]
fn concurrent_bindings_from_the_same_revision_commit_exactly_once() {
    let (store, accounts, projects, first_service) = enrolled_services("concurrent");
    let project = projects.create(project_draft()).unwrap();
    let second_service = ProjectRepositoryService::new(Arc::clone(&accounts), project.workspace_id);
    let barrier = Arc::new(Barrier::new(3));

    let first_barrier = Arc::clone(&barrier);
    let project_id = project.id;
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_service.bind(project_id, 1, None, repository_draft())
    });
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_service.bind(project_id, 1, None, repository_draft())
    });

    barrier.wait();
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ProjectRepositoryError::StaleRepositoryRevision)))
            .count(),
        1
    );
    let repositories = ProjectRepositoryService::new(accounts, project.workspace_id);
    assert_eq!(repositories.history(project.id).unwrap().len(), 1);
    assert_eq!(event_count(&store), 1);
}

#[test]
fn restart_refuses_repository_state_without_its_exact_event() {
    let (store, accounts, projects, repositories) = enrolled_services("missing-event");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(project.id, 1, None, repository_draft())
        .unwrap();
    drop(repositories);
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "DELETE FROM events WHERE kind = 'project.repository_bound'",
            [],
        )
        .unwrap();
    drop(connection);

    let result = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        matches!(result, Err(SupervisorError::ProjectRepositoryStateMismatch)),
        "unexpected bootstrap result: {result:?}"
    );
}

#[test]
fn restart_refuses_a_repository_event_whose_evidence_no_longer_matches_state() {
    let (store, accounts, projects, repositories) = enrolled_services("event-mismatch");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(project.id, 1, None, repository_draft())
        .unwrap();
    drop(repositories);
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(
                 payload_json,
                 '$.repository.baseRevision',
                 '0123456789abcdef0123456789abcdef01234567'
             )
             WHERE kind = 'project.repository_bound'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(SupervisorError::ProjectRepositoryEventMismatch(_))
    ));
}

#[test]
fn restart_refuses_noncanonical_project_repository_state() {
    let (store, accounts, projects, repositories) = enrolled_services("noncanonical-state");
    let project = projects.create(project_draft()).unwrap();
    let mut draft = repository_draft();
    draft.declared_checks = vec![pytest_check(), ruff_check()];
    repositories.bind(project.id, 1, None, draft).unwrap();
    drop(repositories);
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch("DROP TRIGGER project_repository_revisions_no_update")
        .unwrap();
    connection
        .execute(
            "UPDATE project_repository_revisions
             SET declared_checks_json = json_array(
                 json_extract(declared_checks_json, '$[1]'),
                 json_extract(declared_checks_json, '$[0]')
             )",
            [],
        )
        .unwrap();
    connection
        .execute_batch(concat!(
            "CREATE TRIGGER project_repository_revisions_no_update\n",
            "BEFORE UPDATE ON project_repository_revisions\n",
            "BEGIN\n",
            "    SELECT RAISE(ABORT, 'Project Repository revisions are immutable');\n",
            "END;"
        ))
        .unwrap();
    drop(connection);

    let result = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        matches!(
            result,
            Err(SupervisorError::ProjectRepositoryEventMismatch(_))
        ),
        "unexpected bootstrap result: {result:?}"
    );
}

#[test]
fn only_the_latest_repository_revision_can_apply_to_the_current_charter() {
    let (_store, _accounts, projects, repositories) = enrolled_services("current-revision");
    let project = projects.create(project_draft()).unwrap();
    repositories
        .bind(project.id, 1, None, repository_draft())
        .unwrap();
    let mut revised = repository_draft();
    revised.base_revision = "0123456789abcdef0123456789abcdef01234567".to_owned();
    repositories.bind(project.id, 1, Some(1), revised).unwrap();

    let history = repositories.history(project.id).unwrap();
    assert_eq!(history.len(), 2);
    assert!(!history[0].applies_to_current_charter);
    assert!(history[1].applies_to_current_charter);
    assert_eq!(
        repositories.current(project.id).unwrap().unwrap().revision,
        2
    );
}
