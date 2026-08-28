use std::fmt;
use std::path::Path;
use std::sync::Mutex;

use argon2::{
    Argon2,
    password_hash::{
        Error as PasswordHashError, PasswordHasher, PasswordVerifier, phc::PasswordHash,
    },
};
use data_encoding::BASE32_NOPAD;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{SCHEMA_VERSION, SupervisorError, unix_time_ms, verify_schema};

const MIN_PASSWORD_CHARS: usize = 15;
const MAX_PASSWORD_CHARS: usize = 1_024;
const RECOVERY_KEY_BYTES: usize = 20;
const SESSION_TIMEOUT_MS: i64 = 30 * 60 * 1_000;
const MAX_FAILED_ATTEMPTS: i64 = 100;
const FIRST_DELAYED_ATTEMPT: i64 = 5;
const MAX_RETRY_DELAY_MS: i64 = 5 * 60 * 1_000;

/// A short-lived owned secret that zeroizes its Rust allocation on drop.
pub struct SecretString(Zeroizing<String>);

impl SecretString {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    fn expose(&self) -> &str {
        self.0.as_str()
    }

    pub(crate) fn into_inner(self) -> Zeroizing<String> {
        self.0
    }

    pub(crate) fn from_inner(value: Zeroizing<String>) -> Self {
        Self(value)
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

/// A one-time value that the desktop may display to the Owner and must never persist.
pub struct RecoveryKey(Zeroizing<String>);

impl RecoveryKey {
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    pub fn into_string(self) -> String {
        self.0.to_string()
    }

    pub(crate) fn from_inner(value: Zeroizing<String>) -> Self {
        Self(value)
    }

    pub(crate) fn into_inner(self) -> Zeroizing<String> {
        self.0
    }
}

impl fmt::Debug for RecoveryKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RecoveryKey([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessOrigin {
    SameMachine,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorSessionPhase {
    OperatorSetupRequired,
    Locked,
    Unlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorSessionStatus {
    pub phase: OperatorSessionPhase,
    pub workspace_access: bool,
    pub operator_commands: bool,
}

#[derive(Debug)]
pub enum OperatorAccountError {
    PasswordTooShort,
    PasswordTooLong,
    PasswordConfirmationMismatch,
    RecoveryNotConfirmed,
    SetupNotStarted,
    AccountAlreadyExists,
    AccountMissing,
    AuthenticationFailed,
    AuthenticationThrottled { retry_after_ms: u64 },
    CredentialDisabled,
    RemotePasswordNotAllowed,
    NotAuthenticated,
    CredentialProcessing,
    InvalidStore,
    ClockBeforeUnixEpoch,
    Store(rusqlite::Error),
    StateUnavailable,
}

impl fmt::Display for OperatorAccountError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PasswordTooShort => {
                write!(
                    formatter,
                    "password must contain at least {MIN_PASSWORD_CHARS} characters"
                )
            }
            Self::PasswordTooLong => {
                write!(
                    formatter,
                    "password cannot exceed {MAX_PASSWORD_CHARS} characters"
                )
            }
            Self::PasswordConfirmationMismatch => {
                write!(formatter, "password confirmation does not match")
            }
            Self::RecoveryNotConfirmed => write!(formatter, "Recovery Key was not confirmed saved"),
            Self::SetupNotStarted => write!(formatter, "operator setup has not started"),
            Self::AccountAlreadyExists => write!(formatter, "the Owner account already exists"),
            Self::AccountMissing => write!(formatter, "the Owner account does not exist"),
            Self::AuthenticationFailed => write!(formatter, "authentication failed"),
            Self::AuthenticationThrottled { retry_after_ms } => {
                write!(formatter, "try again in {retry_after_ms} milliseconds")
            }
            Self::CredentialDisabled => {
                write!(
                    formatter,
                    "this credential is disabled until account recovery"
                )
            }
            Self::RemotePasswordNotAllowed => {
                write!(formatter, "password fallback is restricted to this machine")
            }
            Self::NotAuthenticated => write!(formatter, "the Operator session is locked"),
            Self::CredentialProcessing => write!(formatter, "credential processing failed"),
            Self::InvalidStore => write!(formatter, "the account store failed validation"),
            Self::ClockBeforeUnixEpoch => write!(formatter, "the system clock is invalid"),
            Self::Store(error) => write!(formatter, "account store operation failed: {error}"),
            Self::StateUnavailable => write!(formatter, "account state is unavailable"),
        }
    }
}

impl std::error::Error for OperatorAccountError {}

impl From<rusqlite::Error> for OperatorAccountError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

#[derive(Clone)]
struct PendingSetup {
    password_hash: String,
    recovery_key_hash: [u8; 32],
}

#[derive(Debug, Clone, Copy)]
struct LiveSession {
    last_activity_ms: i64,
}

struct AccountState {
    connection: Connection,
    pending_setup: Option<PendingSetup>,
    session: Option<LiveSession>,
}

/// Deep account boundary: callers can authenticate or inspect status, never query credential rows.
pub struct OperatorAccountService {
    state: Mutex<AccountState>,
}

impl OperatorAccountService {
    pub fn open(store_path: impl AsRef<Path>) -> Result<Self, OperatorAccountError> {
        let connection = Connection::open(store_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        verify_schema(&connection, SCHEMA_VERSION)
            .map_err(|_| OperatorAccountError::InvalidStore)?;
        Ok(Self {
            state: Mutex::new(AccountState {
                connection,
                pending_setup: None,
                session: None,
            }),
        })
    }

    pub fn status(&self) -> Result<OperatorSessionStatus, OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        if !account_exists(&state.connection)? {
            state.session = None;
            return Ok(status_for(OperatorSessionPhase::OperatorSetupRequired));
        }

        if let Some(session) = state.session {
            let active = now >= session.last_activity_ms
                && now - session.last_activity_ms < SESSION_TIMEOUT_MS;
            if active {
                return Ok(status_for(OperatorSessionPhase::Unlocked));
            }
            state.session = None;
            insert_security_event(&state.connection, "session.locked", now)?;
        }
        Ok(status_for(OperatorSessionPhase::Locked))
    }

    pub fn begin_password_setup(
        &self,
        password: SecretString,
        confirmation: SecretString,
    ) -> Result<RecoveryKey, OperatorAccountError> {
        validate_new_password(password.expose(), confirmation.expose())?;
        {
            let state = self.lock_state()?;
            if account_exists(&state.connection)? {
                return Err(OperatorAccountError::AccountAlreadyExists);
            }
        }
        let password_hash = hash_password(&password)?;
        let (recovery_key, recovery_key_hash) = generate_recovery_key()?;

        let mut state = self.lock_state()?;
        if account_exists(&state.connection)? {
            return Err(OperatorAccountError::AccountAlreadyExists);
        }
        state.pending_setup = Some(PendingSetup {
            password_hash,
            recovery_key_hash,
        });
        Ok(recovery_key)
    }

    pub fn complete_operator_setup(
        &self,
        recovery_saved: bool,
    ) -> Result<OperatorSessionStatus, OperatorAccountError> {
        if !recovery_saved {
            return Err(OperatorAccountError::RecoveryNotConfirmed);
        }
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        if account_exists(&state.connection)? {
            return Err(OperatorAccountError::AccountAlreadyExists);
        }
        let pending = state
            .pending_setup
            .clone()
            .ok_or(OperatorAccountError::SetupNotStarted)?;

        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "INSERT INTO operator_accounts (
                singleton, role, password_hash, recovery_key_hash, created_at_ms, updated_at_ms
             ) VALUES (1, 'owner', ?1, ?2, ?3, ?3)",
            params![
                pending.password_hash,
                pending.recovery_key_hash.as_slice(),
                now
            ],
        )?;
        insert_security_event(&transaction, "operator.enrolled", now)?;
        insert_security_event(&transaction, "session.unlocked", now)?;
        transaction.commit()?;

        state.pending_setup = None;
        state.session = Some(LiveSession {
            last_activity_ms: now,
        });
        Ok(status_for(OperatorSessionPhase::Unlocked))
    }

    pub fn authenticate_password(
        &self,
        password: SecretString,
        origin: AccessOrigin,
    ) -> Result<OperatorSessionStatus, OperatorAccountError> {
        require_same_machine(origin)?;
        validate_candidate_size(password.expose())?;
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        let account =
            load_account(&state.connection)?.ok_or(OperatorAccountError::AccountMissing)?;
        enforce_rate_limit(
            account.password_failed_attempts,
            account.password_retry_after_ms,
            now,
        )?;

        let verified = verify_password(&password, &account.password_hash)?;
        if !verified {
            register_failure(&mut state.connection, CredentialKind::Password, now)?;
            return Err(OperatorAccountError::AuthenticationFailed);
        }

        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE operator_accounts
             SET password_failed_attempts = 0, password_retry_after_ms = 0, updated_at_ms = ?1
             WHERE singleton = 1",
            [now],
        )?;
        insert_security_event(&transaction, "session.unlocked", now)?;
        transaction.commit()?;
        state.session = Some(LiveSession {
            last_activity_ms: now,
        });
        Ok(status_for(OperatorSessionPhase::Unlocked))
    }

