#![cfg(unix)]

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use data_encoding::BASE32_NOPAD;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::{
    AccessOrigin, OperatorAccountError, OperatorAccountService, OperatorSessionStatus, RecoveryKey,
    SecretString, StartupConfig, Supervisor,
};

const PROTOCOL_VERSION: u16 = 1;
const AUTH_TOKEN_BYTES: usize = 32;
const MAX_MESSAGE_BYTES: u64 = 64 * 1024;
const SERVER_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const CLIENT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct LocalServerConfig {
    pub store_path: PathBuf,
    pub initial_workspace_name: String,
    pub socket_path: PathBuf,
    pub auth_token_path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalErrorCode {
    PasswordTooShort,
    PasswordTooLong,
    PasswordConfirmationMismatch,
    RecoveryNotConfirmed,
    SetupNotStarted,
    AccountAlreadyExists,
    AccountMissing,
    AuthenticationFailed,
    AuthenticationThrottled,
    CredentialDisabled,
    RemotePasswordNotAllowed,
    NotAuthenticated,
    AccountServiceUnavailable,
    UnauthorizedClient,
    ProtocolError,
}

#[derive(Debug)]
pub enum LocalTransportError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidTokenFile,
    InsecurePermissions,
    Protocol,
    Remote {
        code: LocalErrorCode,
        retry_after_ms: Option<u64>,
    },
    Supervisor(crate::SupervisorError),
    Account(OperatorAccountError),
}

impl fmt::Display for LocalTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "local transport I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "local transport message was invalid: {error}"),
            Self::InvalidTokenFile => write!(formatter, "local transport token is invalid"),
            Self::InsecurePermissions => {
                write!(
                    formatter,
                    "local transport path permissions are not private"
                )
            }
            Self::Protocol => write!(formatter, "local transport protocol mismatch"),
            Self::Remote { code, .. } => write!(formatter, "local supervisor refused: {code:?}"),
            Self::Supervisor(error) => write!(formatter, "supervisor startup failed: {error}"),
            Self::Account(error) => write!(formatter, "account service failed: {error}"),
        }
    }
}

impl std::error::Error for LocalTransportError {}

impl From<std::io::Error> for LocalTransportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LocalTransportError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<crate::SupervisorError> for LocalTransportError {
    fn from(error: crate::SupervisorError) -> Self {
        Self::Supervisor(error)
    }
}

impl From<OperatorAccountError> for LocalTransportError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

struct LocalAuthToken(Zeroizing<[u8; AUTH_TOKEN_BYTES]>);

impl LocalAuthToken {
    fn generate() -> Result<Self, LocalTransportError> {
        let mut value = Zeroizing::new([0_u8; AUTH_TOKEN_BYTES]);
        getrandom::fill(value.as_mut()).map_err(|_| LocalTransportError::InvalidTokenFile)?;
        Ok(Self(value))
    }

    fn encode(&self) -> Zeroizing<String> {
        Zeroizing::new(BASE32_NOPAD.encode(self.0.as_ref()))
    }

    fn decode(value: &str) -> Result<Self, LocalTransportError> {
        let decoded = BASE32_NOPAD
            .decode(value.as_bytes())
            .map_err(|_| LocalTransportError::InvalidTokenFile)?;
        let token: [u8; AUTH_TOKEN_BYTES] = decoded
            .try_into()
            .map_err(|_| LocalTransportError::InvalidTokenFile)?;
        Ok(Self(Zeroizing::new(token)))
    }

    fn matches_encoded(&self, candidate: &str) -> bool {
        Self::decode(candidate)
            .map(|candidate| bool::from(self.0.ct_eq(candidate.0.as_ref())))
            .unwrap_or(false)
    }
}

pub fn initialize_local_auth_token(path: impl AsRef<Path>) -> Result<(), LocalTransportError> {
    let path = path.as_ref();
    verify_private_parent(path)?;
    if path.exists() {
        load_local_auth_token(path)?;
        return Ok(());
    }

    let token = LocalAuthToken::generate()?;
    let encoded = token.encode();
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(encoded.as_bytes())?;
            file.sync_all()?;
            ensure_private_regular_file(&file)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            load_local_auth_token(path).map(|_| ())
        }
        Err(error) => Err(error.into()),
    }
}

fn load_local_auth_token(path: &Path) -> Result<LocalAuthToken, LocalTransportError> {
    verify_private_parent(path)?;
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    ensure_private_regular_file(&file)?;
    let mut encoded = Zeroizing::new(String::new());
    file.take(128).read_to_string(&mut encoded)?;
    if encoded.len() != 52 {
        return Err(LocalTransportError::InvalidTokenFile);
    }
    LocalAuthToken::decode(&encoded)
}

