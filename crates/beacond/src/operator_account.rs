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
const MIN_SESSION_TIMEOUT_MINUTES: u16 = 1;
const MAX_SESSION_TIMEOUT_MINUTES: u16 = 24 * 60;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSessionEvent {
    ScreenLocked,
    PreparingForSleep,
}

impl PlatformSessionEvent {
    fn reason(self) -> &'static str {
        match self {
            Self::ScreenLocked => "screen_locked",
            Self::PreparingForSleep => "preparing_for_sleep",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorSessionStatus {
    pub phase: OperatorSessionPhase,
    pub workspace_access: bool,
    pub operator_commands: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperatorSessionPolicy {
    pub inactivity_timeout_minutes: u16,
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
    InvalidSessionTimeout,
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
            Self::InvalidSessionTimeout => write!(
                formatter,
                "session timeout must be between {MIN_SESSION_TIMEOUT_MINUTES} and {MAX_SESSION_TIMEOUT_MINUTES} minutes"
            ),
            Self::CredentialProcessing => write!(formatter, "credential processing failed"),
            Self::InvalidStore => write!(formatter, "the account store failed validation"),
            Self::ClockBeforeUnixEpoch => write!(formatter, "the system clock is invalid"),
            // R42: the driver's own words -- table and column names, file paths, constraint
            // text. The `rusqlite::Error` stays whole in the derived `Debug`, which is this
            // enum's diagnostic channel; it gains no `source()` here.
            Self::Store(_) => write!(formatter, "account store operation failed: Store"),
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
        let connection = crate::open_store_connection(store_path.as_ref())?;
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
            let timeout_ms = load_session_timeout_ms(&state.connection)?;
            let active =
                now >= session.last_activity_ms && now - session.last_activity_ms < timeout_ms;
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
            register_failure(&mut state.connection, CredentialKind::Password)?;
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
            register_failure(&mut state.connection, CredentialKind::Recovery)?;
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
        self.lock_with_reason("operator_requested")
    }

    pub fn lock_for_platform_event(
        &self,
        event: PlatformSessionEvent,
    ) -> Result<OperatorSessionStatus, OperatorAccountError> {
        self.lock_with_reason(event.reason())
    }

    fn lock_with_reason(
        &self,
        reason: &'static str,
    ) -> Result<OperatorSessionStatus, OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        let phase = if account_exists(&state.connection)? {
            if state.session.take().is_some() {
                insert_security_event_with_detail(
                    &state.connection,
                    "session.locked",
                    &serde_json::json!({ "reason": reason }).to_string(),
                    now,
                )?;
            }
            OperatorSessionPhase::Locked
        } else {
            state.session = None;
            OperatorSessionPhase::OperatorSetupRequired
        };
        Ok(status_for(phase))
    }

    pub fn session_policy(&self) -> Result<OperatorSessionPolicy, OperatorAccountError> {
        let mut state = self.lock_state()?;
        require_active_session(&mut state, current_time_ms()?)?;
        load_session_policy(&state.connection)
    }

    pub fn set_session_timeout_minutes(
        &self,
        inactivity_timeout_minutes: u16,
    ) -> Result<OperatorSessionPolicy, OperatorAccountError> {
        if !(MIN_SESSION_TIMEOUT_MINUTES..=MAX_SESSION_TIMEOUT_MINUTES)
            .contains(&inactivity_timeout_minutes)
        {
            return Err(OperatorAccountError::InvalidSessionTimeout);
        }
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        require_active_session(&mut state, now)?;
        let transaction = state
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE operator_accounts
             SET session_timeout_minutes = ?1, updated_at_ms = ?2
             WHERE singleton = 1",
            params![inactivity_timeout_minutes, now],
        )?;
        insert_security_event_with_detail(
            &transaction,
            "session.policy_changed",
            &serde_json::json!({
                "inactivity_timeout_minutes": inactivity_timeout_minutes
            })
            .to_string(),
            now,
        )?;
        transaction.commit()?;

        require_active_session(&mut state, now)?;
        Ok(OperatorSessionPolicy {
            inactivity_timeout_minutes,
        })
    }