    pub fn recover_with_key(
        &self,
        recovery_key: SecretString,
        new_password: SecretString,
        confirmation: SecretString,
        origin: AccessOrigin,
    ) -> Result<RecoveryKey, OperatorAccountError> {
        require_same_machine(origin)?;
        validate_new_password(new_password.expose(), confirmation.expose())?;
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        let account =
            load_account(&state.connection)?.ok_or(OperatorAccountError::AccountMissing)?;
        enforce_rate_limit(
            account.recovery_failed_attempts,
            account.recovery_retry_after_ms,
            now,
        )?;

        let presented_hash = recovery_key_hash(recovery_key.expose());
        let verified = match presented_hash {
            Ok(hash) => account.recovery_key_hash.ct_eq(&hash).into(),
            Err(()) => false,
        };
        if !verified {
            register_failure(&mut state.connection, CredentialKind::Recovery, now)?;
            return Err(OperatorAccountError::AuthenticationFailed);
        }

        let password_hash = hash_password(&new_password)?;
        let (replacement_key, replacement_hash) = generate_recovery_key()?;
        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE operator_accounts
             SET password_hash = ?1,
                 recovery_key_hash = ?2,
                 password_failed_attempts = 0,
                 password_retry_after_ms = 0,
                 recovery_failed_attempts = 0,
                 recovery_retry_after_ms = 0,
                 updated_at_ms = ?3
             WHERE singleton = 1",
            params![password_hash, replacement_hash.as_slice(), now],
        )?;
        insert_security_event(&transaction, "recovery.used", now)?;
        insert_security_event(&transaction, "session.unlocked", now)?;
        transaction.commit()?;
        state.session = Some(LiveSession {
            last_activity_ms: now,
        });
        Ok(replacement_key)
    }

    pub fn lock(&self) -> Result<OperatorSessionStatus, OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        let phase = if account_exists(&state.connection)? {
            if state.session.take().is_some() {
                insert_security_event(&state.connection, "session.locked", now)?;
            }
            OperatorSessionPhase::Locked
        } else {
            state.session = None;
            OperatorSessionPhase::OperatorSetupRequired
        };
        Ok(status_for(phase))
    }

    pub fn record_operator_activity(&self) -> Result<(), OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        let session = state
            .session
            .as_mut()
            .ok_or(OperatorAccountError::NotAuthenticated)?;
        if now < session.last_activity_ms || now - session.last_activity_ms >= SESSION_TIMEOUT_MS {
            state.session = None;
            insert_security_event(&state.connection, "session.locked", now)?;
            return Err(OperatorAccountError::NotAuthenticated);
        }
        session.last_activity_ms = now;
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, AccountState>, OperatorAccountError> {
        self.state
            .lock()
            .map_err(|_| OperatorAccountError::StateUnavailable)
    }
}

