#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use beacond::{
    AccessOrigin, AutonomyProfile, BeaconCapability, BudgetBoundary, DeclaredCheck,
    DiscoveryStatus, LiveDataEncryption, LocalErrorCode, LocalModelActivationState, LocalModelEndpoint,
    LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer, LocalTransportError,
    MAX_RECENT_RUN_WINDOW, MissionAdmissionActor, MissionDraft, MissionLifecycle,
    MissionMemberDraft, ModelActivation, OperatorSessionPhase, PlatformMonitorState, PolicyAuthor,
    ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProviderFeature, RepositoryAuthor,
    RepositoryDraft, RepositorySourceKind, ResidentConfigurationState, ResidentLifecycle,
    SecretString, StarterOrganizationDecision, StarterResidentDraft, StarterRole, TeamDraft,
    TeamFormationActor, TeamIneligibilityReason, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TrackRecordEvidenceKind,
    TrackRecordRequirement, initialize_local_auth_token,
};
use serde_json::json;
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
        self.server_with_model_endpoint(LocalModelEndpoint::ollama_default())
    }

    fn server_with_model_endpoint(
        &self,
        local_model_endpoint: LocalModelEndpoint,
    ) -> LocalSupervisorServer {
        initialize_local_auth_token(&self.token).unwrap();
        LocalSupervisorServer::bind(LocalServerConfig {
            store_path: self.store.clone(),
            initial_workspace_name: "Personal".to_owned(),
            socket_path: self.socket.clone(),
            auth_token_path: self.token.clone(),
            lock_path: self.lock.clone(),
            local_model_endpoint,
        })
        .unwrap()
    }
}