    pub fn record_operator_activity(&self) -> Result<(), OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        require_active_session(&mut state, now)?;
        let session = state.session.as_mut().expect("active session exists");
        session.last_activity_ms = now;
        Ok(())
    }

    pub(crate) fn require_authenticated(&self) -> Result<(), OperatorAccountError> {
        let now = current_time_ms()?;
        let mut state = self.lock_state()?;
        require_active_session(&mut state, now)
    }

    pub(crate) fn with_authenticated_store<T, E>(
        &self,
        operation: impl FnOnce(&mut Connection, i64) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<OperatorAccountError>,
    {
        let now = current_time_ms().map_err(E::from)?;
        let mut state = self.lock_state().map_err(E::from)?;
        require_active_session(&mut state, now).map_err(E::from)?;
        operation(&mut state.connection, now)
    }

    /// Test-only: a pragma's value on this service's own connection, so a test can prove that the
    /// one connection every service reaches through `with_authenticated_store` carries the state
    /// `open_store_connection` sets (R21).
    #[cfg(test)]
    pub(crate) fn store_pragma(&self, name: &str) -> i64 {
        let state = self.lock_state().expect("the account state lock");
        state
            .connection
            .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
            .expect("a pragma reads")
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, AccountState>, OperatorAccountError> {
        self.state
            .lock()
            .map_err(|_| OperatorAccountError::StateUnavailable)
    }
}

/// Whether this store still holds its singleton Owner account. A record that names
/// `owner` as its authorization basis is only truthful while that account exists, so
/// reconciliation asks the account module rather than reading its table directly.
pub(crate) fn owner_account_exists(connection: &Connection) -> Result<bool, rusqlite::Error> {
    let count: i64 = connection.query_row(
        "SELECT count(*) FROM operator_accounts WHERE singleton = 1 AND role = 'owner'",
        [],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

fn require_active_session(state: &mut AccountState, now: i64) -> Result<(), OperatorAccountError> {
    let session = state
        .session
        .ok_or(OperatorAccountError::NotAuthenticated)?;
    let timeout_ms = load_session_timeout_ms(&state.connection)?;
    if now < session.last_activity_ms || now - session.last_activity_ms >= timeout_ms {
        state.session = None;
        insert_security_event(&state.connection, "session.locked", now)?;
        return Err(OperatorAccountError::NotAuthenticated);
    }
    Ok(())
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

fn load_session_policy(
    connection: &Connection,
) -> Result<OperatorSessionPolicy, OperatorAccountError> {
    let value: i64 = connection.query_row(
        "SELECT session_timeout_minutes FROM operator_accounts WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    let inactivity_timeout_minutes = u16::try_from(value)
        .ok()
        .filter(|value| (MIN_SESSION_TIMEOUT_MINUTES..=MAX_SESSION_TIMEOUT_MINUTES).contains(value))
        .ok_or(OperatorAccountError::InvalidStore)?;
    Ok(OperatorSessionPolicy {
        inactivity_timeout_minutes,
    })
}

fn load_session_timeout_ms(connection: &Connection) -> Result<i64, OperatorAccountError> {
    Ok(i64::from(load_session_policy(connection)?.inactivity_timeout_minutes) * 60 * 1_000)
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
    let now = current_time_ms()?;
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
    insert_security_event_with_detail(connection, kind, "{}", occurred_at_ms)
}

fn insert_security_event_with_detail(
    connection: &Connection,
    kind: &str,
    detail_json: &str,
    occurred_at_ms: i64,
) -> Result<(), OperatorAccountError> {
    connection.execute(
        "INSERT INTO security_events (kind, detail_json, occurred_at_ms)
         VALUES (?1, ?2, ?3)",
        params![kind, detail_json, occurred_at_ms],
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
        let path = std::env::temp_dir().join(format!("beacond-session-{}.sqlite3", Uuid::new_v4()));
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
        let path = std::env::temp_dir().join(format!("beacond-corrupt-{}.sqlite3", Uuid::new_v4()));
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

    #[test]
    fn shortening_the_policy_below_elapsed_activity_locks_the_current_session() {
        let path =
            std::env::temp_dir().join(format!("beacond-short-policy-{}.sqlite3", Uuid::new_v4()));
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
            last_activity_ms: current_time_ms().unwrap() - 61_000,
        });

        let error = service.set_session_timeout_minutes(1).unwrap_err();

        assert!(matches!(error, OperatorAccountError::NotAuthenticated));
        assert_eq!(
            service.status().unwrap().phase,
            OperatorSessionPhase::Locked
        );
        assert_eq!(
            service
                .state
                .lock()
                .unwrap()
                .connection
                .query_row(
                    "SELECT session_timeout_minutes FROM operator_accounts WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        drop(service);
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }
}

#[cfg(test)]
mod refusal_text_witness {
    use super::*;

    const NEEDLE_A: &str = "R42S3_PRIVATE_A";
    const NEEDLE_B: &str = "R42S3_PRIVATE_B";

    /// Approved retry context, chosen here. The spec asks for 0, a distinct value and the
    /// saturating end of the range, so a selector printing a constant fails on at least two.
    const FIRST_RETRY: u64 = 0;
    const SECOND_RETRY: u64 = 4242;
    const THIRD_RETRY: u64 = u64::MAX;

    /// Carrying the CR/LF the spec lists, so ONE_LINE is a real rung here too.
    fn driver_error(needle: &str) -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName(format!(
            "{needle} /home/example/private.db :private_binding\r\nsecond-line"
        ))
    }

    macro_rules! closed_table {
        (
            $expected_fn:ident / $rows_fn:ident : $ty:ty,
            $( $case:literal | $pattern:pat => $line:expr , $build:expr , $markers:expr ; )+
        ) => {
            fn $expected_fn(value: &$ty) -> (&'static str, String) {
                match value {
                    $( $pattern => ($case, String::from($line)), )+
                }
            }

            fn $rows_fn() -> Vec<(&'static str, $ty, &'static [&'static str])> {
                vec![$(($case, $build, $markers)),+]
            }
        };
    }

    closed_table! {
        expected_account / account_rows: OperatorAccountError,
        "Store" | OperatorAccountError::Store(_)
            => "account store operation failed: Store",
            OperatorAccountError::Store(driver_error(NEEDLE_A)), &[NEEDLE_A];
        "PasswordTooShort" | OperatorAccountError::PasswordTooShort
            => "password must contain at least 15 characters",
            OperatorAccountError::PasswordTooShort, &[];
        "PasswordTooLong" | OperatorAccountError::PasswordTooLong
            => "password cannot exceed 1024 characters",
            OperatorAccountError::PasswordTooLong, &[];
        "PasswordConfirmationMismatch" | OperatorAccountError::PasswordConfirmationMismatch
            => "password confirmation does not match",
            OperatorAccountError::PasswordConfirmationMismatch, &[];
        "RecoveryNotConfirmed" | OperatorAccountError::RecoveryNotConfirmed
            => "Recovery Key was not confirmed saved",
            OperatorAccountError::RecoveryNotConfirmed, &[];
        "SetupNotStarted" | OperatorAccountError::SetupNotStarted
            => "operator setup has not started",
            OperatorAccountError::SetupNotStarted, &[];
        "AccountAlreadyExists" | OperatorAccountError::AccountAlreadyExists
            => "the Owner account already exists",
            OperatorAccountError::AccountAlreadyExists, &[];
        "AccountMissing" | OperatorAccountError::AccountMissing
            => "the Owner account does not exist",
            OperatorAccountError::AccountMissing, &[];
        "AuthenticationFailed" | OperatorAccountError::AuthenticationFailed
            => "authentication failed", OperatorAccountError::AuthenticationFailed, &[];
        "AuthenticationThrottled" | OperatorAccountError::AuthenticationThrottled { retry_after_ms }
            => format!("try again in {retry_after_ms} milliseconds"),
            OperatorAccountError::AuthenticationThrottled {
                retry_after_ms: SECOND_RETRY,
            },
            &[];
        "CredentialDisabled" | OperatorAccountError::CredentialDisabled
            => "this credential is disabled until account recovery",
            OperatorAccountError::CredentialDisabled, &[];
        "RemotePasswordNotAllowed" | OperatorAccountError::RemotePasswordNotAllowed
            => "password fallback is restricted to this machine",
            OperatorAccountError::RemotePasswordNotAllowed, &[];
        "NotAuthenticated" | OperatorAccountError::NotAuthenticated
            => "the Operator session is locked",
            OperatorAccountError::NotAuthenticated, &[];
        "InvalidSessionTimeout" | OperatorAccountError::InvalidSessionTimeout
            => "session timeout must be between 1 and 1440 minutes",
            OperatorAccountError::InvalidSessionTimeout, &[];
        "CredentialProcessing" | OperatorAccountError::CredentialProcessing
            => "credential processing failed",
            OperatorAccountError::CredentialProcessing, &[];
        "InvalidStore" | OperatorAccountError::InvalidStore
            => "the account store failed validation",
            OperatorAccountError::InvalidStore, &[];
        "ClockBeforeUnixEpoch" | OperatorAccountError::ClockBeforeUnixEpoch
            => "the system clock is invalid",
            OperatorAccountError::ClockBeforeUnixEpoch, &[];
        "StateUnavailable" | OperatorAccountError::StateUnavailable
            => "account state is unavailable",
            OperatorAccountError::StateUnavailable, &[];
    }

    /// R42 slice 3: the account error's only data-bearing line is closed, and the other 17 are
    /// pinned so closing one cannot reword the rest.
    ///
    /// `Store` carried the driver's own words -- table and column names, file paths, constraint
    /// text -- to every ordinary account caller. The `rusqlite::Error` stays whole in `Debug`.
    /// The configured bounds (15 / 1024 characters, 1 / 1440 minutes) and the retry duration are
    /// pinned here independently; this slice adds no validation and changes none of them.
    #[test]
    fn r42_account_projection_is_closed() {
        let rows = account_rows();
        assert_eq!(rows.len(), 18, "R42S3.ACCOUNT.CARDINALITY.ROOT");
        let ids: std::collections::BTreeSet<&str> = rows.iter().map(|(case, _, _)| *case).collect();
        assert_eq!(ids.len(), rows.len(), "R42S3.ACCOUNT.CASE_IDS_UNIQUE");
        assert_eq!(rows[0].0, "Store", "R42S3.ACCOUNT.LEAD_IS_STORE");

        for (case, value, markers) in &rows {
            // 1. Fixture premise.
            let (matched, expected) = expected_account(value);
            assert_eq!(matched, *case, "R42S3.FIXTURE.{case}.PATTERN");
            let debug = format!("{value:?}");
            for marker in *markers {
                assert!(debug.contains(marker), "R42S3.FIXTURE.{case}.NEEDLE");
            }

            // 2. Nothing hidden reaches the line; one line; Debug still carries it.
            let line = value.to_string();
            for marker in *markers {
                assert!(
                    !line.contains(marker),
                    "R42S3.ACCOUNT.{case}.NO_NEEDLE: {line}"
                );
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S3.ACCOUNT.{case}.ONE_LINE: {line:?}"
            );
            for marker in *markers {
                assert!(debug.contains(marker), "R42S3.ACCOUNT.{case}.DEBUG_RETAINS");
            }

            // 3. The whole line against the independent literal.
            assert_eq!(line, expected, "R42S3.ACCOUNT.{case}.PROJECTION");
        }

        // The B seed: a different driver payload, the same independently fixed line.
        let first = OperatorAccountError::Store(driver_error(NEEDLE_A));
        let second = OperatorAccountError::Store(driver_error(NEEDLE_B));
        assert!(
            !second.to_string().contains(NEEDLE_B),
            "R42S3.ACCOUNT.Store.NO_NEEDLE"
        );
        assert_eq!(
            first.to_string(),
            second.to_string(),
            "R42S3.ACCOUNT.Store.HIDDEN_CHANGE_SAME_LINE"
        );
        assert!(
            format!("{second:?}").contains(NEEDLE_B),
            "R42S3.ACCOUNT.Store.DEBUG_RETAINS"
        );

        // 4. Approved context: the retry duration at three independently chosen values, and the
        // configured bounds, pinned exactly as they read today.
        for retry_after_ms in [FIRST_RETRY, SECOND_RETRY, THIRD_RETRY] {
            assert_eq!(
                OperatorAccountError::AuthenticationThrottled { retry_after_ms }.to_string(),
                format!("try again in {retry_after_ms} milliseconds"),
                "R42S3.ACCOUNT.AuthenticationThrottled.PROJECTION"
            );
        }
        assert_eq!(
            OperatorAccountError::PasswordTooShort.to_string(),
            "password must contain at least 15 characters",
            "R42S3.ACCOUNT.PasswordTooShort.CONTEXT"
        );
        assert_eq!(
            OperatorAccountError::PasswordTooLong.to_string(),
            "password cannot exceed 1024 characters",
            "R42S3.ACCOUNT.PasswordTooLong.CONTEXT"
        );
        assert_eq!(
            OperatorAccountError::InvalidSessionTimeout.to_string(),
            "session timeout must be between 1 and 1440 minutes",
            "R42S3.ACCOUNT.InvalidSessionTimeout.CONTEXT"
        );

        // The real `From<rusqlite::Error>`: the marker is verified on the ORIGINAL, before the
        // conversion, so a carrier-erasure mutant fails the retention contract rather than merely
        // breaking a fixture premise.
        let original = driver_error(NEEDLE_A);
        assert!(
            format!("{original:?}").contains(NEEDLE_A),
            "R42S3.FIXTURE.FromStore.NEEDLE"
        );
        let converted = OperatorAccountError::from(original);
        assert!(
            format!("{converted:?}").contains(NEEDLE_A),
            "R42S3.ACCOUNT.FromStore.DEBUG_RETAINS"
        );
        assert_eq!(
            converted.to_string(),
            "account store operation failed: Store",
            "R42S3.ACCOUNT.FromStore.PROJECTION"
        );
    }
}