fn status_for(phase: OperatorSessionPhase) -> OperatorSessionStatus {
    let unlocked = phase == OperatorSessionPhase::Unlocked;
    OperatorSessionStatus {
        phase,
        workspace_access: unlocked,
        operator_commands: unlocked,
    }
}

fn current_time_ms() -> Result<i64, OperatorAccountError> {
    unix_time_ms().map_err(|error| match error {
        SupervisorError::ClockBeforeUnixEpoch => OperatorAccountError::ClockBeforeUnixEpoch,
        _ => OperatorAccountError::ClockBeforeUnixEpoch,
    })
}

fn validate_new_password(password: &str, confirmation: &str) -> Result<(), OperatorAccountError> {
    let char_count = password.chars().count();
    if char_count < MIN_PASSWORD_CHARS {
        return Err(OperatorAccountError::PasswordTooShort);
    }
    if char_count > MAX_PASSWORD_CHARS {
        return Err(OperatorAccountError::PasswordTooLong);
    }
    if password != confirmation {
        return Err(OperatorAccountError::PasswordConfirmationMismatch);
    }
    Ok(())
}

fn validate_candidate_size(password: &str) -> Result<(), OperatorAccountError> {
    if password.chars().count() > MAX_PASSWORD_CHARS {
        return Err(OperatorAccountError::PasswordTooLong);
    }
    Ok(())
}

