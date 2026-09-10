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
    AccessOrigin, AutonomyProfile, BeaconCapability, BudgetBoundary, ClassificationIntentRequest,
    CompletionPreview, DeclaredCheck, DiscoveryStatus, LiveDataEncryption, LocalErrorCode,
    LocalModelActivationState, LocalModelEndpoint, LocalServerConfig, LocalSupervisorClient,
    LocalSupervisorServer, LocalTransportError, MAX_RECENT_RUN_WINDOW, MissionAdmissionActor,
    MissionCompletionCommandService, MissionDraft, MissionLifecycle, MissionMemberDraft,
    ModelActivation, OperatorAccountService, OperatorSessionPhase, PlatformMonitorState,
    PolicyAuthor, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProviderFeature,
    RepositoryAuthor, RepositoryDraft, RepositorySourceKind, ResidentConfigurationState,
    ResidentLifecycle, SecretString, StarterOrganizationDecision, StarterResidentDraft,
    StarterRole, SupersedesDto, TeamDraft, TeamFormationActor, TeamIneligibilityReason,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TeamMemberDraft, TrackRecordEvidenceKind, TrackRecordRequirement, initialize_local_auth_token,
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
    /// The unique directory is named by the UUID alone, and its length does NOT depend on `label`.
    ///
    /// A Unix socket path is bounded by `SUN_LEN` (108 bytes including the NUL, so 107 usable),
    /// and this fixture's socket lives inside that directory. The old basename was
    /// `beacon-transport-{label}-{uuid}`, so every descriptive character a test added to its label
    /// came straight off the budget: at main `b8a5f83` a 13-byte TMPDIR produced 109/110-byte paths
    /// and the server refused to bind with `Io(InvalidInput, "path must be shorter than SUN_LEN")`,
    /// while the same bytes under a 9-byte TMPDIR produced 105/106 and passed. That is a fixture
    /// defect, not a transport fault, and it is why `label` no longer reaches the path.
    ///
    /// **Remaining TMPDIR budget on Linux: 58 bytes.** The path is
    /// `{TMPDIR}/bt-{32 hex}/beacond.sock` = `len(TMPDIR) + 1 + 35 + 1 + 12`, so
    /// `len(TMPDIR) <= 58` fits — ample for both 13-byte reproduction roots — and anything longer
    /// cannot be promised support, because the kernel's limit is on the whole path and no fixture
    /// can shorten someone else's TMPDIR.
    ///
    /// **The WHOLE v4 UUID is kept**, all 32 hex digits: shortening the basename is what this fix
    /// is for, and spending collision resistance to do it would be paying with the wrong currency.
    /// `label` remains the argument every caller passes and still names the test in its own output;
    /// it simply no longer decides whether a socket can be bound.
    fn new(label: &str) -> Self {
        let _ = label;
        let root = std::env::temp_dir().join(format!("bt-{}", Uuid::new_v4().simple()));
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
    // Option (b) is an interface property, not a fixture assumption: v1 never
    // promotes the detector candidate into a confident encryption claim.
    assert_eq!(report.conclusion().state(), LiveDataEncryption::Unknown);
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

/// The three completion commands over the REAL socket, with a record wide enough that the old
/// 64 KiB frame could not have carried it.
///
/// This is the bar the ceiling exists for: every field, every citation and every narrative item
/// survives the round trip, the reviewed token the Operator saw is the token the write is judged
/// against, and the same token reads back afterwards. Sets are compared as SETS, because the owner
/// sorts support and delivered Artifacts into its canonical order -- "unchanged" means nothing was
/// dropped or truncated, not that the Operator's typing order is preserved.
#[test]
fn the_completion_commands_round_trip_a_wide_record_over_the_real_socket() {
    let runtime = TestRuntime::new("completion");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
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
    // Twenty declared conditions: the owner's cap, so the completion below is as wide as one can be.
    let conditions: Vec<String> = (0..20)
        .map(|index| format!("Declared condition {index} holds"))
        .collect();
    let admitted = client
        .admit_mission(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: formed.id,
            desired_outcome: "Ship one reviewed feature".to_owned(),
            success_conditions: conditions.clone(),
            constraints: vec!["Keep authority explicit".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: vec![MissionMemberDraft {
                resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();

    let request = wide_completion_request(admitted.id, resident_id);
    let sent_bytes = serde_json::to_vec(&request).unwrap().len();
    assert!(
        sent_bytes > 65_536,
        "C4.WIDE.REQUEST_EXCEEDS_THE_OLD_FRAME: {sent_bytes} bytes"
    );

    // ---------------------------------------------------------------------------------------
    // PREMISE, before any transport claim: the OWNER and its codec accept this exact request.
    //
    // Without this the wide bar would prove only that something crossed a socket -- a refusal from
    // the owner and a refusal from the transport would look alike. A second account service on the
    // same store, authenticated the same way, calls `preview` directly.
    // ---------------------------------------------------------------------------------------
    let direct_accounts = Arc::new(OperatorAccountService::open(&runtime.store).unwrap());
    direct_accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    let owner =
        MissionCompletionCommandService::new(Arc::clone(&direct_accounts), project.workspace_id);
    let direct_preview: CompletionPreview = owner
        .preview(request.clone())
        .expect("C4.PREMISE.OWNER_ACCEPTS_THE_WIDE_REQUEST");
    assert_eq!(
        direct_preview.intent_sha256.len(),
        64,
        "C4.PREMISE.OWNER_TOKEN_IS_HEX64"
    );
    assert_eq!(
        owner.get_intent(admitted.id).unwrap(),
        None,
        "C4.PREMISE.PREVIEW_WROTE_NOTHING"
    );

    // ---------------------------------------------------------------------------------------
    // The PUBLIC boundary, in bytes. The envelope is assembled here rather than by the client, so
    // this measures what a caller actually sends and what the supervisor actually answers.
    //
    // The auth token is read from its file and never printed: no assertion message, no failure dump
    // and no log line below carries it.
    // ---------------------------------------------------------------------------------------
    let token = fs::read_to_string(&runtime.token).unwrap();
    let request_bytes = serde_json::to_vec(&json!({
        "protocol": 1,
        "token": token,
        "command": {
            "name": "preview_mission_completion",
            "request": serde_json::to_value(&request).unwrap(),
        },
    }))
    .unwrap();
    assert!(
        request_bytes.len() > 65_536,
        "C4.RAW.REQUEST_IS_OVER_THE_OLD_FRAME: {} bytes",
        request_bytes.len()
    );
    assert!(
        request_bytes.len() <= 163_840,
        "C4.RAW.REQUEST_IS_WITHIN_THE_COMPLETION_CEILING: {} bytes",
        request_bytes.len()
    );

    let mut raw = UnixStream::connect(&runtime.socket).unwrap();
    raw.write_all(&request_bytes)
        .unwrap_or_else(|error| panic!("C4.RAW.SUPERVISOR_READS_THE_WIDE_BODY: write {error}"));
    raw.shutdown(std::net::Shutdown::Write).unwrap();
    let mut answer = Vec::new();
    // Named, because a refused oversize body arrives here as a RESET rather than as a refusal
    // envelope: an anonymous `unwrap` would report a ceiling regression as an I/O accident.
    raw.read_to_end(&mut answer)
        .unwrap_or_else(|error| panic!("C4.RAW.SUPERVISOR_READS_THE_WIDE_BODY: read {error}"));
    assert!(
        answer.len() > 65_536,
        "C4.RAW.RESPONSE_IS_OVER_THE_OLD_FRAME: {} bytes",
        answer.len()
    );
    assert!(
        answer.len() <= 196_608,
        "C4.RAW.RESPONSE_IS_WITHIN_THE_PREVIEW_CEILING: {} bytes",
        answer.len()
    );

    // Parsed comparison, not a byte-order promise: JSON field order and whitespace are not a record
    // identity, and asserting them would invent a contract nobody agreed. The expected object is the
    // OWNER's own preview from the premise above -- so this compares what the transport delivered
    // against what the owner produced, not against itself.
    let answered: serde_json::Value = serde_json::from_slice(&answer).unwrap();
    assert_eq!(
        answered.get("error"),
        Some(&serde_json::Value::Null),
        "C4.RAW.NO_REFUSAL"
    );
    assert_eq!(
        answered
            .pointer("/result/kind")
            .and_then(serde_json::Value::as_str),
        Some("mission_completion_preview"),
        "C4.RAW.RESULT_KIND"
    );
    assert_eq!(
        answered.pointer("/result/preview"),
        Some(&serde_json::to_value(&direct_preview).unwrap()),
        "C4.RAW.PREVIEW_EQUALS_THE_OWNERS_OWN_PREVIEW"
    );

    // And the literals themselves, in the delivered TEXT BYTES: a differential check alone would
    // pass if both sides lost the same content.
    let text = String::from_utf8(answer.clone()).unwrap();
    for literal in [
        &"s".repeat(500),
        &"r".repeat(500),
        &"n".repeat(500),
        &"y".repeat(2_000),
        &format!("{:064x}", 10_000u128),
        // condition 19, slot 0: an Artifact citation, so this digest is in the record verbatim.
        &format!("{:064x}", 19u128 * 100),
        &admitted.id.to_string(),
        &resident_id.to_string(),
        &configuration.id.to_string(),
        &project.workspace_id.to_string(),
        &project.id.to_string(),
        &direct_preview.intent_sha256,
    ] {
        assert!(
            text.contains(literal.as_str()),
            "C4.RAW.LITERAL_SURVIVES_THE_BOUNDARY: {} chars missing",
            literal.chars().count()
        );
    }

    let preview = client.preview_mission_completion(request.clone()).unwrap();
    assert_eq!(preview, direct_preview, "C4.WIDE.CLIENT_AND_OWNER_AGREE");

    // Bindings the client never sent and cannot forge.
    assert_eq!(preview.actor, "operator", "C4.WIDE.ACTOR");
    assert_eq!(preview.project_id, project.id, "C4.WIDE.PROJECT_BINDING");
    assert_eq!(
        preview.accountable_resident_configuration_id, configuration.id,
        "C4.WIDE.HISTORICAL_CONFIGURATION"
    );
    assert_eq!(
        preview.conditions, conditions,
        "C4.WIDE.DECLARED_CONDITIONS"
    );
    assert_eq!(preview.intent_sha256.len(), 64, "C4.WIDE.TOKEN_IS_HEX64");

    // Nothing dropped and nothing truncated. Scalar fields must be identical; the two SET fields are
    // compared after sorting both sides, because the owner owns their order.
    let echoed = &preview.request;
    assert_eq!(echoed.mission_id, request.mission_id, "C4.WIDE.MISSION_ID");
    assert_eq!(
        echoed.accountable_resident_id, request.accountable_resident_id,
        "C4.WIDE.ACCOUNTABLE_RESIDENT"
    );
    assert_eq!(
        echoed.disposition, request.disposition,
        "C4.WIDE.DISPOSITION"
    );
    assert_eq!(echoed.summary, request.summary, "C4.WIDE.SUMMARY");
    assert_eq!(echoed.spend, request.spend, "C4.WIDE.SPEND");
    assert_eq!(
        echoed.unfinished_work, request.unfinished_work,
        "C4.WIDE.UNFINISHED_WORK"
    );
    assert_eq!(
        echoed.uncertainty, request.uncertainty,
        "C4.WIDE.UNCERTAINTY"
    );
    assert_eq!(
        echoed.condition_results.len(),
        request.condition_results.len(),
        "C4.WIDE.CONDITION_COUNT"
    );
    for (echoed_result, sent_result) in echoed
        .condition_results
        .iter()
        .zip(request.condition_results.iter())
    {
        assert_eq!(
            echoed_result.ordinal, sent_result.ordinal,
            "C4.WIDE.ORDINAL"
        );
        assert_eq!(
            echoed_result.outcome, sent_result.outcome,
            "C4.WIDE.OUTCOME"
        );
        assert_eq!(echoed_result.note, sent_result.note, "C4.WIDE.NOTE");
        assert_eq!(
            echoed_result.support.len(),
            sent_result.support.len(),
            "C4.WIDE.SUPPORT_COUNT"
        );
        let mut echoed_support = format_support(&echoed_result.support);
        let mut sent_support = format_support(&sent_result.support);
        echoed_support.sort();
        sent_support.sort();
        assert_eq!(echoed_support, sent_support, "C4.WIDE.SUPPORT_SET");
    }
    let mut echoed_delivered = echoed.delivered_artifacts.clone();
    let mut sent_delivered = request.delivered_artifacts.clone();
    echoed_delivered.sort();
    sent_delivered.sort();
    assert_eq!(echoed_delivered, sent_delivered, "C4.WIDE.DELIVERED_SET");

    // Nothing is recorded by a preview.
    assert_eq!(
        client.get_mission_completion_intent(admitted.id).unwrap(),
        None,
        "C4.WIDE.PREVIEW_WRITES_NOTHING"
    );

    // The REVIEWED token is what the write is judged against, and the frozen normalized request is
    // what is sent.
    let acknowledgement = client
        .record_mission_completion(echoed.clone(), preview.intent_sha256.clone())
        .unwrap_or_else(|error| {
            // `Display` is the closed transport line; the request and the token never reach it.
            panic!("C4.WIDE.RECORD_ACCEPTS_THE_REVIEWED_TOKEN: {error}")
        });
    assert_eq!(
        acknowledgement.intent_sha256, preview.intent_sha256,
        "C4.WIDE.ACKNOWLEDGEMENT_IS_THE_REVIEWED_TOKEN"
    );
    assert_eq!(
        client
            .get_mission_completion_intent(admitted.id)
            .unwrap()
            .map(|intent| intent.intent_sha256),
        Some(preview.intent_sha256.clone()),
        "C4.WIDE.READ_BACK_IS_THE_SAME_TOKEN"
    );

    // A second write does not overwrite, and says so by its own name.
    assert_remote_code(
        client.record_mission_completion(echoed.clone(), preview.intent_sha256.clone()),
        LocalErrorCode::CompletionAlreadyRecorded,
    );
    // A stale reviewed token is refused as a changed intent, not as a malformed one.
    assert_remote_code(
        client.record_mission_completion(echoed.clone(), "0".repeat(64)),
        LocalErrorCode::CompletionIntentChanged,
    );

    running.store(false, Ordering::Release);
    thread.join().unwrap().unwrap();
}

/// A stable, comparable spelling of one support reference, so a SET can be sorted without ordering
/// the DTO itself.
fn format_support(support: &[beacond::SupportRefDto]) -> Vec<String> {
    support
        .iter()
        .map(|reference| match reference {
            beacond::SupportRefDto::Artifact(digest) => format!("artifact:{digest}"),
            beacond::SupportRefDto::Evaluation(digest) => format!("evaluation:{digest}"),
            beacond::SupportRefDto::Ruling(digest) => format!("ruling:{digest}"),
            beacond::SupportRefDto::Receipt {
                run_id,
                receipt_version,
                receipt_bytes_sha256,
            } => format!("receipt:{run_id}:{receipt_version}:{receipt_bytes_sha256}"),
        })
        .collect()
}

/// The widest completion this Mission can carry: every list at the owner's cap, every digest and run
/// id UNIQUE (a repeated entry is a duplicate the owner refuses, not a wider record), and every text
/// at its scalar bound.
fn wide_completion_request(
    mission_id: Uuid,
    accountable_resident_id: Uuid,
) -> beacond::CompletionIntentRequest {
    let digest = |seed: u128| format!("{seed:064x}");
    let condition_results = (0..20u32)
        .map(|ordinal| beacond::ConditionResultDto {
            ordinal,
            outcome: "assessment_inconclusive".to_owned(),
            support: (0..8u128)
                .map(|slot| {
                    let seed = u128::from(ordinal) * 100 + slot;
                    match slot % 4 {
                        0 => beacond::SupportRefDto::Artifact(digest(seed)),
                        1 => beacond::SupportRefDto::Evaluation(digest(seed)),
                        2 => beacond::SupportRefDto::Ruling(digest(seed)),
                        _ => beacond::SupportRefDto::Receipt {
                            run_id: Uuid::from_u128(seed + 1),
                            receipt_version: u32::MAX,
                            receipt_bytes_sha256: digest(seed + 2),
                        },
                    }
                })
                .collect(),
            note: Some("n".repeat(500)),
        })
        .collect();
    let narrative = |kind: &str| {
        (0..32u32)
            .map(|ordinal| beacond::NarrativeItemDto {
                ordinal,
                kind: kind.to_owned(),
                condition_ordinal: Some(ordinal % 20),
                summary: "s".repeat(500),
            })
            .collect::<Vec<_>>()
    };
    beacond::CompletionIntentRequest {
        mission_id,
        accountable_resident_id,
        disposition: "concluded".to_owned(),
        condition_results,
        delivered_artifacts: (0..32u128).map(|index| digest(index + 10_000)).collect(),
        unfinished_work: narrative("blocked_dependency"),
        uncertainty: narrative("outside_scope_of_checks"),
        spend: beacond::SpendDto::NotMeasured("r".repeat(500)),
        summary: "y".repeat(2_000),
    }
}

/// C5's two commands are KNOWN to the wire, and answer with the account's own refusal.
///
/// The smallest real socket bar for this slice, and the one that must be RED before any wiring
/// exists: today neither name parses, so the supervisor answers `protocol_error` from its own
/// `Json` refusal. A command that is merely unknown and a command that exists and refused the caller
/// are different answers, and this is the difference the wiring has to make.
///
/// Deliberately raw bytes rather than a client method: at the moment this test is written there is no
/// client method to call, and a test that cannot compile cannot be RED by a named behaviour.
#[test]
fn the_classification_commands_are_known_to_the_wire_and_authenticated() {
    let runtime = TestRuntime::new("c5route");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    // The token is read from its file and never printed: no assertion message below carries it.
    let token = fs::read_to_string(&runtime.token).unwrap();

    let mission_id = Uuid::new_v4();
    let cases: [(&str, serde_json::Value); 2] = [
        (
            "classify_mission_impact",
            json!({
                "request": {
                    "missionId": mission_id,
                    "impact": "ordinary",
                    "reason": "No operator-visible surface changed.",
                    "expectedPolicyRevision": null,
                    "supersedes": { "kind": "none" },
                },
            }),
        ),
        (
            "get_mission_classification",
            json!({ "mission_id": mission_id }),
        ),
    ];

    for (name, fields) in cases {
        let mut command = serde_json::Map::new();
        command.insert("name".to_owned(), serde_json::Value::from(name));
        for (key, value) in fields.as_object().expect("the fields are an object") {
            command.insert(key.clone(), value.clone());
        }
        let body = serde_json::to_vec(&json!({
            "protocol": 1,
            "token": token,
            "command": serde_json::Value::Object(command),
        }))
        .unwrap();

        let mut stream = UnixStream::connect(&runtime.socket).unwrap();
        stream.write_all(&body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        let answered: serde_json::Value = serde_json::from_slice(&answer).unwrap();

        assert_eq!(
            answered
                .pointer("/error/code")
                .and_then(serde_json::Value::as_str),
            Some("not_authenticated"),
            "C5T.ROUTE.IS_A_KNOWN_COMMAND: {name}"
        );
        assert_eq!(
            answered.get("result"),
            Some(&serde_json::Value::Null),
            "C5T.ROUTE.A_REFUSAL_CARRIES_NO_RESULT: {name}"
        );
    }

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The classification write and its bounded observation, end to end on a real store and socket.
///
/// One Mission, one Project, one published Policy, through the public services that mint real records.
/// Everything asserted here came back over the socket from the REAL owner; nothing is stubbed.
#[test]
fn classification_crosses_the_socket_and_its_observation_is_bounded() {
    let runtime = TestRuntime::new("c5live");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
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
    let admitted = client
        .admit_mission(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: formed.id,
            desired_outcome: "Ship one reviewed feature".to_owned(),
            success_conditions: vec!["Relevant gates pass".to_owned()],
            constraints: vec![],
            prohibitions: vec![],
            members: vec![MissionMemberDraft {
                resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();

    // Before any Policy: the observation is four fields, all proved absences except its own cursor.
    let empty = client.get_mission_classification(admitted.id).unwrap();
    assert_eq!(empty.mission_id, admitted.id, "C5T.OBSERVE.ITS_OWN_MISSION");
    assert_eq!(empty.latest, None, "C5T.OBSERVE.ABSENCE_IS_PROVED");
    assert_eq!(
        empty.published_policy, None,
        "C5T.OBSERVE.NO_POLICY_IS_ITS_OWN_ANSWER"
    );
    assert!(
        empty
            .observed_at_event_seq
            .chars()
            .all(|character| character.is_ascii_digit()),
        "C5T.OBSERVE.THE_CURSOR_IS_A_CANONICAL_DECIMAL_STRING: {:?}",
        empty.observed_at_event_seq
    );

    // Publishing a Policy is the prerequisite the surface routes to, and the observation reports it.
    let policy = client
        .publish_project_policy(
            project.id,
            project.charter.revision,
            None,
            ProjectPolicyDraft {
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
            },
        )
        .unwrap();
    let with_policy = client.get_mission_classification(admitted.id).unwrap();
    let cited = with_policy
        .published_policy
        .clone()
        .expect("C5T.OBSERVE.A_PUBLISHED_POLICY_IS_REPORTED");
    assert_eq!(
        (
            cited.project_id,
            cited.policy_revision,
            cited.charter_revision
        ),
        (project.id, policy.revision, policy.charter_revision),
        "C5T.OBSERVE.THE_PUBLISHED_POLICY_IS_THE_STORES"
    );

    // The write: the Operator's five keys, and nothing else.
    let recorded = client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "high".to_owned(),
            reason: "Touches the operator credential path.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| panic!("C5T.CLASSIFY.THE_OWNER_ACCEPTS_THE_REQUEST: {error}"));
    assert_eq!(recorded.version, 1, "C5T.CLASSIFY.THE_FIRST_VERSION_IS_ONE");
    assert_eq!(
        recorded.impact, "high",
        "C5T.CLASSIFY.THE_JUDGMENT_IS_THE_OPERATORS"
    );
    assert_eq!(
        recorded.reason, "Touches the operator credential path.",
        "C5T.CLASSIFY.THE_REASON_CROSSES_VERBATIM"
    );
    assert_eq!(
        recorded.basis, "operator_declared",
        "C5T.CLASSIFY.A_NEW_WRITE_IS_OPERATOR_DECLARED"
    );
    assert_eq!(
        recorded.rule_id, None,
        "C5T.CLASSIFY.A_NEW_WRITE_NAMES_NO_RULE"
    );
    assert_eq!(
        recorded.classified_by, "operator",
        "C5T.CLASSIFY.THE_ACTOR_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            recorded.policy.policy_revision,
            recorded.policy.charter_revision
        ),
        (policy.revision, policy.charter_revision),
        "C5T.CLASSIFY.THE_CITATION_IS_THE_STORES"
    );
    assert_eq!(
        recorded.supersedes,
        SupersedesDto::None,
        "C5T.CLASSIFY.IT_REPLACES_NOTHING"
    );
    assert_eq!(
        recorded.digest.canonicalizer, "beacon.mission-impact.v1",
        "C5T.CLASSIFY.THE_DIGEST_CARRIES_ITS_CANONICALIZER"
    );

    // The observation now reports it, whole, and the cursor has moved forward.
    let after = client.get_mission_classification(admitted.id).unwrap();
    assert_eq!(
        after.latest.as_ref(),
        Some(&recorded),
        "C5T.OBSERVE.THE_LATEST_IS_THE_RECORD_THAT_WAS_WRITTEN"
    );
    assert!(
        after.observed_at_event_seq.parse::<u64>().unwrap()
            > with_policy.observed_at_event_seq.parse::<u64>().unwrap(),
        "C5T.OBSERVE.THE_CURSOR_MOVES_WITH_THE_CHAIN"
    );

    // A second `supersedes: none` is refused by name, and the reclassification names its predecessor.
    assert_remote_code(
        client.classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "ordinary".to_owned(),
            reason: "A second first classification.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::None,
        }),
        LocalErrorCode::ClassificationAlreadyClassified,
    );
    let reclassified = client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "ordinary".to_owned(),
            reason: "A later review found no operator-visible surface.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::Record {
                predecessor_digest: recorded.digest.value.clone(),
            },
        })
        .unwrap_or_else(|error| panic!("C5T.RECLASSIFY.THE_PREDECESSOR_IS_ACCEPTED: {error}"));
    assert_eq!(
        reclassified.version, 2,
        "C5T.RECLASSIFY.THE_LADDER_ADVANCES"
    );
    assert_eq!(
        reclassified.supersedes,
        SupersedesDto::Record {
            predecessor_digest: recorded.digest.value.clone()
        },
        "C5T.RECLASSIFY.IT_NAMES_WHAT_IT_REPLACES"
    );
    // And naming the SUPERSEDED record again is refused by its own name.
    assert_remote_code(
        client.classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "high".to_owned(),
            reason: "Naming a predecessor that is no longer latest.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::Record {
                predecessor_digest: recorded.digest.value.clone(),
            },
        }),
        LocalErrorCode::ClassificationPredecessorNotLatest,
    );

    // Current versus historical Policy: revise the Charter, publish revision 2, and the observation
    // reports the NEW published Policy beside a record that still cites the OLD one.
    let revised = client
        .revise_project_charter(
            project.id,
            project.charter.revision,
            ProjectCharterDraft {
                purpose: "Prove the classification transport.".to_owned(),
                desired_outcomes: vec!["Classification crosses the socket".to_owned()],
                constraints: vec![],
                prohibitions: vec![],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        )
        .unwrap();
    let second_policy = client
        .publish_project_policy(
            project.id,
            revised.charter.revision,
            Some(policy.revision),
            ProjectPolicyDraft {
                team_lead_selection: TeamLeadSelectionPolicy {
                    required_capabilities: vec![BeaconCapability::WorkCoordinateMission],
                    availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
                    track_record_requirements: vec![TrackRecordRequirement {
                        evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                        minimum_count: 2,
                    }],
                    conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
                    ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
                },
            },
        )
        .unwrap();
    let moved = client.get_mission_classification(admitted.id).unwrap();
    assert_eq!(
        moved
            .published_policy
            .as_ref()
            .map(|policy| policy.policy_revision),
        Some(second_policy.revision),
        "C5T.OBSERVE.THE_PUBLISHED_POLICY_IS_THE_CURRENT_ONE"
    );
    assert_eq!(
        moved
            .latest
            .as_ref()
            .map(|record| record.policy.policy_revision),
        Some(policy.revision),
        "C5T.OBSERVE.THE_RECORDS_CITATION_STAYS_HISTORICAL"
    );
    assert_ne!(
        moved
            .published_policy
            .as_ref()
            .map(|policy| policy.policy_revision),
        moved
            .latest
            .as_ref()
            .map(|record| record.policy.policy_revision),
        "C5T.OBSERVE.THE_TWO_CITATIONS_ARE_DISTINCT"
    );
    // An echoed revision that is no longer current is refused by name, without writing.
    assert_remote_code(
        client.classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "high".to_owned(),
            reason: "Echoing a Policy revision that has moved.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::Record {
                predecessor_digest: reclassified.digest.value.clone(),
            },
        }),
        LocalErrorCode::ClassificationStalePolicyRevision,
    );

    // Scope opacity, byte for byte: a Mission this Workspace never admitted answers exactly as an
    // unknown one does, and neither says whether it exists.
    let unknown = client.get_mission_classification(Uuid::new_v4());
    let foreign = client.get_mission_classification(Uuid::new_v4());
    let line = |result: &Result<beacond::ClassificationObservation, LocalTransportError>| {
        result.as_ref().err().map(std::string::ToString::to_string)
    };
    assert_eq!(
        line(&unknown),
        line(&foreign),
        "C5T.SCOPE.ABSENCE_AND_FOREIGN_ARE_BYTE_IDENTICAL"
    );
    assert_remote_code(unknown, LocalErrorCode::MissionClassificationUnavailable);

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
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