fn verify_private_parent(path: &Path) -> Result<(), LocalTransportError> {
    let parent = path
        .parent()
        .ok_or(LocalTransportError::InsecurePermissions)?;
    let metadata = fs::metadata(parent)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(LocalTransportError::InsecurePermissions);
    }
    Ok(())
}

fn ensure_private_regular_file(file: &File) -> Result<(), LocalTransportError> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(LocalTransportError::InsecurePermissions);
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct WireSecret(#[serde(with = "zeroizing_string")] Zeroizing<String>);

impl WireSecret {
    fn from_secret(value: SecretString) -> Self {
        Self(value.into_inner())
    }

    fn into_secret(self) -> SecretString {
        SecretString::from_inner(self.0)
    }

    fn from_recovery(value: RecoveryKey) -> Self {
        Self(value.into_inner())
    }

    fn into_recovery(self) -> RecoveryKey {
        RecoveryKey::from_inner(self.0)
    }

    fn expose(&self) -> &str {
        self.0.as_str()
    }
}

mod zeroizing_string {
    use super::*;

    pub fn serialize<S>(value: &Zeroizing<String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(value.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Zeroizing<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Zeroizing::new)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestEnvelope {
    protocol: u16,
    token: WireSecret,
    command: LocalCommand,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
enum LocalCommand {
    Status,
    BeginPasswordSetup {
        password: WireSecret,
        confirmation: WireSecret,
    },
    CompleteOperatorSetup {
        recovery_saved: bool,
    },
    AuthenticatePassword {
        password: WireSecret,
        origin: AccessOrigin,
    },
    RecoverWithKey {
        recovery_key: WireSecret,
        new_password: WireSecret,
        confirmation: WireSecret,
        origin: AccessOrigin,
    },
    Lock,
    RecordOperatorActivity,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseEnvelope {
    protocol: u16,
    result: Option<LocalResult>,
    error: Option<WireError>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum LocalResult {
    SessionStatus { status: OperatorSessionStatus },
    RecoveryKey { recovery_key: WireSecret },
    Empty,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireError {
    code: LocalErrorCode,
    retry_after_ms: Option<u64>,
}

pub struct LocalSupervisorServer {
    listener: UnixListener,
    socket_path: PathBuf,
    _lock_file: File,
    token: LocalAuthToken,
    accounts: OperatorAccountService,
}

impl LocalSupervisorServer {
    pub fn bind(config: LocalServerConfig) -> Result<Self, LocalTransportError> {
        verify_private_parent(&config.socket_path)?;
        initialize_local_auth_token(&config.auth_token_path)?;
        let lock_file = acquire_supervisor_lock(&config.lock_path)?;
        if config.socket_path.exists() {
            let metadata = fs::symlink_metadata(&config.socket_path)?;
            if !metadata.file_type().is_socket() {
                return Err(LocalTransportError::Protocol);
            }
            fs::remove_file(&config.socket_path)?;
        }
        Supervisor::bootstrap(StartupConfig {
            store_path: config.store_path.clone(),
            initial_workspace_name: config.initial_workspace_name,
        })?;
        let accounts = OperatorAccountService::open(&config.store_path)?;
        let token = load_local_auth_token(&config.auth_token_path)?;
        let listener = UnixListener::bind(&config.socket_path)?;
        fs::set_permissions(&config.socket_path, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&config.socket_path)?;
        if !metadata.file_type().is_socket() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(LocalTransportError::InsecurePermissions);
        }
        Ok(Self {
            listener,
            socket_path: config.socket_path,
            _lock_file: lock_file,
            token,
            accounts,
        })
    }

    pub fn serve(self) -> Result<(), LocalTransportError> {
        loop {
            let (stream, _) = self.listener.accept()?;
            self.handle_connection(stream);
        }
    }

    pub fn serve_until(self, running: Arc<AtomicBool>) -> Result<(), LocalTransportError> {
        self.listener.set_nonblocking(true)?;
        while running.load(Ordering::Acquire) {
            match self.listener.accept() {
                Ok((stream, _)) => self.handle_connection(stream),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn handle_connection(&self, mut stream: UnixStream) {
        let response = configure_stream_deadlines(&stream, SERVER_REQUEST_TIMEOUT)
            .and_then(|()| self.read_request(&mut stream))
            .and_then(|request| self.dispatch(request))
            .unwrap_or_else(response_from_transport_error);
        if let Ok(serialized) = serde_json::to_vec(&response) {
            let serialized = Zeroizing::new(serialized);
            let _ = stream.write_all(&serialized);
        }
    }

    fn read_request(&self, stream: &mut UnixStream) -> Result<LocalCommand, LocalTransportError> {
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(MAX_MESSAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(LocalTransportError::Protocol);
        }
        let request: RequestEnvelope = serde_json::from_slice(&bytes)?;
        if request.protocol != PROTOCOL_VERSION {
            return Err(LocalTransportError::Protocol);
        }
        if !self.token.matches_encoded(request.token.expose()) {
            return Err(LocalTransportError::Remote {
                code: LocalErrorCode::UnauthorizedClient,
                retry_after_ms: None,
            });
        }
        Ok(request.command)
    }

    fn dispatch(&self, command: LocalCommand) -> Result<ResponseEnvelope, LocalTransportError> {
        let result = match command {
            LocalCommand::Status => LocalResult::SessionStatus {
                status: self.accounts.status()?,
            },
            LocalCommand::BeginPasswordSetup {
                password,
                confirmation,
            } => LocalResult::RecoveryKey {
                recovery_key: WireSecret::from_recovery(
                    self.accounts
                        .begin_password_setup(password.into_secret(), confirmation.into_secret())?,
                ),
            },
            LocalCommand::CompleteOperatorSetup { recovery_saved } => LocalResult::SessionStatus {
                status: self.accounts.complete_operator_setup(recovery_saved)?,
            },
            LocalCommand::AuthenticatePassword { password, origin } => LocalResult::SessionStatus {
                status: self
                    .accounts
                    .authenticate_password(password.into_secret(), origin)?,
            },
            LocalCommand::RecoverWithKey {
                recovery_key,
                new_password,
                confirmation,
                origin,
            } => LocalResult::RecoveryKey {
                recovery_key: WireSecret::from_recovery(self.accounts.recover_with_key(
                    recovery_key.into_secret(),
                    new_password.into_secret(),
                    confirmation.into_secret(),
                    origin,
                )?),
            },
            LocalCommand::Lock => LocalResult::SessionStatus {
                status: self.accounts.lock()?,
            },
            LocalCommand::RecordOperatorActivity => {
                self.accounts.record_operator_activity()?;
                LocalResult::Empty
            }
        };
        Ok(ResponseEnvelope {
            protocol: PROTOCOL_VERSION,
            result: Some(result),
            error: None,
        })
    }
}

fn acquire_supervisor_lock(path: &Path) -> Result<File, LocalTransportError> {
    verify_private_parent(path)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    ensure_private_regular_file(&file)?;
    use std::os::fd::AsRawFd;
    // The lock is held for the server lifetime. A replacement process can only remove a stale
    // socket after the kernel releases this lock, so two supervisors cannot unlink each other.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

impl Drop for LocalSupervisorServer {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.socket_path);
    }
}

#[derive(Clone)]
pub struct LocalSupervisorClient {
    socket_path: PathBuf,
    token_path: PathBuf,
}

impl LocalSupervisorClient {
    pub fn connect(
        socket_path: impl AsRef<Path>,
        auth_token_path: impl AsRef<Path>,
    ) -> Result<Self, LocalTransportError> {
        load_local_auth_token(auth_token_path.as_ref())?;
        Ok(Self {
            socket_path: socket_path.as_ref().to_owned(),
            token_path: auth_token_path.as_ref().to_owned(),
        })
    }

    pub fn status(&self) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::Status)?)
    }

    pub fn begin_password_setup(
        &self,
        password: SecretString,
        confirmation: SecretString,
    ) -> Result<RecoveryKey, LocalTransportError> {
        expect_recovery(self.send(LocalCommand::BeginPasswordSetup {
            password: WireSecret::from_secret(password),
            confirmation: WireSecret::from_secret(confirmation),
        })?)
    }

    pub fn complete_operator_setup(
        &self,
        recovery_saved: bool,
    ) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::CompleteOperatorSetup { recovery_saved })?)
    }

    pub fn authenticate_password(
        &self,
        password: SecretString,
        origin: AccessOrigin,
    ) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::AuthenticatePassword {
            password: WireSecret::from_secret(password),
            origin,
        })?)
    }

    pub fn recover_with_key(
        &self,
        recovery_key: SecretString,
        new_password: SecretString,
        confirmation: SecretString,
        origin: AccessOrigin,
    ) -> Result<RecoveryKey, LocalTransportError> {
        expect_recovery(self.send(LocalCommand::RecoverWithKey {
            recovery_key: WireSecret::from_secret(recovery_key),
            new_password: WireSecret::from_secret(new_password),
            confirmation: WireSecret::from_secret(confirmation),
            origin,
        })?)
    }

    pub fn lock(&self) -> Result<OperatorSessionStatus, LocalTransportError> {
        expect_status(self.send(LocalCommand::Lock)?)
    }

    pub fn record_operator_activity(&self) -> Result<(), LocalTransportError> {
        match self.send(LocalCommand::RecordOperatorActivity)? {
            LocalResult::Empty => Ok(()),
            _ => Err(LocalTransportError::Protocol),
        }
    }

    fn send(&self, command: LocalCommand) -> Result<LocalResult, LocalTransportError> {
        let token = load_local_auth_token(&self.token_path)?;
        let request = RequestEnvelope {
            protocol: PROTOCOL_VERSION,
            token: WireSecret(token.encode()),
            command,
        };
        let serialized = Zeroizing::new(serde_json::to_vec(&request)?);
        let mut stream = UnixStream::connect(&self.socket_path)?;
        configure_stream_deadlines(&stream, CLIENT_RESPONSE_TIMEOUT)?;
        stream.write_all(&serialized)?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut bytes = Zeroizing::new(Vec::new());
        stream.take(MAX_MESSAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_MESSAGE_BYTES {
            return Err(LocalTransportError::Protocol);
        }
        let response: ResponseEnvelope = serde_json::from_slice(&bytes)?;
        if response.protocol != PROTOCOL_VERSION {
            return Err(LocalTransportError::Protocol);
        }
        match (response.result, response.error) {
            (Some(result), None) => Ok(result),
            (None, Some(error)) => Err(LocalTransportError::Remote {
                code: error.code,
                retry_after_ms: error.retry_after_ms,
            }),
            _ => Err(LocalTransportError::Protocol),
        }
    }
}

