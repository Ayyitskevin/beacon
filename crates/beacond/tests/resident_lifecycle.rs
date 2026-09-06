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

/// Two Residents in one Workspace, so a startup bar can tell "the failing row" from "row 0".
fn admit_two_residents(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
) -> (StarterOrganizationService, Vec<Uuid>) {
    let organization = StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id);
    let accepted = organization
        .accept(
            1,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: Some("Northstar".to_owned()),
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: Some("Southstar".to_owned()),
                },
            ],
        )
        .unwrap();
    let ids = accepted.residents.iter().map(|r| r.id).collect();
    (organization, ids)
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
    // R44: BOTH ENDS. Tampering only the last-sorting Resident made "names the failing
    // row" and "names the last row" the same assertion; `rows.last()` survived it.
    for (end, order) in [("last", "DESC"), ("first", "ASC")] {
        for (label, tamper, expected_fault) in [
            (
                "agent-state",
                "UPDATE agents SET lifecycle = 'suspended' WHERE kind = 'resident' AND id = ?1",
                "InvalidStore",
            ),
            (
                "missing-event",
                "DELETE FROM events WHERE kind = 'resident.activated'
             AND payload_json LIKE '%' || ?1 || '%'",
                "InvalidStore",
            ),
        ] {
            let store = TestStore::new(&format!("{label}-{end}"));
            let accounts = enrolled_accounts(&store);
            // R39.3 (kimi's HOLD): TWO Residents, so "names the failing row" and "names row 0" are
            // different claims. With one Resident a mutant naming `residents[0].id` is
            // indistinguishable from the real thing -- kimi measured exactly that surviving.
            let (_organization, resident_ids) = admit_two_residents(&store, &accounts);
            let service = ResidentLifecycleService::new(accounts, store.workspace_id);
            for resident_id in &resident_ids {
                service.activate(*resident_id, None).unwrap();
            }
            drop(service);

            let connection = Connection::open(&store.path).unwrap();
            // This pass enumerates `ORDER BY agent.workspace_id, agent.id`; the tamper hits whichever
            // Resident sorts LAST, read back from the store rather than guessed from the ids.
            let resident_id: Uuid = connection
                .query_row(
                    // R44: BOTH ENDS -- one run per end of this pass's own ORDER BY.
                    &format!(
                        "SELECT id FROM agents WHERE kind = 'resident' ORDER BY id {order} LIMIT 1"
                    ),
                    [],
                    |row| row.get::<_, String>(0),
                )
                .map(|id| Uuid::parse_str(&id).unwrap())
                .expect("the fixture admitted two Residents");
            assert!(resident_ids.contains(&resident_id));
            let touched = connection
                .execute(tamper, rusqlite::params![resident_id.to_string()])
                .unwrap();
            assert_eq!(touched, 1, "{label}: the plant touched the wrong row count");
            drop(connection);

            let error = Supervisor::bootstrap(StartupConfig {
                store_path: store.path.clone(),
                initial_workspace_name: "Ignored".to_owned(),
            })
            .unwrap_err();
            // Which guard reaches this depends on the tamper: the log walk runs first, so an Event
            // tamper is refused there. When the state pass IS the refuser, it must name the Resident
            // and end in a fault NAME -- the variant alone let three mutants through (pass 10's
            // lesson, applied late).
            match &error {
                SupervisorError::ResidentLifecycleStateMismatch {
                    resident_id: named, ..
                } => {
                    assert_eq!(*named, resident_id, "{label}: the wrong Resident was named");
                    let line = error.to_string();
                    assert!(
                        line.contains(&resident_id.to_string()),
                        "{label}: the line does not name the Resident: {line}"
                    );
                    let tail = line.rsplit(": ").next().unwrap();
                    assert_eq!(tail, expected_fault, "{label}: wrong fault name: {line}");
                    // blue's HOLD: the carried reader's OWN sentence never reaches the line (R28.4).
                    assert!(
                        !line.contains("Resident lifecycle state is invalid"),
                        "{label}: the reader's own sentence reached the line: {line}"
                    );
                }
                SupervisorError::ResidentLifecycleEventMismatch(_) => {}
                other => panic!("{label} failed through the wrong guard: {other:?}"),
            }
        }
    }
}

/// R45.1: the service allocates positive decision sequences; a stored negative ordering key is
/// not an alternative history and must be refused under the reader's typed fault.
#[test]
fn sol1_startup_refuses_minimum_lifecycle_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(accounts, store.workspace_id);
    service.activate(resident_id, None).unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'resident_lifecycle_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER resident_lifecycle_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE resident_lifecycle_decisions SET sequence = ?1 WHERE resident_id = ?2",
                rusqlite::params![i64::MIN, resident_id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MIN sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted lifecycle sequence i64::MIN as Ready");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the lifecycle ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a Resident lifecycle decision's ordering key is not readable"
    );
    assert!(raw.contains(&resident_id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, i64::MIN);
}

#[test]
fn astra_startup_refuses_zero_lifecycle_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(accounts, store.workspace_id);
    service.activate(resident_id, None).unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'resident_lifecycle_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER resident_lifecycle_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE resident_lifecycle_decisions SET sequence = ?1 WHERE resident_id = ?2",
                rusqlite::params![0_i64, resident_id.to_string()],
            )
            .unwrap(),
        1,
        "the 0_i64 sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("R45.LIFECYCLE.ZERO_REFUSED");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the lifecycle ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a Resident lifecycle decision's ordering key is not readable"
    );
    assert!(raw.contains(&resident_id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, 0, "R45.LIFECYCLE.ZERO_KEY");
    assert!(
        error.to_string().contains("sequence 0"),
        "R45.LIFECYCLE.ZERO_LINE_KEY"
    );
}

#[test]
fn astra_startup_accepts_maximum_lifecycle_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(accounts, store.workspace_id);
    service.activate(resident_id, None).unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'resident_lifecycle_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER resident_lifecycle_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE resident_lifecycle_decisions SET sequence = ?1 WHERE resident_id = ?2",
                rusqlite::params![i64::MAX, resident_id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MAX sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let result = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        result.is_ok(),
        "R45.LIFECYCLE.MAX_POSITIVE_ORDER_KEY: {result:?}"
    );
}

#[test]
fn astra_minimum_lifecycle_sequence_outward_key_is_exact() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let (_organization, resident_id) = admit_resident(&store, &accounts);
    let service = ResidentLifecycleService::new(accounts, store.workspace_id);
    service.activate(resident_id, None).unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'resident_lifecycle_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER resident_lifecycle_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE resident_lifecycle_decisions SET sequence = ?1 WHERE resident_id = ?2",
                rusqlite::params![i64::MIN, resident_id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MIN sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted lifecycle sequence i64::MIN as Ready");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the lifecycle ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a Resident lifecycle decision's ordering key is not readable"
    );
    assert!(raw.contains(&resident_id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, i64::MIN);
    let line = error.to_string();
    eprintln!("OUTWARD {line}");
    assert!(
        line.contains(&i64::MIN.to_string()),
        "R45.LIFECYCLE.MIN_LINE_KEY_EXACT: {line}"
    );
}