fn hash_password(password: &SecretString) -> Result<String, OperatorAccountError> {
    // RustCrypto Argon2 0.6 generates a unique salt and defaults to Argon2id v19 with the
    // OWASP minimum m=19456 KiB, t=2, p=1.
    // Sources: https://docs.rs/argon2/0.6.0/argon2/ and
    // https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html
    Argon2::default()
        .hash_password(password.expose().as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|_| OperatorAccountError::CredentialProcessing)
}

fn verify_password(
    password: &SecretString,
    stored_hash: &str,
) -> Result<bool, OperatorAccountError> {
    let parsed =
        PasswordHash::new(stored_hash).map_err(|_| OperatorAccountError::CredentialProcessing)?;
    match Argon2::default().verify_password(password.expose().as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(PasswordHashError::PasswordInvalid) => Ok(false),
        Err(_) => Err(OperatorAccountError::CredentialProcessing),
    }
}

fn generate_recovery_key() -> Result<(RecoveryKey, [u8; 32]), OperatorAccountError> {
    let mut bytes = Zeroizing::new([0_u8; RECOVERY_KEY_BYTES]);
    getrandom::fill(bytes.as_mut()).map_err(|_| OperatorAccountError::CredentialProcessing)?;
    let encoded = Zeroizing::new(BASE32_NOPAD.encode(bytes.as_ref()));
    let grouped = encoded
        .as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).expect("Base32 is ASCII"))
        .collect::<Vec<_>>()
        .join("-");
    let hash = digest_recovery_key(encoded.as_bytes());
    Ok((RecoveryKey(Zeroizing::new(grouped)), hash))
}

fn recovery_key_hash(value: &str) -> Result<[u8; 32], ()> {
    let normalized = Zeroizing::new(
        value
            .chars()
            .filter(|character| *character != '-' && *character != ' ')
            .flat_map(char::to_uppercase)
            .collect::<String>(),
    );
    if normalized.len() != 32 {
        return Err(());
    }
    let decoded = BASE32_NOPAD.decode(normalized.as_bytes()).map_err(|_| ())?;
    if decoded.len() != RECOVERY_KEY_BYTES {
        return Err(());
    }
    Ok(digest_recovery_key(normalized.as_bytes()))
}

fn digest_recovery_key(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn require_same_machine(origin: AccessOrigin) -> Result<(), OperatorAccountError> {
    if origin == AccessOrigin::Remote {
        return Err(OperatorAccountError::RemotePasswordNotAllowed);
    }
    Ok(())
}

struct StoredAccount {
    password_hash: String,
    recovery_key_hash: [u8; 32],
    password_failed_attempts: i64,
    password_retry_after_ms: i64,
    recovery_failed_attempts: i64,
    recovery_retry_after_ms: i64,
}

fn account_exists(connection: &Connection) -> Result<bool, OperatorAccountError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operator_accounts WHERE singleton = 1)",
            [],
            |row| row.get(0),
        )
        .map_err(OperatorAccountError::Store)
}

