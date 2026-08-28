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
    AccessOrigin, AutonomyProfile, LocalErrorCode, LocalServerConfig, LocalSupervisorClient,
    LocalSupervisorServer, LocalTransportError, OperatorSessionPhase, PlatformMonitorState,
    ProjectCharterDraft, ProjectDraft, SecretString, StarterOrganizationDecision,
    StarterResidentDraft, StarterRole, initialize_local_auth_token,
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
fn authenticated_client_decides_the_starter_organization_over_the_real_socket() {
    let runtime = TestRuntime::new("starter-organization");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.starter_organization(),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let proposed = client.starter_organization().unwrap();
    assert_eq!(proposed.decision, StarterOrganizationDecision::Proposed);

    assert_remote_code(
        client.accept_starter_organization(2, vec![draft(StarterRole::Lead, None)]),
        LocalErrorCode::UnsupportedStarterProposal,
    );
    assert_remote_code(
        client.accept_starter_organization(1, vec![]),
        LocalErrorCode::InvalidStarterRoster,
    );
    assert_remote_code(
        client.accept_starter_organization(
            1,
            vec![
                draft(StarterRole::Builder, None),
                draft(StarterRole::Builder, None),
            ],
        ),
        LocalErrorCode::InvalidStarterRoster,
    );
    for invalid_name in ["   ".to_owned(), "line\nbreak".to_owned(), "x".repeat(81)] {
        assert_remote_code(
            client.accept_starter_organization(
                1,
                vec![draft(StarterRole::Lead, Some(&invalid_name))],
            ),
            LocalErrorCode::InvalidStarterRoster,
        );
    }
    assert_eq!(
        client.starter_organization().unwrap().decision,
        StarterOrganizationDecision::Proposed
    );

    let accepted = client
        .accept_starter_organization(
            proposed.proposal_version,
            vec![
                draft(StarterRole::Lead, Some("Northstar")),
                draft(StarterRole::Builder, None),
                draft(StarterRole::Reviewer, None),
            ],
        )
        .unwrap();
    assert_eq!(accepted.decision, StarterOrganizationDecision::Accepted);
    assert_eq!(accepted.residents.len(), 3);
    assert_remote_code(
        client.accept_starter_organization(1, vec![draft(StarterRole::Researcher, None)]),
        LocalErrorCode::StarterDecisionRecorded,
    );

    client.lock().unwrap();
    assert_remote_code(
        client.starter_organization(),
        LocalErrorCode::NotAuthenticated,
    );
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_discards_the_starter_organization_over_the_real_socket() {
    let runtime = TestRuntime::new("discard-starter-organization");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let discarded = client.discard_starter_organization(1).unwrap();
    assert_eq!(discarded.decision, StarterOrganizationDecision::Discarded);
    assert!(discarded.residents.is_empty());
    assert_remote_code(
        client.accept_starter_organization(1, vec![draft(StarterRole::Lead, None)]),
        LocalErrorCode::StarterDecisionRecorded,
    );

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_creates_and_reads_projects_over_the_real_socket() {
    let runtime = TestRuntime::new("projects");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(client.projects(), LocalErrorCode::NotAuthenticated);
    assert_remote_code(
        client.create_project(project_draft("Refused", AutonomyProfile::Supervised)),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    assert!(client.projects().unwrap().is_empty());

    let mut invalid_drafts = Vec::new();
    invalid_drafts.push(ProjectDraft {
        name: "   ".to_owned(),
        ..project_draft("Valid", AutonomyProfile::Observe)
    });
    invalid_drafts.push(ProjectDraft {
        name: "line\nbreak".to_owned(),
        ..project_draft("Valid", AutonomyProfile::Observe)
    });
    let mut empty_purpose = project_draft("Empty purpose", AutonomyProfile::Observe);
    empty_purpose.charter.purpose = "   ".to_owned();
    invalid_drafts.push(empty_purpose);
    let mut too_many_outcomes = project_draft("Too many", AutonomyProfile::Observe);
    too_many_outcomes.charter.desired_outcomes = vec!["Outcome".to_owned(); 21];
    invalid_drafts.push(too_many_outcomes);
    let mut duplicate_outcome = project_draft("Duplicate", AutonomyProfile::Observe);
    duplicate_outcome.charter.desired_outcomes =
        vec!["One result".to_owned(), "  One result  ".to_owned()];
    invalid_drafts.push(duplicate_outcome);
    let mut control_constraint = project_draft("Control", AutonomyProfile::Observe);
    control_constraint.charter.constraints = vec!["No hidden\u{0000}state".to_owned()];
    invalid_drafts.push(control_constraint);
    for invalid in invalid_drafts {
        assert_remote_code(
            client.create_project(invalid),
            LocalErrorCode::InvalidProject,
        );
        assert!(client.projects().unwrap().is_empty());
    }

    let created = client
        .create_project(project_draft("Beacon Alpha", AutonomyProfile::Supervised))
        .unwrap();
    assert_eq!(created.name, "Beacon Alpha");
    assert_eq!(created.charter.revision, 1);
    assert_eq!(client.project(created.id).unwrap(), created);
    assert_eq!(client.projects().unwrap()[0].id, created.id);
    let observed = client
        .create_project(project_draft("Research only", AutonomyProfile::Observe))
        .unwrap();
    assert_eq!(observed.charter.autonomy_profile, AutonomyProfile::Observe);
    assert_remote_code(
        client.project(Uuid::new_v4()),
        LocalErrorCode::ProjectNotFound,
    );
    assert_eq!(client.projects().unwrap().len(), 2);

    client.lock().unwrap();
    assert_remote_code(client.projects(), LocalErrorCode::NotAuthenticated);
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();

    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    assert_remote_code(client.projects(), LocalErrorCode::NotAuthenticated);
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    assert_eq!(client.projects().unwrap().len(), 2);
    assert_eq!(client.project(created.id).unwrap(), created);
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

fn draft(role: StarterRole, display_name: Option<&str>) -> StarterResidentDraft {
    StarterResidentDraft {
        role,
        display_name: display_name.map(str::to_owned),
    }
}

fn project_draft(name: &str, autonomy_profile: AutonomyProfile) -> ProjectDraft {
    ProjectDraft {
        name: name.to_owned(),
        charter: ProjectCharterDraft {
            purpose: "Build a trustworthy local software product.".to_owned(),
            desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
            constraints: vec!["Keep Workspace data local".to_owned()],
            prohibitions: vec!["Do not deploy publicly".to_owned()],
            autonomy_profile,
        },
    }
}

fn assert_remote_code<T>(result: Result<T, LocalTransportError>, expected: LocalErrorCode) {
    match result {
        Err(LocalTransportError::Remote { code, .. }) => assert_eq!(code, expected),
        Ok(_) => panic!("command unexpectedly succeeded"),
        Err(error) => panic!("unexpected transport error: {error}"),
    }
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