fn serve_ollama_json(body: &'static str) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let size = stream.read(&mut request).unwrap();
        assert!(
            std::str::from_utf8(&request[..size])
                .unwrap()
                .starts_with("GET /api/tags HTTP/1.1\r\n")
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    address
}

const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

fn tags_response() -> String {
    json!({
        "models": [{
            "name": MODEL_NAME,
            "model": MODEL_NAME,
            "modified_at": "2026-08-14T14:38:20.13373148-04:00",
            "size": 17_741_872_154_u64,
            "digest": MODEL_DIGEST,
            "details": {
                "format": "gguf",
                "family": "qwen35",
                "families": ["qwen35"],
                "parameter_size": "27.3B",
                "quantization_level": "Q4_K_M"
            }
        }]
    })
    .to_string()
}

fn serve_ollama_inspections(count: usize) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    thread::spawn(move || {
        let responses = [
            ("GET /api/tags HTTP/1.1\r\n", tags_response()),
            (
                "POST /api/show HTTP/1.1\r\n",
                json!({ "capabilities": ["completion", "tools", "future_audio"] }).to_string(),
            ),
            ("GET /api/tags HTTP/1.1\r\n", tags_response()),
        ];
        for _ in 0..count {
            for (expected, body) in &responses {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(
                    request.starts_with(expected),
                    "unexpected request: {request}"
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    address
}

fn read_http_request(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let size = stream.read(&mut chunk).unwrap();
        assert!(size > 0, "request closed before its declared body arrived");
        request.extend_from_slice(&chunk[..size]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            return String::from_utf8(request).unwrap();
        }
    }
}

#[test]
fn authenticated_client_reviews_activates_and_deactivates_an_exact_local_model() {
    let runtime = TestRuntime::new("model-activation");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.inspect_local_model(MODEL_NAME.to_owned(), MODEL_DIGEST.to_owned()),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();

    let review = client
        .inspect_local_model(MODEL_NAME.to_owned(), MODEL_DIGEST.to_owned())
        .unwrap();
    assert_eq!(
        review.provider_features,
        [ProviderFeature::Completion, ProviderFeature::Tools]
    );
    assert_eq!(review.unrecognized_features, ["future_audio"]);

    let active = client
        .activate_local_model(review, vec![ProviderFeature::Completion])
        .unwrap();
    assert_eq!(active.state, LocalModelActivationState::Active);
    let listed = client.local_model_activations().unwrap();
    assert_eq!(listed.as_slice(), std::slice::from_ref(&active));

    let inactive = client.deactivate_local_model(active.id).unwrap();
    assert_eq!(inactive.state, LocalModelActivationState::Inactive);
    assert_eq!(inactive.predecessor_id, Some(active.id));
    assert_eq!(client.local_model_activations().unwrap(), [inactive]);

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_reads_live_data_encryption_over_both_real_transports() {
    let runtime = TestRuntime::new("disk-encryption");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    // A diagnostic about where live data sits is Workspace material: it is
    // refused before enrolment and again after the session closes.
    assert!(matches!(
        client.live_data_encryption_status(),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));
    let recovery = client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert!(!recovery.expose().is_empty());
    client.complete_operator_setup(true).unwrap();

    let report = client.live_data_encryption_status().unwrap();
    // The host running this test is whatever it is, so the assertion is on the
    // contract rather than on one state: the answer is one of three, it names
    // the volume it is about, and it carries the evidence it read.
    assert!(matches!(
        report.conclusion().state(),
        LiveDataEncryption::Enabled | LiveDataEncryption::Disabled | LiveDataEncryption::Unknown
    ));
    assert!(!report.evidence().is_empty());
    assert!(report.observed_at_ms() > 0);
    // The conclusion is bounded by its own shape, so what crosses the wire can
    // never be a sentence something downstream has to cut.
    assert!(report.conclusion().subjects().len() <= 3);
    for subject in report.conclusion().subjects() {
        assert!(subject.value().chars().count() <= 24);
    }
    let sentence = beacond::render_conclusion(report.conclusion());
    assert!(!sentence.is_empty());
    assert!(sentence.chars().count() <= beacond::MAX_RENDERED_CONCLUSION_CHARS);
    // Nothing about this reading may be presented as Beacon's own encryption.
    let rendered = serde_json::to_string(&report).unwrap();
    assert!(!rendered.contains("Beacon-encrypted"));

    client.lock().unwrap();
    assert!(matches!(
        client.live_data_encryption_status(),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_discovers_inactive_local_models_over_both_real_transports() {
    let runtime = TestRuntime::new("model-discovery");
    let address = serve_ollama_json(
        r#"{"models":[{"name":"qwen3.8:27b","model":"qwen3.8:27b","modified_at":"2026-08-14T14:38:20-04:00","size":17741872154,"digest":"22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643","details":{"format":"gguf","family":"qwen35","families":["qwen35"],"parameter_size":"27.3B","quantization_level":"Q4_K_M"}}]}"#,
    );
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert!(matches!(
        client.discover_local_models(),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));
    let recovery = client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert!(!recovery.expose().is_empty());
    client.complete_operator_setup(true).unwrap();

    let report = client.discover_local_models().unwrap();
    assert_eq!(report.status, DiscoveryStatus::Available);
    assert_eq!(report.models.len(), 1);
    assert_eq!(report.models[0].name, "qwen3.8:27b");
    assert_eq!(report.models[0].activation, ModelActivation::Inactive);

    client.lock().unwrap();
    assert!(matches!(
        client.discover_local_models(),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
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
fn authenticated_client_controls_resident_lifecycle_over_the_real_socket() {
    let runtime = TestRuntime::new("resident-lifecycle");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(client.residents(), LocalErrorCode::NotAuthenticated);
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let accepted = client
        .accept_starter_organization(1, vec![draft(StarterRole::Lead, Some("Northstar"))])
        .unwrap();
    let resident_id = accepted.residents[0].id;

    let initial = client.residents().unwrap();
    assert_eq!(initial.len(), 1);
    assert_eq!(initial[0].lifecycle, ResidentLifecycle::Inactive);
    assert_eq!(initial[0].latest_decision_id, None);
    assert_remote_code(
        client.suspend_resident(resident_id, None),
        LocalErrorCode::InvalidResidentTransition,
    );

    let active = client.activate_resident(resident_id, None).unwrap();
    assert_eq!(active.lifecycle, ResidentLifecycle::Active);
    let active_id = active.latest_decision_id.unwrap();
    assert_remote_code(
        client.activate_resident(resident_id, None),
        LocalErrorCode::StaleResidentDecision,
    );
    assert_eq!(client.residents().unwrap(), std::slice::from_ref(&active));

    let suspended = client
        .suspend_resident(resident_id, Some(active_id))
        .unwrap();
    assert_eq!(suspended.lifecycle, ResidentLifecycle::Suspended);
    assert_remote_code(
        client.suspend_resident(resident_id, Some(active_id)),
        LocalErrorCode::StaleResidentDecision,
    );

    let reactivated = client
        .reactivate_resident(resident_id, suspended.latest_decision_id)
        .unwrap();
    assert_eq!(reactivated.lifecycle, ResidentLifecycle::Active);
    let retired = client
        .retire_resident(resident_id, reactivated.latest_decision_id)
        .unwrap();
    assert_eq!(retired.lifecycle, ResidentLifecycle::Retired);
    assert_remote_code(
        client.reactivate_resident(resident_id, retired.latest_decision_id),
        LocalErrorCode::InvalidResidentTransition,
    );
    assert_remote_code(
        client.retire_resident(Uuid::new_v4(), None),
        LocalErrorCode::ResidentNotFound,
    );
    assert_eq!(client.residents().unwrap(), [retired]);
    assert_eq!(
        client.starter_organization().unwrap().residents[0].lifecycle,
        ResidentLifecycle::Inactive
    );

    client.lock().unwrap();
    assert_remote_code(client.residents(), LocalErrorCode::NotAuthenticated);
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_configures_and_clears_a_resident_over_the_real_socket() {
    let runtime = TestRuntime::new("resident-configuration");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.resident_capability_catalog(),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.resident_configurations(),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.configure_resident(Uuid::new_v4(), None, Uuid::new_v4(), vec![]),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let accepted = client
        .accept_starter_organization(1, vec![draft(StarterRole::Lead, Some("Northstar"))])
        .unwrap();
    let resident_id = accepted.residents[0].id;
    client.activate_resident(resident_id, None).unwrap();
    let review = client
        .inspect_local_model(MODEL_NAME.to_owned(), MODEL_DIGEST.to_owned())
        .unwrap();
    let activation = client
        .activate_local_model(review, vec![ProviderFeature::Completion])
        .unwrap();

    let catalog = client.resident_capability_catalog().unwrap();
    assert_eq!(catalog.len(), 7);
    assert_eq!(catalog[0].capability, BeaconCapability::WorkCreateMission);
    assert!(client.resident_configurations().unwrap().is_empty());
    assert_remote_code(
        client.configure_resident(resident_id, None, activation.id, vec![]),
        LocalErrorCode::InvalidResidentConfiguration,
    );
    assert_remote_code(
        client.configure_resident(
            Uuid::new_v4(),
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        LocalErrorCode::ResidentNotFound,
    );
    let configured = client
        .configure_resident(
            resident_id,
            None,
            activation.id,
            vec![
                BeaconCapability::WorkCreateMission,
                BeaconCapability::WorkFormTeam,
            ],
        )
        .unwrap();
    assert_eq!(configured.state, ResidentConfigurationState::Configured);
    assert_eq!(
        configured.binding.as_ref().unwrap().budget_boundary,
        BudgetBoundary::NoSpend
    );
    assert!(configured.eligible_for_team);
    assert_eq!(
        client.resident_configurations().unwrap(),
        std::slice::from_ref(&configured)
    );

    assert_remote_code(
        client.configure_resident(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        LocalErrorCode::StaleResidentConfiguration,
    );
    let cleared = client
        .clear_resident_configuration(resident_id, configured.id)
        .unwrap();
    assert_eq!(cleared.state, ResidentConfigurationState::Cleared);
    assert!(!cleared.eligible_for_team);
    assert_remote_code(
        client.clear_resident_configuration(resident_id, configured.id),
        LocalErrorCode::StaleResidentConfiguration,
    );
    let reconfigured = client
        .configure_resident(
            resident_id,
            Some(cleared.id),
            activation.id,
            vec![BeaconCapability::ArtifactEvaluate],
        )
        .unwrap();
    assert!(reconfigured.eligible_for_team);

    client.deactivate_local_model(activation.id).unwrap();
    let revoked = client.resident_configurations().unwrap().remove(0);
    assert_eq!(revoked.id, reconfigured.id);
    assert_eq!(
        revoked.ineligibility_reasons,
        [TeamIneligibilityReason::ModelActivationUnavailable]
    );

    client.lock().unwrap();
    assert_remote_code(
        client.resident_configurations(),
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
        client.project_charter_history(Uuid::new_v4()),
        LocalErrorCode::NotAuthenticated,
    );
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
    assert_eq!(
        client.project_charter_history(created.id).unwrap(),
        std::slice::from_ref(&created.charter)
    );
    let revision_two = project_draft("ignored", AutonomyProfile::Observe).charter;
    let revised = client
        .revise_project_charter(created.id, 1, revision_two.clone())
        .unwrap();
    assert_eq!(revised.charter.revision, 2);
    assert_eq!(revised.charter.autonomy_profile, AutonomyProfile::Observe);
    assert_eq!(
        client.project_charter_history(created.id).unwrap(),
        [created.charter.clone(), revised.charter.clone()]
    );
    assert_remote_code(
        client.revise_project_charter(
            created.id,
            1,
            ProjectCharterDraft {
                purpose: "Conflicting intent".to_owned(),
                ..revision_two.clone()
            },
        ),
        LocalErrorCode::StaleCharterRevision,
    );
    assert_remote_code(
        client.revise_project_charter(created.id, 2, revision_two.clone()),
        LocalErrorCode::UnchangedCharter,
    );
    let mut invalid_revision = revision_two.clone();
    invalid_revision.purpose = "   ".to_owned();
    assert_remote_code(
        client.revise_project_charter(created.id, 2, invalid_revision),
        LocalErrorCode::InvalidProject,
    );
    assert_remote_code(
        client.revise_project_charter(Uuid::new_v4(), 1, revision_two.clone()),
        LocalErrorCode::ProjectNotFound,
    );
    let revised_summary = client.projects().unwrap().remove(0);
    assert_eq!(revised_summary.id, created.id);
    assert_eq!(revised_summary.charter_revision, 2);
    assert_eq!(revised_summary.autonomy_profile, AutonomyProfile::Observe);
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
    assert_remote_code(
        client.project_charter_history(created.id),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.revise_project_charter(created.id, 2, revision_two),
        LocalErrorCode::NotAuthenticated,
    );
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
    assert_eq!(client.project(created.id).unwrap(), revised);
    assert_eq!(client.project_charter_history(created.id).unwrap().len(), 2);
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_forms_a_team_and_admits_a_mission_over_the_real_socket() {
    let runtime = TestRuntime::new("teams");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.teams(Uuid::new_v4()),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let resident_id = client
        .accept_starter_organization(1, vec![draft(StarterRole::Lead, Some("Northstar"))])
        .unwrap()
        .residents[0]
        .id;
    client.activate_resident(resident_id, None).unwrap();
    let review = client
        .inspect_local_model(MODEL_NAME.to_owned(), MODEL_DIGEST.to_owned())
        .unwrap();
    let activation = client
        .activate_local_model(review, vec![ProviderFeature::Completion])
        .unwrap();
    let configuration = client
        .configure_resident(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    let project = client
        .create_project(project_draft("Beacon Alpha", AutonomyProfile::Supervised))
        .unwrap();

    assert!(client.teams(project.id).unwrap().is_empty());
    let formed = client
        .form_team(TeamDraft {
            project_id: project.id,
            name: "Alpha Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();
    assert_eq!(formed.formed_by, TeamFormationActor::Operator);
    assert_eq!(
        formed.members[0].formation_configuration_id,
        configuration.id
    );
    assert_eq!(client.team(formed.id).unwrap(), formed);
    assert_eq!(
        client.teams(project.id).unwrap(),
        std::slice::from_ref(&formed)
    );
    assert_remote_code(client.team(Uuid::new_v4()), LocalErrorCode::TeamNotFound);
    assert_remote_code(
        client.form_team(TeamDraft {
            project_id: project.id,
            name: "Duplicate Member".to_owned(),
            members: vec![
                TeamMemberDraft {
                    resident_id,
                    expected_configuration_id: configuration.id,
                },
                TeamMemberDraft {
                    resident_id,
                    expected_configuration_id: configuration.id,
                },
            ],
        }),
        LocalErrorCode::InvalidTeam,
    );

    assert!(client.missions(project.id).unwrap().is_empty());
    let admitted = client
        .admit_mission(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: formed.id,
            desired_outcome: "Ship one reviewed feature".to_owned(),
            success_conditions: vec!["Relevant gates pass".to_owned()],
            constraints: vec!["Keep authority explicit".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: vec![MissionMemberDraft {
                resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();
    assert_eq!(admitted.lifecycle, MissionLifecycle::Admitted);
    assert_eq!(admitted.admitted_by, MissionAdmissionActor::Operator);
    assert_eq!(client.mission(admitted.id).unwrap(), admitted);
    assert_eq!(
        client.missions(project.id).unwrap(),
        std::slice::from_ref(&admitted)
    );
    assert_remote_code(
        client.mission(Uuid::new_v4()),
        LocalErrorCode::MissionNotFound,
    );

    client.lock().unwrap();
    assert_remote_code(client.team(formed.id), LocalErrorCode::NotAuthenticated);
    assert_remote_code(client.teams(project.id), LocalErrorCode::NotAuthenticated);
    assert_remote_code(
        client.mission(admitted.id),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.missions(project.id),
        LocalErrorCode::NotAuthenticated,
    );
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

fn draft(role: StarterRole, display_name: Option<&str>) -> StarterResidentDraft {
    StarterResidentDraft {
        role,
        display_name: display_name.map(str::to_owned),
    }
}

#[test]
fn authenticated_client_publishes_project_policy_over_the_real_socket() {
    let runtime = TestRuntime::new("project-policy");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.project_policy(Uuid::new_v4()),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let project = client
        .create_project(project_draft("Policy Project", AutonomyProfile::Supervised))
        .unwrap();
    assert!(client.project_policy(project.id).unwrap().is_none());

    let draft = ProjectPolicyDraft {
        team_lead_selection: TeamLeadSelectionPolicy {
            required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
            availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
            track_record_requirements: vec![TrackRecordRequirement {
                evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                minimum_count: 1,
            }],
            conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
            ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
        },
    };
    let policy = client
        .publish_project_policy(project.id, project.charter.revision, None, draft)
        .unwrap();
    assert_eq!(policy.authored_by, PolicyAuthor::Operator);
    assert!(policy.applies_to_current_charter);
    assert_eq!(
        client.project_policy(project.id).unwrap(),
        Some(policy.clone())
    );
    assert_eq!(
        client.project_policy_history(project.id).unwrap(),
        std::slice::from_ref(&policy)
    );

    client.lock().unwrap();
    assert_remote_code(
        client.project_policy(project.id),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.project_policy_history(project.id),
        LocalErrorCode::NotAuthenticated,
    );
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn authenticated_client_binds_project_repository_over_the_real_socket() {
    let runtime = TestRuntime::new("project-repository");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert_remote_code(
        client.current_project_repository(Uuid::new_v4()),
        LocalErrorCode::NotAuthenticated,
    );
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let project = client
        .create_project(project_draft(
            "Repository Project",
            AutonomyProfile::Supervised,
        ))
        .unwrap();
    assert!(
        client
            .current_project_repository(project.id)
            .unwrap()
            .is_none()
    );

    // Declaration only: this fabricated path is never opened by this test.
    let draft = RepositoryDraft {
        source_kind: RepositorySourceKind::LocalPath,
        source: "/home/operator/projects/sample-tracker".to_owned(),
        base_revision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_owned(),
        declared_checks: vec![DeclaredCheck {
            name: "pytest".to_owned(),
            argv: vec![
                "pytest".to_owned(),
                "tests/test_invariants.py".to_owned(),
                "-q".to_owned(),
            ],
            working_directory: ".".to_owned(),
        }],
    };
    let repository = client
        .bind_project_repository(project.id, project.charter.revision, None, draft)
        .unwrap();
    assert_eq!(repository.author_kind, RepositoryAuthor::Operator);
    assert_eq!(repository.revision, 1);
    assert!(repository.applies_to_current_charter);
    assert_eq!(
        client.current_project_repository(project.id).unwrap(),
        Some(repository.clone())
    );
    assert_eq!(
        client.project_repository_history(project.id).unwrap(),
        std::slice::from_ref(&repository)
    );

    client.lock().unwrap();
    assert_remote_code(
        client.current_project_repository(project.id),
        LocalErrorCode::NotAuthenticated,
    );
    assert_remote_code(
        client.project_repository_history(project.id),
        LocalErrorCode::NotAuthenticated,
    );
    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
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
        local_model_endpoint: LocalModelEndpoint::ollama_default(),
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

#[test]
fn recent_receipts_are_workspace_scoped_bounded_and_authenticated_over_the_local_transport() {
    // The Query crosses the socket with the bound the daemon applied, not the
    // one the caller asked for, and it refuses a closed session like every
    // other read. An empty Workspace answers with an empty reading rather than
    // a failure: nothing has run yet is not a broken store.
    let runtime = TestRuntime::new("recent-receipts");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    assert!(matches!(
        client.recent_receipts(50),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();

    let page = client.recent_receipts(50).unwrap();
    assert!(page.receipts.is_empty());
    assert_eq!(page.runs_in_flight_in_window, 0);
    assert_eq!(page.runs_unconfirmed_in_window, 0);
    assert_eq!(page.run_window_applied, 50);
    assert!(!page.more_runs_exist);

    // The daemon owns the bound, and says which one it applied.
    let clamped = client.recent_receipts(100_000).unwrap();
    assert_eq!(clamped.run_window_applied, MAX_RECENT_RUN_WINDOW);

    // No payload body crosses the wire, whatever the reading found.
    let rendered = serde_json::to_string(&page).unwrap();
    assert!(!rendered.contains("receiptJson"));

    client.lock().unwrap();
    assert!(matches!(
        client.recent_receipts(50),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::NotAuthenticated,
            ..
        })
    ));

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

#[test]
fn a_store_that_disagrees_with_itself_reaches_the_wire_as_preflight_unavailable() {
    // gpt1's P2: `wire_error_from_recent_receipt` mapped `InvalidStore` to
    // `PreflightUnavailable`, and nothing preserved that. Changing only this
    // mapping to any other code left the whole local-transport target green,
    // because every permanent case was an authenticated, happy, clamped or
    // locked read -- none of them a refusal from the Query itself.
    //
    // Reusing `preflight_unavailable` rather than inventing a code is the
    // decision under guard here, not an implementation detail: a cross-kind
    // read fails for the same two reasons a preflight read does, and the
    // desktop translates the sentence at its own seam.
    let runtime = TestRuntime::new("recent-receipts-invalid-store");
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
    let project = client
        .create_project(project_draft("Beacon Alpha", AutonomyProfile::Supervised))
        .unwrap();

    // A terminal Run with no Receipt: both writers insert the state and the
    // Receipt in one transaction, so this row cannot have been written by this
    // reader. Seeded directly, because no Command can produce it.
    let connection = rusqlite::Connection::open(&runtime.store).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    let workspace_id: String = connection
        .query_row("SELECT id FROM workspaces LIMIT 1", [], |row| row.get(0))
        .unwrap();
    connection
        .execute(
            "INSERT INTO project_repository_revisions (
                 workspace_id, project_id, revision, charter_revision, source_kind, source,
                 base_revision, declared_checks_json, author_kind, authored_at_ms
             ) VALUES (?1, ?2, 1, 1, 'local_path', '/private/beacon/repo',
                       '0123456789012345678901234567890123456789',
                       '[{\"name\":\"unit\"}]', 'operator', 0)",
            rusqlite::params![workspace_id, project.id.to_string()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'refused', 0, 10, 11)",
            rusqlite::params![
                workspace_id,
                Uuid::new_v4().to_string(),
                project.id.to_string()
            ],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        client.recent_receipts(50),
        Err(LocalTransportError::Remote {
            code: LocalErrorCode::PreflightUnavailable,
            ..
        })
    ));

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}
