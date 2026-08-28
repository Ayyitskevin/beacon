use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use beacond::{
    AutonomyProfile, BudgetBoundary, ExternalEffects, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectError, ProjectService, SecretString, StartupConfig,
    Supervisor,
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
                "beacond-projects-{label}-{}.sqlite3",
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

fn enrolled_service(label: &str) -> (TestStore, Arc<OperatorAccountService>, ProjectService) {
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
    let projects = ProjectService::new(Arc::clone(&accounts), report.workspace.id);
    (store, accounts, projects)
}

fn draft(name: &str, profile: AutonomyProfile) -> ProjectDraft {
    ProjectDraft {
        name: name.to_owned(),
        charter: ProjectCharterDraft {
            purpose: "Build a trustworthy local software product.".to_owned(),
            desired_outcomes: vec![
                "Ship a verified vertical slice".to_owned(),
                "Preserve operator control".to_owned(),
            ],
            constraints: vec!["Keep Workspace data local".to_owned()],
            prohibitions: vec!["Do not deploy publicly".to_owned()],
            autonomy_profile: profile,
        },
    }
}

#[test]
fn owner_creates_and_reads_an_immutable_first_charter() {
    let (store, accounts, projects) = enrolled_service("create");
    assert!(projects.list().unwrap().is_empty());

    let created = projects
        .create(ProjectDraft {
            name: "  Beacon Alpha  ".to_owned(),
            charter: ProjectCharterDraft {
                purpose: "  Build a trustworthy local software product.  ".to_owned(),
                desired_outcomes: vec!["  Ship a verified vertical slice  ".to_owned()],
                constraints: vec!["  Keep Workspace data local  ".to_owned()],
                prohibitions: vec!["  Do not deploy publicly  ".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        })
        .unwrap();

    assert_eq!(created.name, "Beacon Alpha");
    assert_eq!(created.charter.revision, 1);
    assert_eq!(
        created.charter.purpose,
        "Build a trustworthy local software product."
    );
    assert_eq!(
        created.charter.desired_outcomes,
        ["Ship a verified vertical slice"]
    );
    assert_eq!(
        created.charter.autonomy_profile,
        AutonomyProfile::Supervised
    );
    assert_eq!(created.charter.budget_boundary, BudgetBoundary::NoSpend);
    assert_eq!(
        created.charter.external_effects,
        ExternalEffects::Prohibited
    );
    assert_eq!(projects.get(created.id).unwrap(), created);

    let summaries = projects.list().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, created.id);
    assert_eq!(summaries[0].charter_revision, 1);
    assert_eq!(summaries[0].autonomy_profile, AutonomyProfile::Supervised);

    drop(projects);
    drop(accounts);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.persisted_event_count, 2);
}

#[test]
fn project_commands_require_an_active_owner_session() {
    let (_store, accounts, projects) = enrolled_service("locked");
    accounts.lock().unwrap();

    assert!(matches!(projects.list(), Err(ProjectError::Account(_))));
    assert!(matches!(
        projects.create(draft("Refused", AutonomyProfile::Observe)),
        Err(ProjectError::Account(_))
    ));
}

#[test]
fn invalid_project_or_charter_input_is_refused_without_partial_state() {
    let (_store, _accounts, projects) = enrolled_service("invalid");
    let mut cases = Vec::new();

    cases.push(ProjectDraft {
        name: "   ".to_owned(),
        ..draft("valid", AutonomyProfile::Observe)
    });
    cases.push(ProjectDraft {
        name: "line\nbreak".to_owned(),
        ..draft("valid", AutonomyProfile::Observe)
    });
    cases.push(ProjectDraft {
        name: "x".repeat(121),
        ..draft("valid", AutonomyProfile::Observe)
    });
    let mut empty_purpose = draft("Empty purpose", AutonomyProfile::Observe);
    empty_purpose.charter.purpose = "   ".to_owned();
    cases.push(empty_purpose);
    let mut no_outcome = draft("No outcome", AutonomyProfile::Observe);
    no_outcome.charter.desired_outcomes.clear();
    cases.push(no_outcome);
    let mut too_many_outcomes = draft("Too many", AutonomyProfile::Observe);
    too_many_outcomes.charter.desired_outcomes = vec!["Outcome".to_owned(); 21];
    cases.push(too_many_outcomes);
    let mut duplicate_outcome = draft("Duplicate", AutonomyProfile::Observe);
    duplicate_outcome.charter.desired_outcomes =
        vec!["One result".to_owned(), "  One result  ".to_owned()];
    cases.push(duplicate_outcome);
    let mut control_entry = draft("Control", AutonomyProfile::Observe);
    control_entry.charter.constraints = vec!["No hidden\u{0000}state".to_owned()];
    cases.push(control_entry);

    for invalid in cases {
        assert!(projects.create(invalid).is_err());
        assert!(projects.list().unwrap().is_empty());
    }
}

#[test]
fn owner_can_choose_observe_and_unknown_project_ids_fail_closed() {
    let (_store, _accounts, projects) = enrolled_service("observe");
    let created = projects
        .create(draft("Research only", AutonomyProfile::Observe))
        .unwrap();

    assert_eq!(created.charter.autonomy_profile, AutonomyProfile::Observe);
    assert!(matches!(
        projects.get(Uuid::new_v4()),
        Err(ProjectError::NotFound)
    ));
}

#[test]
fn restart_refuses_project_state_without_its_creation_event() {
    let (store, accounts, projects) = enrolled_service("missing-event");
    projects
        .create(draft("Audited", AutonomyProfile::Supervised))
        .unwrap();
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute("DELETE FROM events WHERE kind = 'project.created'", [])
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::ProjectStateMismatch)
    ));
}