/// B8 — a long diagnostic label must not decide whether the socket can be bound.
///
/// The label here is 120 characters. Under the OLD root formula
/// (`beacon-transport-{label}-{uuid}`) that directory basename is 174 bytes — comfortably inside
/// `NAME_MAX` (255), so `create_dir` succeeds and the failure lands exactly where it matters: the
/// server's `bind`, with `Io(InvalidInput, "path must be shorter than SUN_LEN")`. Under the current
/// formula the basename is 35 bytes whatever the label says, so the server binds.
///
/// It binds a real `LocalSupervisorServer` rather than measuring a string, and drops it so the
/// socket is cleaned up.
#[test]
fn a_long_diagnostic_label_still_binds_the_local_socket() {
    let label = "a".repeat(120);
    let runtime = TestRuntime::new(&label);

    // BIND FIRST, deliberately. The failure this bar exists to catch belongs at the socket bind --
    // under the old formula `LocalSupervisorServer::bind` refuses with
    // `Io(InvalidInput, "path must be shorter than SUN_LEN")` -- and a length check placed ahead of
    // it would short-circuit that and pin arithmetic instead of the kernel's answer.
    let server = runtime.server();
    assert!(
        runtime.socket.exists(),
        "B8.SOCKET_BOUND: the server did not create {}",
        runtime.socket.display()
    );
    drop(server);

    // Recorded after the bind succeeded, as the budget this fixture keeps rather than as the thing
    // being tested.
    let socket_len = runtime.socket.as_os_str().len();
    assert!(
        socket_len <= 107,
        "B8.SOCKET_PATH_FITS_SUN_LEN: {socket_len} bytes: {}",
        runtime.socket.display()
    );
}
