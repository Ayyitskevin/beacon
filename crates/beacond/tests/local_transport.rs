#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use beacond::{
    AccessOrigin, LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer,
    OperatorSessionPhase, PlatformMonitorState, SecretString, initialize_local_auth_token,
};
use uuid::Uuid;

struct TestRuntime {
    root: PathBuf,
    store: PathBuf,
    socket: PathBuf,
    token: PathBuf,
    lock: PathBuf,
}

impl TestRuntime {
    fn new(label: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("beacon-transport-{label}-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            store: root.join("beacon.sqlite3"),
            socket: root.join("beacond.sock"),
            token: root.join("client.token"),
            lock: root.join("supervisor.lock"),
            root,
        }
    }

    fn server(&self) -> LocalSupervisorServer {
        initialize_local_auth_token(&self.token).unwrap();
        LocalSupervisorServer::bind(LocalServerConfig {
            store_path: self.store.clone(),
            initial_workspace_name: "Personal".to_owned(),
            socket_path: self.socket.clone(),
            auth_token_path: self.token.clone(),
            lock_path: self.lock.clone(),
        })
        .unwrap()
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn token_file_is_private_and_stable() {
    let runtime = TestRuntime::new("token");

    initialize_local_auth_token(&runtime.token).unwrap();
    let first = fs::read(&runtime.token).unwrap();
    initialize_local_auth_token(&runtime.token).unwrap();

    assert_eq!(fs::read(&runtime.token).unwrap(), first);
    assert_eq!(
        fs::symlink_metadata(&runtime.token)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(first.len(), 52);
}

#[test]
fn authenticated_client_completes_account_flow_without_owning_the_store() {
    let runtime = TestRuntime::new("account-flow");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_eq!(
        client.status().unwrap().phase,
        OperatorSessionPhase::OperatorSetupRequired
    );
    let recovery = client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    assert_eq!(
        client.status().unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    assert_eq!(
        client
            .set_session_timeout_minutes(90)
            .unwrap()
            .inactivity_timeout_minutes,
        90
    );
    assert_eq!(
        client.session_policy().unwrap().inactivity_timeout_minutes,
        90
    );
    let monitor = client.platform_session_monitor_status().unwrap();
    assert_eq!(monitor.screen_lock, PlatformMonitorState::Starting);
    assert_eq!(monitor.suspend, PlatformMonitorState::Starting);
    client.lock().unwrap();
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    assert_eq!(recovery.expose().replace('-', "").len(), 32);

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn a_different_capability_token_is_rejected_without_changing_session_state() {
    let runtime = TestRuntime::new("wrong-token");
    let server = runtime.server();
    let wrong_token = runtime.root.join("wrong.token");
    initialize_local_auth_token(&wrong_token).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let intruder = LocalSupervisorClient::connect(&runtime.socket, &wrong_token).unwrap();
    let owner = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert!(intruder.status().is_err());
    assert_eq!(
        owner.status().unwrap().phase,
        OperatorSessionPhase::OperatorSetupRequired
    );

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn a_second_supervisor_cannot_replace_the_live_servers_socket() {
    let runtime = TestRuntime::new("single-writer-lock");
    let server = runtime.server();

    let second = LocalSupervisorServer::bind(LocalServerConfig {
        store_path: runtime.store.clone(),
        initial_workspace_name: "Personal".to_owned(),
        socket_path: runtime.socket.clone(),
        auth_token_path: runtime.token.clone(),
        lock_path: runtime.lock.clone(),
    });

    assert!(second.is_err());
    assert!(runtime.socket.exists());
    drop(server);
}

#[test]
fn an_incomplete_unauthenticated_request_cannot_stall_the_supervisor() {
    let runtime = TestRuntime::new("slow-client");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let slow_client = UnixStream::connect(&runtime.socket).unwrap();
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let (result_sender, result_receiver) = mpsc::channel();
    thread::spawn(move || {
        result_sender.send(client.status()).unwrap();
    });

    let prompt_result = result_receiver.recv_timeout(Duration::from_millis(1_500));
    let completed_promptly = prompt_result.is_ok();
    drop(slow_client);
    let result = match prompt_result {
        Ok(result) => result,
        Err(_) => result_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("valid client should complete after the slow client disconnects"),
    };

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
    assert!(
        result.is_ok(),
        "an incomplete pre-authentication request stalled valid local clients"
    );
    assert!(
        completed_promptly,
        "the valid request completed only after the slow client disconnected"
    );
}