#[test]
fn restart_refuses_a_creation_event_with_a_different_timestamp() {
    let (store, accounts, projects) = enrolled_service("event-time");
    projects
        .create(draft("Audited", AutonomyProfile::Supervised))
        .unwrap();
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "UPDATE events SET occurred_at_ms = occurred_at_ms + 1
             WHERE kind = 'project.created'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::ProjectEventMismatch(_))
    ));
}

#[test]
fn restart_refuses_a_discontinuous_charter_history() {
    let (store, accounts, projects) = enrolled_service("charter-gap");
    let created = projects
        .create(draft("Audited", AutonomyProfile::Supervised))
        .unwrap();
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO project_charter_revisions (
                workspace_id, project_id, revision, purpose, desired_outcomes_json,
                constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                external_effects, author_kind, authored_at_ms
             ) VALUES (?1, ?2, 3, 'Changed purpose', '[\"Changed outcome\"]', '[]', '[]',
                       'observe', 'no_spend', 'prohibited', 'owner', ?3)",
            rusqlite::params![
                created.workspace_id.to_string(),
                created.id.to_string(),
                i64::try_from(created.created_at_ms + 1).unwrap()
            ],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::ProjectStateMismatch)
    ));
}

#[test]
fn restart_refuses_a_current_charter_pointer_behind_the_terminal_revision() {
    let (store, accounts, projects) = enrolled_service("stale-charter-pointer");
    let created = projects
        .create(draft("Audited", AutonomyProfile::Supervised))
        .unwrap();
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO project_charter_revisions (
                workspace_id, project_id, revision, purpose, desired_outcomes_json,
                constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                external_effects, author_kind, authored_at_ms
             ) VALUES (?1, ?2, 2, 'Changed purpose', '[\"Changed outcome\"]', '[]', '[]',
                       'observe', 'no_spend', 'prohibited', 'owner', ?3)",
            rusqlite::params![
                created.workspace_id.to_string(),
                created.id.to_string(),
                i64::try_from(created.created_at_ms + 1).unwrap()
            ],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::ProjectStateMismatch)
    ));
}
