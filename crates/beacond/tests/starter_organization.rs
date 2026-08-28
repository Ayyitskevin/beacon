use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use beacond::{
    OperatorAccountService, OperatorSessionPhase, ResidentLifecycle, SecretString,
    StarterOrganizationDecision, StarterOrganizationError, StarterOrganizationService,
    StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
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
                "beacond-starter-organization-{label}-{}.sqlite3",
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

fn enrolled_service(
    label: &str,
) -> (
    TestStore,
    Arc<OperatorAccountService>,
    StarterOrganizationService,
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
    let organization = StarterOrganizationService::new(Arc::clone(&accounts), report.workspace.id);
    (store, accounts, organization)
}

#[test]
fn fresh_workspace_proposes_three_residents_and_keeps_researcher_available() {
    let (_store, accounts, organization) = enrolled_service("proposal");

    let view = organization.view().unwrap();

    assert_eq!(view.proposal_version, 1);
    assert_eq!(view.decision, StarterOrganizationDecision::Proposed);
    assert!(view.residents.is_empty());
    assert_eq!(
        view.templates
            .iter()
            .map(|template| (template.role, template.recommended))
            .collect::<Vec<_>>(),
        [
            (StarterRole::Lead, true),
            (StarterRole::Builder, true),
            (StarterRole::Reviewer, true),
            (StarterRole::Researcher, false),
        ]
    );

    accounts.lock().unwrap();
    assert!(matches!(
        organization.view(),
        Err(StarterOrganizationError::Account(_))
    ));
}

#[test]
fn owner_accepts_an_edited_roster_as_inactive_residents_once() {
    let (store, accounts, organization) = enrolled_service("accept");

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
                    display_name: None,
                },
                StarterResidentDraft {
                    role: StarterRole::Reviewer,
                    display_name: Some("Proof".to_owned()),
                },
            ],
        )
        .unwrap();

    assert_eq!(accepted.decision, StarterOrganizationDecision::Accepted);
    assert_eq!(accepted.residents.len(), 3);
    assert_eq!(accepted.residents[0].role, StarterRole::Lead);
    assert_eq!(
        accepted.residents[0].display_name.as_deref(),
        Some("Northstar")
    );
    assert_eq!(accepted.residents[1].display_name, None);
    assert!(
        accepted
            .residents
            .iter()
            .all(|resident| resident.lifecycle == ResidentLifecycle::Inactive)
    );
    assert!(matches!(
        organization.accept(
            1,
            vec![StarterResidentDraft {
                role: StarterRole::Researcher,
                display_name: None,
            }]
        ),
        Err(StarterOrganizationError::DecisionAlreadyRecorded)
    ));

    drop(organization);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO agents (
                id, workspace_id, kind, admission_source, role, display_name_state,
                display_name, lifecycle, created_at_ms, updated_at_ms
             ) SELECT ?1, id, 'resident', 'later_manual_admission', 'lead', 'absent',
                      '', 'inactive', 1, 1
               FROM workspaces",
            params![Uuid::new_v4().to_string()],
        )
        .unwrap();
    drop(connection);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.persisted_event_count, 2);
}

#[test]
fn owner_discards_the_proposal_without_creating_a_resident() {
    let (store, _accounts, organization) = enrolled_service("discard");

    let discarded = organization.discard(1).unwrap();

    assert_eq!(discarded.decision, StarterOrganizationDecision::Discarded);
    assert!(discarded.residents.is_empty());
    assert!(matches!(
        organization.accept(
            1,
            vec![StarterResidentDraft {
                role: StarterRole::Lead,
                display_name: None,
            }]
        ),
        Err(StarterOrganizationError::DecisionAlreadyRecorded)
    ));

    drop(organization);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.persisted_event_count, 2);
}

#[test]
fn invalid_edits_leave_the_proposal_undecided() {
    let (_store, _accounts, organization) = enrolled_service("invalid-edits");

    let invalid_attempts = [
        (
            organization.accept(2, vec![draft(StarterRole::Lead, None)]),
            "unsupported version",
        ),
        (organization.accept(1, vec![]), "empty roster"),
        (
            organization.accept(
                1,
                vec![
                    draft(StarterRole::Builder, None),
                    draft(StarterRole::Builder, None),
                ],
            ),
            "duplicate role",
        ),
        (
            organization.accept(1, vec![draft(StarterRole::Lead, Some("   "))]),
            "blank Display Name",
        ),
        (
            organization.accept(1, vec![draft(StarterRole::Lead, Some("line\nbreak"))]),
            "control character",
        ),
        (
            organization.accept(1, vec![draft(StarterRole::Lead, Some(&"x".repeat(81)))]),
            "oversized Display Name",
        ),
    ];

    for (result, label) in invalid_attempts {
        assert!(result.is_err(), "{label} should be refused");
        let view = organization.view().unwrap();
        assert_eq!(view.decision, StarterOrganizationDecision::Proposed);
        assert!(view.residents.is_empty());
    }

    let accepted = organization
        .accept(1, vec![draft(StarterRole::Lead, Some("  Northstar  "))])
        .unwrap();
    assert_eq!(
        accepted.residents[0].display_name.as_deref(),
        Some("Northstar")
    );
}

#[test]
fn concurrent_owner_decisions_commit_exactly_once() {
    let (_store, accounts, organization) = enrolled_service("concurrent-decision");
    let workspace_id = organization.view().unwrap().workspace_id;
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = Vec::new();

    for role in [StarterRole::Lead, StarterRole::Builder] {
        let organization = StarterOrganizationService::new(Arc::clone(&accounts), workspace_id);
        let barrier = Arc::clone(&barrier);
        threads.push(thread::spawn(move || {
            barrier.wait();
            organization.accept(1, vec![draft(role, None)])
        }));
    }

    let results = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(StarterOrganizationError::DecisionAlreadyRecorded)
            ))
            .count(),
        1
    );
    assert_eq!(organization.view().unwrap().residents.len(), 1);
}

#[test]
fn restart_refuses_a_decision_whose_audit_event_is_missing() {
    let (store, accounts, organization) = enrolled_service("missing-event");
    organization
        .accept(1, vec![draft(StarterRole::Lead, None)])
        .unwrap();
    drop(organization);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "DELETE FROM events WHERE kind = 'starter_organization.accepted'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::StarterOrganizationStateMismatch)
    ));
}

fn draft(role: StarterRole, display_name: Option<&str>) -> StarterResidentDraft {
    StarterResidentDraft {
        role,
        display_name: display_name.map(str::to_owned),
    }
}