fn configure_stream_deadlines(
    stream: &UnixStream,
    timeout: Duration,
) -> Result<(), LocalTransportError> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    Ok(())
}

fn expect_status(result: LocalResult) -> Result<OperatorSessionStatus, LocalTransportError> {
    match result {
        LocalResult::SessionStatus { status } => Ok(status),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn expect_recovery(result: LocalResult) -> Result<RecoveryKey, LocalTransportError> {
    match result {
        LocalResult::RecoveryKey { recovery_key } => Ok(recovery_key.into_recovery()),
        _ => Err(LocalTransportError::Protocol),
    }
}

fn response_from_transport_error(error: LocalTransportError) -> ResponseEnvelope {
    let wire_error = match error {
        LocalTransportError::Account(error) => wire_error_from_account(error),
        LocalTransportError::Remote {
            code,
            retry_after_ms,
        } => WireError {
            code,
            retry_after_ms,
        },
        _ => WireError {
            code: LocalErrorCode::ProtocolError,
            retry_after_ms: None,
        },
    };
    ResponseEnvelope {
        protocol: PROTOCOL_VERSION,
        result: None,
        error: Some(wire_error),
    }
}

fn wire_error_from_account(error: OperatorAccountError) -> WireError {
    let (code, retry_after_ms) = match error {
        OperatorAccountError::PasswordTooShort => (LocalErrorCode::PasswordTooShort, None),
        OperatorAccountError::PasswordTooLong => (LocalErrorCode::PasswordTooLong, None),
        OperatorAccountError::PasswordConfirmationMismatch => {
            (LocalErrorCode::PasswordConfirmationMismatch, None)
        }
        OperatorAccountError::RecoveryNotConfirmed => (LocalErrorCode::RecoveryNotConfirmed, None),
        OperatorAccountError::SetupNotStarted => (LocalErrorCode::SetupNotStarted, None),
        OperatorAccountError::AccountAlreadyExists => (LocalErrorCode::AccountAlreadyExists, None),
        OperatorAccountError::AccountMissing => (LocalErrorCode::AccountMissing, None),
        OperatorAccountError::AuthenticationFailed => (LocalErrorCode::AuthenticationFailed, None),
        OperatorAccountError::AuthenticationThrottled { retry_after_ms } => (
            LocalErrorCode::AuthenticationThrottled,
            Some(retry_after_ms),
        ),
        OperatorAccountError::CredentialDisabled => (LocalErrorCode::CredentialDisabled, None),
        OperatorAccountError::RemotePasswordNotAllowed => {
            (LocalErrorCode::RemotePasswordNotAllowed, None)
        }
        OperatorAccountError::NotAuthenticated => (LocalErrorCode::NotAuthenticated, None),
        _ => (LocalErrorCode::AccountServiceUnavailable, None),
    };
    WireError {
        code,
        retry_after_ms,
    }
}
