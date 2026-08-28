use std::fs;
use std::path::{Path, PathBuf};

use mosaicd::{
    AccessOrigin, OperatorAccountError, OperatorAccountService, OperatorSessionPhase,
    PlatformSessionEvent, SecretString, StartupConfig, Supervisor,
};
use rusqlite::Connection;
use uuid::Uuid;

const PASSWORD: &str = "a long local passphrase";
const NEW_PASSWORD: &str = "a different long passphrase";

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "mosaicd-operator-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
        }
    }

    fn service(&self) -> OperatorAccountService {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();
        OperatorAccountService::open(&self.path).unwrap()
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn enroll(service: &OperatorAccountService, password: &str) -> String {
    let recovery = service
        .begin_password_setup(
            SecretString::new(password.to_owned()),
            SecretString::new(password.to_owned()),
        )
        .unwrap()
        .into_string();
    service.complete_operator_setup(true).unwrap();
    recovery
}

#[test]
fn setup_requires_recovery_confirmation_before_writing_an_account() {
    let store = TestStore::new("confirmation");
    let service = store.service();
    let recovery = service
        .begin_password_setup(
            SecretString::new(PASSWORD.to_owned()),
            SecretString::new(PASSWORD.to_owned()),
        )
        .unwrap();

    let error = service.complete_operator_setup(false).unwrap_err();

    assert!(matches!(error, OperatorAccountError::RecoveryNotConfirmed));
    assert_eq!(
        service.status().unwrap().phase,
        OperatorSessionPhase::OperatorSetupRequired
    );
    assert_eq!(account_count(&store.path), 0);
    assert!(!format!("{recovery:?}").contains(recovery.expose()));
}

#[test]
fn password_rules_accept_unicode_and_spaces_but_reject_bad_boundaries() {
    let store = TestStore::new("password-rules");
    let service = store.service();

    assert!(matches!(
        service.begin_password_setup(
            SecretString::new("too short".to_owned()),
            SecretString::new("too short".to_owned())
        ),
        Err(OperatorAccountError::PasswordTooShort)
    ));
    assert!(matches!(
        service.begin_password_setup(
            SecretString::new("this passphrase is long enough".to_owned()),
            SecretString::new("this passphrase is different".to_owned())
        ),
        Err(OperatorAccountError::PasswordConfirmationMismatch)
    ));
    let oversized = "🦀".repeat(1_025);
    assert!(matches!(
        service.begin_password_setup(
            SecretString::new(oversized.clone()),
            SecretString::new(oversized)
        ),
        Err(OperatorAccountError::PasswordTooLong)
    ));

    let unicode = "  correct horse café battery 🔐  ";
    let recovery = service
        .begin_password_setup(
            SecretString::new(unicode.to_owned()),
            SecretString::new(unicode.to_owned()),
        )
        .unwrap();
    assert_eq!(recovery.expose().replace('-', "").len(), 32);
}

#[test]
fn enrollment_stores_verifiers_only_and_a_restart_returns_locked() {
    let store = TestStore::new("persistence");
    let service = store.service();
    let recovery = enroll(&service, PASSWORD);

    let status = service.status().unwrap();
    assert_eq!(status.phase, OperatorSessionPhase::Unlocked);
    assert!(status.workspace_access);
    assert!(status.operator_commands);

    let connection = Connection::open(&store.path).unwrap();
    let (role, password_hash, recovery_hash): (String, String, Vec<u8>) = connection
        .query_row(
            "SELECT role, password_hash, recovery_key_hash FROM operator_accounts WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(role, "owner");
    assert!(password_hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
    assert_eq!(recovery_hash.len(), 32);
    let database_bytes = fs::read(&store.path).unwrap();
    assert!(!contains_bytes(&database_bytes, PASSWORD.as_bytes()));
    assert!(!contains_bytes(&database_bytes, recovery.as_bytes()));
    drop(connection);
    drop(service);

    let reopened = OperatorAccountService::open(&store.path).unwrap();
    let status = reopened.status().unwrap();
    assert_eq!(status.phase, OperatorSessionPhase::Locked);
    assert!(!status.workspace_access);
    assert!(!status.operator_commands);
    reopened
        .authenticate_password(
            SecretString::new(PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    assert_eq!(
        reopened.status().unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
}

#[test]
fn session_timeout_defaults_to_thirty_minutes_and_persists_owner_changes() {
    let store = TestStore::new("session-policy");
    let service = store.service();
    enroll(&service, PASSWORD);

    assert_eq!(
        service.session_policy().unwrap().inactivity_timeout_minutes,
        30
    );
    assert_eq!(
        service
            .set_session_timeout_minutes(45)
            .unwrap()
            .inactivity_timeout_minutes,
        45
    );
    drop(service);

    let reopened = OperatorAccountService::open(&store.path).unwrap();
    reopened
        .authenticate_password(
            SecretString::new(PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    assert_eq!(
        reopened
            .session_policy()
            .unwrap()
            .inactivity_timeout_minutes,
        45
    );
}

#[test]
fn session_timeout_change_requires_an_unlocked_owner_and_enforces_closed_bounds() {
    let store = TestStore::new("session-policy-validation");
    let service = store.service();

    assert!(matches!(
        service.set_session_timeout_minutes(30),
        Err(OperatorAccountError::NotAuthenticated)
    ));
    enroll(&service, PASSWORD);
    for invalid in [0, 1_441] {
        assert!(matches!(
            service.set_session_timeout_minutes(invalid),
            Err(OperatorAccountError::InvalidSessionTimeout)
        ));
    }
    assert_eq!(
        service
            .set_session_timeout_minutes(1)
            .unwrap()
            .inactivity_timeout_minutes,
        1
    );
    assert_eq!(
        service
            .set_session_timeout_minutes(1_440)
            .unwrap()
            .inactivity_timeout_minutes,
        1_440
    );
    service.lock().unwrap();
    assert!(matches!(
        service.session_policy(),
        Err(OperatorAccountError::NotAuthenticated)
    ));
}

#[test]
fn platform_lock_and_suspend_events_lock_without_ever_unlocking_the_owner() {
    let store = TestStore::new("platform-lock");
    let service = store.service();
    enroll(&service, PASSWORD);

    assert_eq!(
        service
            .lock_for_platform_event(PlatformSessionEvent::ScreenLocked)
            .unwrap()
            .phase,
        OperatorSessionPhase::Locked
    );
    assert_eq!(
        service
            .lock_for_platform_event(PlatformSessionEvent::PreparingForSleep)
            .unwrap()
            .phase,
        OperatorSessionPhase::Locked
    );
}

#[test]
fn password_fallback_is_refused_for_remote_access() {
    let store = TestStore::new("remote-password");
    let service = store.service();
    enroll(&service, PASSWORD);
    service.lock().unwrap();

    let error = service
        .authenticate_password(SecretString::new(PASSWORD.to_owned()), AccessOrigin::Remote)
        .unwrap_err();

    assert!(matches!(
        error,
        OperatorAccountError::RemotePasswordNotAllowed
    ));
    assert_eq!(
        service.status().unwrap().phase,
        OperatorSessionPhase::Locked
    );
}

#[test]
fn repeated_password_failures_are_throttled_and_success_clears_the_counter() {
    let store = TestStore::new("password-throttle");
    let service = store.service();
    enroll(&service, PASSWORD);
    service.lock().unwrap();

    for _ in 0..5 {
        assert!(matches!(
            service.authenticate_password(
                SecretString::new("the wrong long passphrase".to_owned()),
                AccessOrigin::SameMachine,
            ),
            Err(OperatorAccountError::AuthenticationFailed)
        ));
    }
    assert!(matches!(
        service.authenticate_password(
            SecretString::new(PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        ),
        Err(OperatorAccountError::AuthenticationThrottled { .. })
    ));
    let connection = Connection::open(&store.path).unwrap();
    let attempts: i64 = connection
        .query_row(
            "SELECT password_failed_attempts FROM operator_accounts WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(attempts, 5);
}

#[test]
fn recovery_rotates_the_key_and_password_atomically() {
    let store = TestStore::new("recovery");
    let service = store.service();
    let old_recovery = enroll(&service, PASSWORD);
    service.lock().unwrap();

    let new_recovery = service
        .recover_with_key(
            SecretString::new(old_recovery.clone()),
            SecretString::new(NEW_PASSWORD.to_owned()),
            SecretString::new(NEW_PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap()
        .into_string();
    assert_ne!(new_recovery, old_recovery);
    assert_eq!(
        service.status().unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    service.lock().unwrap();

    assert!(matches!(
        service.recover_with_key(
            SecretString::new(old_recovery),
            SecretString::new(PASSWORD.to_owned()),
            SecretString::new(PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        ),
        Err(OperatorAccountError::AuthenticationFailed)
    ));
    service
        .authenticate_password(
            SecretString::new(NEW_PASSWORD.to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
}

fn account_count(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM operator_accounts", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
