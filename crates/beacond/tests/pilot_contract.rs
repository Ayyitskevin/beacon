//! Public-interface absence and historical-meaning checks. No fixture schema is installed here.
use beacond::pilot_authority::{
    OperationIntent, PilotAuthorityService, PilotError, Units, WorkspaceAllocation,
};
use beacond::{
    AutonomyProfile, BudgetBoundary, ExternalEffects, OperatorAccountService, ProjectCharterDraft,
    ProjectDraft, ProjectService, SecretString, StartupConfig, Supervisor,
};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

struct Store {
    path: PathBuf,
    workspace: Uuid,
    accounts: Arc<OperatorAccountService>,
}
impl Store {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("beacond-i1-public-{}.sqlite3", Uuid::new_v4()));
        let report = Supervisor::bootstrap(StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "I1 public synthetic".into(),
        })
        .unwrap();
        let accounts = Arc::new(OperatorAccountService::open(&path).unwrap());
        Self {
            path,
            workspace: report.workspace.id,
            accounts,
        }
    }
    fn enroll(&self) {
        self.accounts
            .begin_password_setup(
                SecretString::new("a long local passphrase".into()),
                SecretString::new("a long local passphrase".into()),
            )
            .unwrap();
        self.accounts.complete_operator_setup(true).unwrap();
    }
    fn pilot(&self) -> PilotAuthorityService {
        PilotAuthorityService::new(Arc::clone(&self.accounts), self.workspace)
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}
fn unhex(text: &str) -> Vec<u8> {
    let text: String = text.split_whitespace().collect();
    text.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn pilot_public_authentication_precedes_untrusted_decode_and_missing_schema() {
    let store = Store::new();
    let pilot = store.pilot();
    assert!(matches!(
        pilot.inspect_current_owner_references(b"not a record"),
        Err(PilotError::Account(_))
    ));
    assert!(matches!(
        pilot.resolve_resident_operation(b"not a record"),
        Err(PilotError::Account(_))
    ));
    store.enroll();
    assert!(matches!(
        pilot.inspect_current_owner_references(b"not a record"),
        Err(PilotError::InvalidBytes)
    ));
    let w = WorkspaceAllocation {
        workspace: store.workspace,
        predecessor: None,
        issued_at_ms: 0,
        expires_at_ms: u64::MAX,
        ceiling: Units([1, 1, 1, 1, 1, 1, 1, 1, 100, 0, 1, 1]),
    };
    assert!(matches!(
        pilot.publish_workspace_allocation(&w.canonical_bytes().unwrap()),
        Err(PilotError::FixtureSchemaUnavailable)
    ));
    store.accounts.lock().unwrap();
    assert!(matches!(
        pilot.retained_records(),
        Err(PilotError::Account(_))
    ));
}

#[test]
fn pilot_public_resident_route_stays_fail_closed_no_caller_role_authority() {
    let store = Store::new();
    store.enroll();
    let bytes = unhex(include_str!("fixtures/i1/operation.hex"));
    let intent = OperationIntent::decode(&bytes).unwrap();
    assert_eq!(intent.canonical_bytes().unwrap(), bytes);
    assert!(matches!(
        store.pilot().resolve_resident_operation(&bytes),
        Err(PilotError::UnresolvedContinuation)
    ));
    let connection = beacond::open_store_connection(&store.path).unwrap();
    let tables: i64 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name LIKE 'pilot_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let events: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind LIKE 'pilot.%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!((tables, events), (0, 0));
}

#[test]
fn pilot_old_store_ladder_reopens_original_prohibitions_unchanged() {
    let store = Store::new();
    store.enroll();
    let project = ProjectService::new(Arc::clone(&store.accounts), store.workspace)
        .create(ProjectDraft {
            name: "I1 historical positive".into(),
            charter: ProjectCharterDraft {
                purpose: "Preserve historical policy".into(),
                desired_outcomes: vec!["No implicit new permission".into()],
                constraints: vec![],
                prohibitions: vec![],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        })
        .unwrap();
    assert_eq!(project.charter.budget_boundary, BudgetBoundary::NoSpend);
    assert_eq!(
        project.charter.external_effects,
        ExternalEffects::Prohibited
    );
    let connection = beacond::open_store_connection(&store.path).unwrap();
    let before: Vec<(i64, String, String)> = connection
        .prepare("SELECT sequence,kind,payload_json FROM events ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    let reopened = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "ignored".into(),
    })
    .unwrap();
    assert_eq!(reopened.workspace.id, store.workspace);
    let after: Vec<(i64, String, String)> = connection
        .prepare("SELECT sequence,kind,payload_json FROM events ORDER BY sequence")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(before, after);
    assert!(OperatorAccountService::open(&store.path).is_ok());
}
