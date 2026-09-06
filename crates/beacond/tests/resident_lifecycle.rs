use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use beacond::{
    OperatorAccountService, OperatorSessionPhase, ResidentLifecycle, ResidentLifecycleError,
    ResidentLifecycleService, SecretString, StarterOrganizationService, StarterResidentDraft,
    StarterRole, StartupConfig, Supervisor, SupervisorError,
};
use rusqlite::Connection;
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
    workspace_id: Uuid,
}

impl TestStore {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "beacond-resident-lifecycle-{label}-{}.sqlite3",
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
    accounts
}

fn admit_resident(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
) -> (StarterOrganizationService, Uuid) {
    let organization = StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id);
    let accepted = organization
        .accept(
            1,
            vec![StarterResidentDraft {
                role: StarterRole::Lead,
                display_name: Some("Northstar".to_owned()),
            }],
        )
        .unwrap();
    (organization, accepted.residents[0].id)
}

#[test]
fn operator_controls_the_full_resident_lifecycle_without_rewriting_admission() {
    let store = TestStore::new("full-lifecycle");
    let accounts = enrolled_accounts(&store);
    let (organization, resident_id) = admit_resident(&store, &accounts);
    let accepted_snapshot = organization.view().unwrap();
    let service = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);

    let initial = service.list().unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].id, resident_id);
    assert_eq!(initial[0].lifecycle, ResidentLifecycle::Inactive);
    assert_eq!(initial[0].latest_decision_id, None);

    let active = service.activate(resident_id, None).unwrap();
    assert_eq!(active.lifecycle, ResidentLifecycle::Active);
    let first_decision = active.latest_decision_id.unwrap();

    let suspended = service.suspend(resident_id, Some(first_decision)).unwrap();
    assert_eq!(suspended.lifecycle, ResidentLifecycle::Suspended);
    let second_decision = suspended.latest_decision_id.unwrap();
    assert!(matches!(
        service.suspend(resident_id, Some(first_decision)),
        Err(ResidentLifecycleError::StaleDecision)
    ));

    let active_again = service
        .reactivate(resident_id, Some(second_decision))
        .unwrap();
    assert_eq!(active_again.lifecycle, ResidentLifecycle::Active);
    let third_decision = active_again.latest_decision_id.unwrap();

    let retired = service.retire(resident_id, Some(third_decision)).unwrap();
    assert_eq!(retired.lifecycle, ResidentLifecycle::Retired);
    let retirement_id = retired.latest_decision_id.unwrap();
    assert!(matches!(
        service.reactivate(resident_id, Some(retirement_id)),
        Err(ResidentLifecycleError::InvalidTransition)
    ));

    assert_eq!(organization.view().unwrap(), accepted_snapshot);
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM resident_lifecycle_decisions",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind LIKE 'resident.%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT lifecycle FROM agents WHERE id = ?1",
                [resident_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "retired"
    );
    assert!(
        connection
            .execute(
                "UPDATE resident_lifecycle_decisions SET decided_by = 'operator' WHERE id = ?1",
                [retirement_id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM resident_lifecycle_decisions WHERE id = ?1",
                [retirement_id.to_string()],
            )
            .is_err()
    );
    drop(connection);
    drop(service);
    drop(organization);
    drop(accounts);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.persisted_event_count, 6);
}

#[test]
fn retirement_can_close_an_inactive_identity_and_is_permanent() {
    let store = TestStore::new("retire-inactive");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(accounts, store.workspace_id);

    let retired = service.retire(resident_id, None).unwrap();

    assert_eq!(retired.lifecycle, ResidentLifecycle::Retired);
    assert!(matches!(
        service.activate(resident_id, retired.latest_decision_id),
        Err(ResidentLifecycleError::InvalidTransition)
    ));
}

#[test]
fn locked_unknown_and_illegal_commands_fail_without_a_decision() {
    let store = TestStore::new("refusals");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);

    assert!(matches!(
        service.suspend(resident_id, None),
        Err(ResidentLifecycleError::InvalidTransition)
    ));
    assert!(matches!(
        service.activate(Uuid::new_v4(), None),
        Err(ResidentLifecycleError::NotFound)
    ));
    accounts.lock().unwrap();
    assert!(matches!(
        service.list(),
        Err(ResidentLifecycleError::Account(_))
    ));
    assert!(matches!(
        service.activate(resident_id, None),
        Err(ResidentLifecycleError::Account(_))
    ));

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM resident_lifecycle_decisions",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        0
    );
}

#[test]
fn concurrent_activation_has_one_winner_and_one_stale_command() {
    let store = TestStore::new("concurrent");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = Arc::new(ResidentLifecycleService::new(accounts, store.workspace_id));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let service = Arc::clone(&service);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                service.activate(resident_id, None)
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ResidentLifecycleError::StaleDecision)))
            .count(),
        1
    );
}

#[test]
fn startup_refuses_tampered_current_state_or_missing_lifecycle_event() {
    for (label, tamper) in [
        (
            "agent-state",
            "UPDATE agents SET lifecycle = 'suspended' WHERE kind = 'resident'",
        ),
        (
            "missing-event",
            "DELETE FROM events WHERE kind = 'resident.activated'",
        ),
    ] {
        let store = TestStore::new(label);
        let accounts = enrolled_accounts(&store);
        let (_organization, resident_id) = admit_resident(&store, &accounts);
        let service = ResidentLifecycleService::new(accounts, store.workspace_id);
        service.activate(resident_id, None).unwrap();
        drop(service);

        let connection = Connection::open(&store.path).unwrap();
        connection.execute(tamper, []).unwrap();
        drop(connection);

        assert!(matches!(
            Supervisor::bootstrap(StartupConfig {
                store_path: store.path.clone(),
                initial_workspace_name: "Ignored".to_owned(),
            }),
            Err(SupervisorError::ResidentLifecycleStateMismatch { .. })
                | Err(SupervisorError::ResidentLifecycleEventMismatch(_))
        ));
    }
}
