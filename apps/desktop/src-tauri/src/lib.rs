#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use mosaicd::{
    AccessOrigin, LocalErrorCode, LocalSupervisorClient, LocalTransportError,
    OperatorSessionStatus, SecretString, initialize_local_auth_token,
};
use serde::Serialize;
use tauri::{Manager, State};

struct DesktopState {
    supervisor: LocalSupervisorClient,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PreferredMethod {
    Passkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PasskeyUnavailableReason {
    UnsupportedLinuxWebkitgtk,
    NativeAdapterNotAvailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasskeyCapability {
    available: bool,
    reason: PasskeyUnavailableReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PasswordScope {
    SameMachine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PasswordFallbackCapability {
    available: bool,
    scope: PasswordScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticationCapabilities {
    preferred_method: PreferredMethod,
    passkey: PasskeyCapability,
    password_fallback: PasswordFallbackCapability,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryKeyOutput {
    recovery_key: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommandError {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_ms: Option<u64>,
}

impl From<LocalTransportError> for CommandError {
    fn from(error: LocalTransportError) -> Self {
        let (code, message, retry_after_ms) = match error {
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordTooShort,
                ..
            } => ("password_too_short", "Use at least 15 characters.", None),
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordTooLong,
                ..
            } => (
                "password_too_long",
                "Use no more than 1,024 characters.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::PasswordConfirmationMismatch,
                ..
            } => (
                "password_confirmation_mismatch",
                "The password confirmation does not match.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::RecoveryNotConfirmed,
                ..
            } => (
                "recovery_not_confirmed",
                "Confirm that the Recovery Key was saved offline.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::AuthenticationFailed,
                ..
            } => (
                "authentication_failed",
                "That credential was not accepted.",
                None,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::AuthenticationThrottled,
                retry_after_ms,
            } => (
                "authentication_throttled",
                "Wait before trying again.",
                retry_after_ms,
            ),
            LocalTransportError::Remote {
                code: LocalErrorCode::CredentialDisabled,
                ..
            } => (
                "credential_disabled",
                "Use the other local credential to recover this account.",
                None,
            ),
            _ => (
                "account_service_unavailable",
                "The local account service is unavailable.",
                None,
            ),
        };
        Self {
            code,
            message,
            retry_after_ms,
        }
    }
}

fn authentication_capabilities_value() -> AuthenticationCapabilities {
    AuthenticationCapabilities {
        preferred_method: PreferredMethod::Passkey,
        passkey: PasskeyCapability {
            available: false,
            reason: if cfg!(target_os = "linux") {
                PasskeyUnavailableReason::UnsupportedLinuxWebkitgtk
            } else {
                PasskeyUnavailableReason::NativeAdapterNotAvailable
            },
        },
        password_fallback: PasswordFallbackCapability {
            available: true,
            scope: PasswordScope::SameMachine,
        },
    }
}

fn session_status_value(state: &DesktopState) -> Result<OperatorSessionStatus, CommandError> {
    state.supervisor.status().map_err(Into::into)
}

fn begin_password_setup_value(
    state: &DesktopState,
    password: SecretString,
    confirmation: SecretString,
) -> Result<RecoveryKeyOutput, CommandError> {
    state
        .supervisor
        .begin_password_setup(password, confirmation)
        .map(|recovery_key| RecoveryKeyOutput {
            recovery_key: recovery_key.into_string(),
        })
        .map_err(Into::into)
}

fn complete_operator_setup_value(
    state: &DesktopState,
    recovery_saved: bool,
) -> Result<OperatorSessionStatus, CommandError> {
    state
        .supervisor
        .complete_operator_setup(recovery_saved)
        .map_err(Into::into)
}

// Tauri command registration and typed Result/value transport:
// https://v2.tauri.app/develop/calling-rust/#commands
#[tauri::command]
fn authentication_capabilities() -> AuthenticationCapabilities {
    authentication_capabilities_value()
}

#[tauri::command]
fn session_status(state: State<'_, DesktopState>) -> Result<OperatorSessionStatus, CommandError> {
    session_status_value(&state)
}

#[tauri::command]
fn begin_password_setup(
    state: State<'_, DesktopState>,
    password: String,
    confirmation: String,
) -> Result<RecoveryKeyOutput, CommandError> {
    begin_password_setup_value(
        &state,
        SecretString::new(password),
        SecretString::new(confirmation),
    )
}

#[tauri::command]
fn complete_operator_setup(
    state: State<'_, DesktopState>,
    recovery_saved: bool,
) -> Result<OperatorSessionStatus, CommandError> {
    complete_operator_setup_value(&state, recovery_saved)
}

#[tauri::command]
fn authenticate_password(
    state: State<'_, DesktopState>,
    password: String,
) -> Result<OperatorSessionStatus, CommandError> {
    state
        .supervisor
        .authenticate_password(SecretString::new(password), AccessOrigin::SameMachine)
        .map_err(Into::into)
}

#[tauri::command]
fn recover_operator_account(
    state: State<'_, DesktopState>,
    recovery_key: String,
    new_password: String,
    confirmation: String,
) -> Result<RecoveryKeyOutput, CommandError> {
    state
        .supervisor
        .recover_with_key(
            SecretString::new(recovery_key),
            SecretString::new(new_password),
            SecretString::new(confirmation),
            AccessOrigin::SameMachine,
        )
        .map(|replacement| RecoveryKeyOutput {
            recovery_key: replacement.into_string(),
        })
        .map_err(Into::into)
}

#[tauri::command]
fn lock_session(state: State<'_, DesktopState>) -> Result<OperatorSessionStatus, CommandError> {
    state.supervisor.lock().map_err(Into::into)
}

fn prepare_runtime_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "Beacon runtime path is not a directory",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn supervisor_binary() -> Result<PathBuf, std::io::Error> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("BEACON_MOSAICD_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current = std::env::current_exe()?;
    Ok(current.with_file_name("mosaicd"))
}

fn start_or_connect_supervisor(
    data_dir: &Path,
) -> Result<LocalSupervisorClient, Box<dyn std::error::Error>> {
    let runtime_dir = data_dir.join("runtime");
    prepare_runtime_directory(&runtime_dir)?;
    let socket_path = runtime_dir.join("mosaicd.sock");
    let token_path = runtime_dir.join("client.token");
    let lock_path = runtime_dir.join("supervisor.lock");
    let store_path = data_dir.join("beacon.sqlite3");
    initialize_local_auth_token(&token_path)?;
    let client = LocalSupervisorClient::connect(&socket_path, &token_path)?;
    if client.status().is_ok() {
        return Ok(client);
    }

    let mut log_options = OpenOptions::new();
    log_options
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let log = log_options.open(runtime_dir.join("mosaicd.log"))?;
    log.set_permissions(fs::Permissions::from_mode(0o600))?;
    let log_error = log.try_clone()?;
    let mut child = Command::new(supervisor_binary()?)
        .arg("serve")
        .arg("--store")
        .arg(&store_path)
        .arg("--socket")
        .arg(&socket_path)
        .arg("--auth-token")
        .arg(&token_path)
        .arg("--lock")
        .arg(&lock_path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_error))
        .spawn()?;

    for _ in 0..100 {
        if client.status().is_ok() {
            return Ok(client);
        }
        if let Some(status) = child.try_wait()? {
            return Err(std::io::Error::other(format!(
                "mosaicd exited before accepting local commands: {status}"
            ))
            .into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "mosaicd did not accept local commands within five seconds",
    )
    .into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Tauri 2.11 resolves an application-specific local-data directory from the bundle
            // identifier: https://docs.rs/tauri/2.11.5/tauri/path/struct.PathResolver.html#method.app_local_data_dir
            let data_dir = app.path().app_local_data_dir()?;
            prepare_runtime_directory(&data_dir)?;
            let supervisor = start_or_connect_supervisor(&data_dir)?;
            app.manage(DesktopState { supervisor });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            authentication_capabilities,
            session_status,
            begin_password_setup,
            complete_operator_setup,
            authenticate_password,
            recover_operator_account,
            lock_session
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Beacon Phase 0 desktop shell");
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;

    use mosaicd::{
        LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer, SecretString,
        initialize_local_auth_token,
    };
    use serde_json::Value;
    use uuid::Uuid;

    use super::{
        DesktopState, authentication_capabilities_value, begin_password_setup_value,
        complete_operator_setup_value, prepare_runtime_directory, session_status_value,
    };

    #[test]
    fn app_data_directory_is_owner_only() {
        let root = std::env::temp_dir().join(format!("beacon-data-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o775)).unwrap();

        prepare_runtime_directory(&root).unwrap();

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn main_window_capability_grants_every_registered_account_command() {
        let capability: Value =
            serde_json::from_str(include_str!("../capabilities/main-window.json")).unwrap();
        let permissions = capability["permissions"].as_array().unwrap();
        let permissions = permissions
            .iter()
            .map(|permission| permission.as_str().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            permissions,
            [
                "allow-authentication-capabilities",
                "allow-session-status",
                "allow-begin-password-setup",
                "allow-complete-operator-setup",
                "allow-authenticate-password",
                "allow-recover-operator-account",
                "allow-lock-session",
            ]
        );
    }

    struct TestState {
        state: DesktopState,
        root: PathBuf,
        running: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl TestState {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("beacon-desktop-{}", Uuid::new_v4()));
            prepare_runtime_directory(&root).unwrap();
            let socket = root.join("mosaicd.sock");
            let token = root.join("client.token");
            initialize_local_auth_token(&token).unwrap();
            let server = LocalSupervisorServer::bind(LocalServerConfig {
                store_path: root.join("beacon.sqlite3"),
                initial_workspace_name: "Personal".to_owned(),
                socket_path: socket.clone(),
                auth_token_path: token.clone(),
                lock_path: root.join("supervisor.lock"),
            })
            .unwrap();
            let running = Arc::new(AtomicBool::new(true));
            let server_running = Arc::clone(&running);
            let thread = std::thread::spawn(move || server.serve_until(server_running).unwrap());
            let supervisor = LocalSupervisorClient::connect(socket, token).unwrap();
            Self {
                state: DesktopState { supervisor },
                root,
                running,
                thread: Some(thread),
            }
        }
    }

    impl Drop for TestState {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Release);
            self.thread.take().unwrap().join().unwrap();
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn signed_out_status_exposes_no_workspace_data_or_operator_command() {
        let state = TestState::new();
        let status = serde_json::to_value(session_status_value(&state.state).unwrap()).unwrap();

        assert_eq!(status["phase"], "operator_setup_required");
        assert_eq!(status["workspaceAccess"], false);
        assert_eq!(status["operatorCommands"], false);
        assert_eq!(
            status
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["operatorCommands", "phase", "workspaceAccess"]
        );
        assert!(!contains_workspace_material(&status));
    }

    #[test]
    fn capability_output_is_explicit_about_the_missing_native_passkey_adapter() {
        let capabilities = serde_json::to_value(authentication_capabilities_value()).unwrap();

        assert_eq!(capabilities["preferredMethod"], "passkey");
        assert_eq!(capabilities["passkey"]["available"], false);
        assert_eq!(
            capabilities["passkey"]["reason"],
            "unsupported_linux_webkitgtk"
        );
        assert_eq!(capabilities["passwordFallback"]["scope"], "same_machine");
        assert_eq!(
            capabilities
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            ["passkey", "passwordFallback", "preferredMethod"]
        );
    }

    #[test]
    fn command_adapter_returns_the_recovery_key_once_then_an_exact_unlocked_status() {
        let state = TestState::new();
        let output = begin_password_setup_value(
            &state.state,
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
        let recovery = serde_json::to_value(output).unwrap();
        assert_eq!(recovery.as_object().unwrap().len(), 1);
        assert_eq!(
            recovery["recoveryKey"]
                .as_str()
                .unwrap()
                .replace('-', "")
                .len(),
            32
        );

        let status =
            serde_json::to_value(complete_operator_setup_value(&state.state, true).unwrap())
                .unwrap();
        assert_eq!(status["phase"], "unlocked");
        assert_eq!(status["workspaceAccess"], true);
        assert_eq!(status["operatorCommands"], true);
        assert_eq!(status.as_object().unwrap().len(), 3);
    }

    fn contains_workspace_material(value: &Value) -> bool {
        value.to_string().to_lowercase().contains("workspace_id")
            || value.to_string().to_lowercase().contains("workspace_name")
    }
}