fn load_account(connection: &Connection) -> Result<Option<StoredAccount>, OperatorAccountError> {
    connection
        .query_row(
            "SELECT password_hash,
                    recovery_key_hash,
                    password_failed_attempts,
                    password_retry_after_ms,
                    recovery_failed_attempts,
                    recovery_retry_after_ms
             FROM operator_accounts
             WHERE singleton = 1",
            [],
            |row| {
                let recovery_hash = row.get::<_, Vec<u8>>(1)?;
                let recovery_key_hash: [u8; 32] =
                    recovery_hash.try_into().map_err(|value: Vec<u8>| {
                        rusqlite::Error::IntegralValueOutOfRange(1, value.len() as i64)
                    })?;
                Ok(StoredAccount {
                    password_hash: row.get(0)?,
                    recovery_key_hash,
                    password_failed_attempts: row.get(2)?,
                    password_retry_after_ms: row.get(3)?,
                    recovery_failed_attempts: row.get(4)?,
                    recovery_retry_after_ms: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(OperatorAccountError::Store)
}

#[derive(Clone, Copy)]
enum CredentialKind {
    Password,
    Recovery,
}

fn enforce_rate_limit(
    attempts: i64,
    retry_after_ms: i64,
    now: i64,
) -> Result<(), OperatorAccountError> {
    if attempts >= MAX_FAILED_ATTEMPTS {
        return Err(OperatorAccountError::CredentialDisabled);
    }
    if retry_after_ms > now {
        return Err(OperatorAccountError::AuthenticationThrottled {
            retry_after_ms: u64::try_from(retry_after_ms - now).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn register_failure(
    connection: &mut Connection,
    kind: CredentialKind,
    now: i64,
) -> Result<(), OperatorAccountError> {
    let (attempt_column, retry_column, event_kind) = match kind {
        CredentialKind::Password => (
            "password_failed_attempts",
            "password_retry_after_ms",
            "authentication.failed",
        ),
        CredentialKind::Recovery => (
            "recovery_failed_attempts",
            "recovery_retry_after_ms",
            "recovery.failed",
        ),
    };
    let select = format!("SELECT {attempt_column} FROM operator_accounts WHERE singleton = 1");
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let previous: i64 = transaction.query_row(&select, [], |row| row.get(0))?;
    let attempts = (previous + 1).min(MAX_FAILED_ATTEMPTS);
    let retry_after = if attempts < FIRST_DELAYED_ATTEMPT {
        0
    } else {
        let exponent = u32::try_from((attempts - FIRST_DELAYED_ATTEMPT).min(18)).unwrap_or(18);
        now.saturating_add((1_000_i64 << exponent).min(MAX_RETRY_DELAY_MS))
    };
    let update = format!(
        "UPDATE operator_accounts
         SET {attempt_column} = ?1, {retry_column} = ?2, updated_at_ms = ?3
         WHERE singleton = 1"
    );
    transaction.execute(&update, params![attempts, retry_after, now])?;
    insert_security_event(&transaction, event_kind, now)?;
    transaction.commit()?;
    Ok(())
}

fn insert_security_event(
    connection: &Connection,
    kind: &str,
    occurred_at_ms: i64,
) -> Result<(), OperatorAccountError> {
    connection.execute(
        "INSERT INTO security_events (kind, detail_json, occurred_at_ms)
         VALUES (?1, '{}', ?2)",
        params![kind, occurred_at_ms],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;
    use crate::{StartupConfig, Supervisor};

    #[test]
    fn status_expires_an_idle_session_without_treating_the_check_as_activity() {
        let path = std::env::temp_dir().join(format!("mosaicd-session-{}.sqlite3", Uuid::new_v4()));
        Supervisor::bootstrap(StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();
        let service = OperatorAccountService::open(&path).unwrap();
        service
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        service.complete_operator_setup(true).unwrap();
        service.state.lock().unwrap().session = Some(LiveSession {
            last_activity_ms: 0,
        });

        let status = service.status().unwrap();

        assert_eq!(status.phase, OperatorSessionPhase::Locked);
        assert!(!status.workspace_access);
        drop(service);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn malformed_stored_password_hash_fails_closed_as_processing_error() {
        let path = std::env::temp_dir().join(format!("mosaicd-corrupt-{}.sqlite3", Uuid::new_v4()));
        Supervisor::bootstrap(StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();
        let service = OperatorAccountService::open(&path).unwrap();
        service
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        service.complete_operator_setup(true).unwrap();
        service.lock().unwrap();
        service
            .state
            .lock()
            .unwrap()
            .connection
            .execute(
                "UPDATE operator_accounts SET password_hash = 'not-a-valid-password-hash-but-long-enough'",
                [],
            )
            .unwrap();

        let error = service
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .unwrap_err();

        assert!(matches!(error, OperatorAccountError::CredentialProcessing));
        drop(service);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}
