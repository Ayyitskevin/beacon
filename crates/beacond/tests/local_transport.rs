#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use beacond::{
    AccessOrigin, AppendixRequestDto, AutonomyProfile, BeaconCapability, BudgetBoundary,
    ClassificationIntentRequest, CompletionPreview, Conflicts, DeclaredCheck, DiscoveryStatus,
    EvaluationDraft, EvaluationService, EvaluationSubjectDraft, EvaluationSupersedes, Evaluator,
    Finding, FindingSeverity, LiveDataEncryption, LocalErrorCode, LocalModelActivationState,
    LocalModelEndpoint, LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer,
    LocalTransportError, MAX_RECENT_RUN_WINDOW, MissionAdmissionActor,
    MissionCompletionCommandService, MissionDraft, MissionLifecycle, MissionMemberDraft,
    ModelActivation, OperatorAccountService, OperatorSessionPhase, OutcomeService,
    PlatformMonitorState, PolicyAuthor, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProviderFeature, RepositoryAuthor, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationState, ResidentLifecycle, RubricDeclaration, SecretString,
    StarterOrganizationDecision, StarterResidentDraft, StarterRole, SupersedesDto, TeamDraft,
    TeamFormationActor, TeamIneligibilityReason, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TrackRecordEvidenceKind,
    TrackRecordRequirement, Verdict, initialize_local_auth_token,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

struct TestRuntime {
    root: PathBuf,
    store: PathBuf,
    socket: PathBuf,
    token: PathBuf,
    lock: PathBuf,
    retain: bool,
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
            retain: false,
        }
    }

    /// Only the restart journey opts in. No fallible work separates construction from retention;
    /// every later assertion/unwind leaves the original directory and database in place.
    fn retained(label: &str) -> Self {
        let mut runtime = Self::new(label);
        runtime.retain = true;
        eprintln!("B14.JOURNEY.RETAINED_FIXTURE={}", runtime.root.display());
        runtime
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

// Selected-journey evidence only. Names are fixed at the call sites; no account/token/debug dump.
// create_new refuses an existing path (including a symlink); no stage overwrites another stage.
fn write_journey_evidence(root: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[derive(serde::Serialize)]
struct JourneyMetadataObservation {
    accepted_requests: usize,
    completed_responses: usize,
    routes: Vec<&'static str>,
    failure: Option<&'static str>,
    worker_elapsed_ms: u128,
}

/// A held fixture, not a provider. Unlike the older detached helper, this selected case counts
/// accepted connections and joins on success and unwind. Bounds are cooperative I/O limits, not
/// a physical-exit promise: scheduling, filesystem finalization and the native server remain outside.
struct JourneyMetadata {
    address: SocketAddr,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<JourneyMetadataObservation>>,
    root: PathBuf,
}

impl JourneyMetadata {
    fn start(root: &std::path::Path) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .expect("B14.JOURNEY.METADATA_LISTENER_BINDS");
        listener
            .set_nonblocking(true)
            .expect("B14.JOURNEY.METADATA_ACCEPT_IS_NONBLOCKING");
        let address = listener.local_addr().expect("B14.JOURNEY.METADATA_ADDRESS");
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let thread = thread::spawn(move || {
            let started = Instant::now();
            let deadline = started + Duration::from_secs(30);
            let mut observation = JourneyMetadataObservation {
                accepted_requests: 0,
                completed_responses: 0,
                routes: Vec::new(),
                failure: None,
                worker_elapsed_ms: 0,
            };
            let responses = [
                ("GET /api/tags HTTP/1.1\r\n", tags_response()),
                (
                    "POST /api/show HTTP/1.1\r\n",
                    json!({ "capabilities": ["completion", "tools", "future_audio"] }).to_string(),
                ),
                ("GET /api/tags HTTP/1.1\r\n", tags_response()),
            ];
            // Initial inspection and activation's reinspection: exactly three requests each.
            let result = (|| -> Result<(), &'static str> {
                for (expected, body) in responses.iter().cycle().take(6) {
                    let mut stream = loop {
                        if !worker_running.load(Ordering::Acquire) {
                            return Err("stopped_before_six_responses");
                        }
                        if Instant::now() >= deadline {
                            return Err("metadata_window_elapsed");
                        }
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                thread::sleep(Duration::from_millis(5));
                            }
                            Err(_) => return Err("metadata_accept_failed"),
                        }
                    };
                    observation.accepted_requests += 1;
                    let request_deadline = deadline.min(Instant::now() + Duration::from_secs(2));
                    let request = Self::read_request(&mut stream, request_deadline)?;
                    if !request.starts_with(expected) {
                        return Err("unexpected_metadata_route");
                    }
                    // Persist only the allowlisted literal, never the supplied request/body.
                    observation.routes.push((*expected).trim_end());
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let mut remaining = response.as_bytes();
                    while !remaining.is_empty() {
                        let time_left = request_deadline
                            .checked_duration_since(Instant::now())
                            .filter(|remaining| !remaining.is_zero())
                            .ok_or("metadata_request_deadline")?;
                        stream
                            .set_write_timeout(Some(time_left))
                            .map_err(|_| "metadata_timeout_setup")?;
                        let count = stream
                            .write(remaining)
                            .map_err(|_| "metadata_write_failed")?;
                        if count == 0 {
                            return Err("metadata_write_closed");
                        }
                        remaining = &remaining[count..];
                    }
                    observation.completed_responses += 1;
                }
                Ok(())
            })();
            observation.failure = result.err();
            observation.worker_elapsed_ms = started.elapsed().as_millis();
            observation
        });
        Self {
            address,
            running,
            thread: Some(thread),
            root: root.to_owned(),
        }
    }

    fn read_request(
        stream: &mut std::net::TcpStream,
        deadline: Instant,
    ) -> Result<String, &'static str> {
        const LIMIT: usize = 8192;
        let mut bytes = Vec::new();
        loop {
            let time_left = deadline
                .checked_duration_since(Instant::now())
                .filter(|remaining| !remaining.is_zero())
                .ok_or("metadata_request_deadline")?;
            stream
                .set_read_timeout(Some(time_left))
                .map_err(|_| "metadata_timeout_setup")?;
            let mut chunk = [0_u8; 1024];
            let count = stream
                .read(&mut chunk)
                .map_err(|_| "metadata_read_failed")?;
            if count == 0 {
                return Err("metadata_request_closed");
            }
            if bytes.len() + count > LIMIT {
                return Err("metadata_request_too_large");
            }
            bytes.extend_from_slice(&chunk[..count]);
            let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let header =
                std::str::from_utf8(&bytes[..end]).map_err(|_| "metadata_header_invalid")?;
            let mut length = None;
            for line in header.lines().skip(1) {
                let (name, value) = line.split_once(':').ok_or("metadata_header_invalid")?;
                if name.eq_ignore_ascii_case("transfer-encoding") {
                    return Err("metadata_transfer_encoding");
                }
                if name.eq_ignore_ascii_case("content-length") {
                    if length.is_some() {
                        return Err("metadata_duplicate_length");
                    }
                    length = Some(
                        value
                            .trim()
                            .parse::<usize>()
                            .map_err(|_| "metadata_length_invalid")?,
                    );
                }
            }
            let total = (end + 4)
                .checked_add(length.unwrap_or(0))
                .filter(|total| *total <= LIMIT)
                .ok_or("metadata_request_too_large")?;
            if bytes.len() > total {
                return Err("metadata_trailing_bytes");
            }
            if bytes.len() == total {
                return String::from_utf8(bytes).map_err(|_| "metadata_request_invalid");
            }
        }
    }

    fn stop(&mut self) -> Result<JourneyMetadataObservation, &'static str> {
        self.running.store(false, Ordering::Release);
        let joined = self.thread.take().ok_or("metadata_already_joined")?.join();
        let evidence = match &joined {
            Ok(observation) => json!({
                "synthetic": true, "joined": true, "observation": observation,
                "limits": { "requests": 6, "request_bytes": 8192, "request_seconds": 2, "worker_seconds": 30 },
                "scope": "accepted fixture connections only; not all process/host network attempts"
            }),
            Err(_) => {
                json!({ "synthetic": true, "joined": true, "failure": "worker_panicked", "counts": "UNKNOWN" })
            }
        };
        write_journey_evidence(&self.root, "metadata.json", evidence.to_string().as_bytes())
            .map_err(|_| "metadata_evidence_write_failed")?;
        joined.map_err(|_| "metadata_worker_panicked")
    }
}

impl Drop for JourneyMetadata {
    fn drop(&mut self) {
        if self.thread.is_some() && self.stop().is_err() {
            // No raw worker panic payload; do not mask the original journey failure.
            eprintln!("B14.JOURNEY.METADATA_JOIN_OR_EVIDENCE_FAILED");
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
        if self.retain {
            let note = json!({
                "synthetic": true,
                "fixture_retained": true,
                "unwinding": thread::panicking(),
                "scope": "fixture Drop only; not process exit or host-wide effects"
            });
            if write_journey_evidence(&self.root, "fixture-drop.json", note.to_string().as_bytes())
                .is_err()
            {
                eprintln!("B14.JOURNEY.RETENTION_NOTE_WRITE_FAILED");
            }
            return;
        }
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

    // B15 F1: a completion may not be recorded before its Mission is classified. One published
    // Policy to cite, then the Operator's own ordinary judgment.
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
        .unwrap_or_else(|error| panic!("C4.WIDE.A_POLICY_IS_PUBLISHED: {error}"));
    client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id: admitted.id,
            impact: "ordinary".to_owned(),
            reason: "One reviewed change, fully reversible.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| panic!("C4.WIDE.THE_MISSION_IS_CLASSIFIED: {error}"));

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

/// The whole prepared Mission, end to end on a real store and socket, with every receipt field
/// compared against separately retained owner records.
///
/// The fixture mints everything through the public services, in the order the owner requires: a
/// published Policy, then the classification, then the completion — a classification AFTER the
/// completion cannot be prepared, which is the sequencing the refusal bars below prove from the other
/// side.
#[test]
fn a_prepared_candidate_receipt_equals_the_owners_own_records() {
    let runtime = TestRuntime::new("c6live");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let prepared_mission = prepare_c6_fixture(&client);

    // Policy default on a HIGH-impact Mission: the request is `policy_default` and the resolved mode
    // is `included`, which is exactly the pair that must not be conflated.
    let receipt = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C6T.PREPARE.THE_OWNER_ACCEPTS_THE_REQUEST: {error}"));

    // The owner's own records, read independently of the transport on a second authenticated service.
    let accounts = Arc::new(OperatorAccountService::open(&runtime.store).unwrap());
    accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    let outcomes = OutcomeService::new(Arc::clone(&accounts), prepared_mission.workspace_id);
    let stored = outcomes
        .manifest(&receipt.manifest_digest.value)
        .expect("C6T.PREPARE.THE_MANIFEST_IS_READABLE");
    let record = outcomes
        .candidate(&receipt.manifest_digest.value)
        .expect("C6T.PREPARE.THE_CANDIDATE_IS_READABLE");

    assert_eq!(
        receipt.mission_id, prepared_mission.mission_id,
        "C6T.RECEIPT.MISSION_ID"
    );
    assert_eq!(
        receipt.manifest_id,
        stored.manifest_id(),
        "C6T.RECEIPT.MANIFEST_ID_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            receipt.manifest_digest.algorithm.as_str(),
            receipt.manifest_digest.canonicalizer.as_str(),
            receipt.manifest_digest.value.as_str(),
        ),
        (
            stored.digest().algorithm.as_str(),
            stored.digest().canonicalizer.as_str(),
            stored.digest().value.as_str(),
        ),
        "C6T.RECEIPT.MANIFEST_DIGEST_ENVELOPE_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            receipt.candidate_artifact_digest.algorithm.as_str(),
            receipt.candidate_artifact_digest.canonicalizer.as_str(),
            receipt.candidate_artifact_digest.value.as_str(),
        ),
        (
            record.digest().algorithm.as_str(),
            record.digest().canonicalizer.as_str(),
            record.digest().value.as_str(),
        ),
        "C6T.RECEIPT.ARTIFACT_RECORD_DIGEST_IS_THE_OWNERS"
    );
    let payload = record.payload();
    assert_eq!(
        (
            receipt.candidate_content_digest.algorithm.as_str(),
            receipt.candidate_content_digest.canonicalizer.as_str(),
            receipt.candidate_content_digest.value.as_str(),
        ),
        (
            payload.content_digest.algorithm.as_str(),
            payload.content_digest.canonicalizer.as_str(),
            payload.content_digest.value.as_str(),
        ),
        "C6T.RECEIPT.CONTENT_DIGEST_IS_THE_OWNERS"
    );
    // The two digests are DIFFERENT things under DIFFERENT canonicalizers. A caller that verified the
    // wrong one would be checking the record against content bytes, or the reverse.
    assert_ne!(
        receipt.candidate_artifact_digest.value, receipt.candidate_content_digest.value,
        "C6T.RECEIPT.THE_RECORD_AND_CONTENT_DIGESTS_ARE_NOT_THE_SAME_VALUE"
    );
    assert_eq!(
        receipt.candidate_artifact_digest.canonicalizer, "beacon.artifact.v1",
        "C6T.RECEIPT.THE_RECORD_DIGEST_CANONICALIZER"
    );
    assert_eq!(
        receipt.candidate_content_digest.canonicalizer, "beacon.raw.v1",
        "C6T.RECEIPT.THE_CONTENT_DIGEST_CANONICALIZER"
    );
    assert_eq!(
        receipt.manifest_digest.canonicalizer, "beacon.outcome.manifest.v2",
        "C6T.RECEIPT.THE_MANIFEST_DIGEST_CANONICALIZER"
    );
    assert_eq!(
        receipt.candidate_content_bytes, payload.content_bytes,
        "C6T.RECEIPT.CONTENT_BYTES_ARE_THE_OWNERS"
    );
    assert_eq!(
        receipt.media_type, payload.media_type,
        "C6T.RECEIPT.MEDIA_TYPE_IS_THE_OWNERS"
    );
    assert_eq!(
        receipt.media_type, "text/markdown; charset=utf-8",
        "C6T.RECEIPT.MEDIA_TYPE_IS_MARKDOWN"
    );
    assert!(
        receipt.candidate_content_bytes > 0 && receipt.candidate_content_bytes <= 1_048_576,
        "C6T.RECEIPT.CONTENT_BYTES_ARE_INSIDE_THE_OWNING_BOUND: {}",
        receipt.candidate_content_bytes
    );
    assert_eq!(
        serde_json::to_value(receipt.appendix_request).unwrap(),
        serde_json::Value::from("policy_default"),
        "C6T.RECEIPT.THE_APPENDIX_REQUEST_IS_WHAT_THE_MANIFEST_HOLDS"
    );
    assert_eq!(
        serde_json::to_value(receipt.appendix_mode).unwrap(),
        serde_json::Value::from("included"),
        "C6T.RECEIPT.THE_MODE_IS_THE_MANIFESTS_OWN_RESOLUTION"
    );

    // The whole receipt, as a daemon response envelope, stays inside the existing 64 KiB domain.
    let envelope_bytes = serde_json::to_vec(&serde_json::json!({
        "protocol": 1,
        "result": { "kind": "prepared_outcome_candidate", "candidate": receipt },
        "error": null,
    }))
    .unwrap()
    .len();
    assert!(
        envelope_bytes < 65_536,
        "C6T.RECEIPT.THE_RESPONSE_STAYS_IN_THE_OLD_DOMAIN: {envelope_bytes} bytes"
    );

    // An explicit identical repeat returns the SAME identity and writes nothing: the Artifact's
    // creation Event sequence is where a second write would show.
    let first_event_seq = record.creation_event_seq();
    let repeat = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C6T.REPEAT.AN_IDENTICAL_REQUEST_IS_ACCEPTED: {error}"));
    assert_eq!(
        repeat, receipt,
        "C6T.REPEAT.THE_SAME_REQUEST_RETURNS_THE_SAME_RECEIPT"
    );
    let after = outcomes
        .candidate(&receipt.manifest_digest.value)
        .expect("the candidate is still readable");
    assert_eq!(
        after.creation_event_seq(),
        first_event_seq,
        "C6T.REPEAT.NO_SECOND_WRITE_AND_NO_SECOND_EVENT"
    );

    // An explicit `include` on the SAME Mission is a DIFFERENT manifest identity, even though this
    // Mission is high-impact and both requests resolve to `included`: the request is inside the bytes.
    let included = client
        .prepare_outcome_candidate(prepared_mission.mission_id, AppendixRequestDto::Include)
        .unwrap_or_else(|error| panic!("C6T.APPENDIX.AN_EXPLICIT_INCLUDE_IS_ACCEPTED: {error}"));
    assert_eq!(
        serde_json::to_value(included.appendix_request).unwrap(),
        serde_json::Value::from("include"),
        "C6T.APPENDIX.THE_EXPLICIT_REQUEST_IS_REPORTED"
    );
    assert_eq!(
        serde_json::to_value(included.appendix_mode).unwrap(),
        serde_json::Value::from("included"),
        "C6T.APPENDIX.BOTH_RESOLVE_TO_INCLUDED_HERE"
    );
    assert_ne!(
        included.manifest_digest.value, receipt.manifest_digest.value,
        "C6T.APPENDIX.THE_TWO_REQUESTS_ARE_DISTINCT_IDENTITIES"
    );
    assert_ne!(
        included.manifest_id, receipt.manifest_id,
        "C6T.APPENDIX.THE_TWO_REQUESTS_ARE_DISTINCT_IDENTITIES"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The three row counts this bar's no-write claim is actually ABOUT, read from the isolated store.
///
/// Read-only, and read from the STORE rather than from the receipt: a count derived from the reply
/// would be the reply agreeing with itself. One Deferred read transaction, so the three counts are
/// one consistent picture rather than three moments; and the connection is opened AND dropped inside
/// this call, so no transaction of this test's is ever live across the command under test — holding
/// one there would turn a measurement into interference.
///
/// `busy_timeout` follows the existing read pattern in this file (see the repository-revision read
/// below); no sleep and no retry loop is invented here.
fn domain_row_counts(store: &std::path::Path) -> (i64, i64, i64) {
    let mut connection = rusqlite::Connection::open(store).expect("the isolated store opens");
    connection
        .busy_timeout(Duration::from_secs(5))
        .expect("the read waits rather than failing fast");
    let read = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
        .expect("one read transaction over the three tables");
    let count = |table: &str| -> i64 {
        read.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap_or_else(|error| panic!("counting {table}: {error}"))
    };
    let counts = (
        count("outcome_manifests"),
        count("artifacts"),
        count("events"),
    );
    read.commit().expect("a read transaction commits");
    counts
}

/// Re-preparing an ALREADY PUBLISHED candidate: the same identities come back, nothing is written
/// again, and the publication that exists is untouched.
///
/// The published initial state is established through PUBLIC owner commands only —
/// `EvaluationService::record` with an explicit test draft by an independent Resident, then
/// `OutcomeService::publish` — which is the same shape the existing `outcome_closure` suite uses. No
/// publication relation is planted, no Run or Receipt is invented, and nothing here claims a real
/// evaluator or a model did any work: it is fixture data for the one scenario this bar is about.
#[test]
fn re_preparing_a_published_candidate_changes_nothing_that_is_published() {
    let runtime = TestRuntime::new("c6repub");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let fixture = prepare_c6_fixture_with_reviewer(&client);

    // 1. Prepare over the REAL socket.
    let first = client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .unwrap_or_else(|error| panic!("C6T.PUBLISHED.THE_FIRST_PREPARE_IS_ACCEPTED: {error}"));

    // 2. Publish it, through public owner commands on a second authenticated service.
    let accounts = Arc::new(OperatorAccountService::open(&runtime.store).unwrap());
    accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    let outcomes = OutcomeService::new(Arc::clone(&accounts), fixture.workspace_id);
    let record = outcomes
        .candidate(&first.manifest_digest.value)
        .expect("C6T.PUBLISHED.THE_CANDIDATE_IS_READABLE");
    let evaluation = EvaluationService::new(Arc::clone(&accounts), fixture.workspace_id)
        .record(EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(
                first.candidate_artifact_digest.value.clone(),
            ),
            evaluator: Evaluator {
                // An INDEPENDENT Resident: not the one accountable for the completion.
                resident_id: fixture.reviewer_resident_id,
                configuration_id: fixture.reviewer_configuration_id,
            },
            rubric: RubricDeclaration {
                id: "outcome-document-v1".to_owned(),
                version: 1,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: data_encoding::HEXLOWER.encode(&[0x20_u8; 32]),
            },
            cited_runs: Vec::new(),
            verdict: Verdict::Meets,
            findings: vec![Finding {
                severity: FindingSeverity::Note,
                summary: "Read the exact Artifact bytes.".to_owned(),
            }],
            conflicts: Conflicts::None,
            rationale: "An independent Resident read the exact Artifact bytes.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        })
        .expect("C6T.PUBLISHED.THE_EVALUATION_IS_RECORDED");
    let published = outcomes
        .publish(fixture.mission_id, record.digest(), evaluation.digest())
        .expect("C6T.PUBLISHED.THE_CANDIDATE_IS_PUBLISHED");
    let published_version = published.publication_version();
    let published_count = published.publication_version_count();
    let exported = outcomes
        .export_markdown(fixture.mission_id)
        .expect("C6T.PUBLISHED.THE_EXPORT_IS_READABLE");
    let published_bytes = exported.bytes().to_vec();
    let creation_event_seq = record.creation_event_seq();
    let summaries = client.mission_publications(fixture.mission_id).unwrap();
    assert_eq!(
        summaries.len(),
        1,
        "C6T.PUBLISHED.THE_FIXTURE_HAS_EXACTLY_ONE_PUBLICATION"
    );

    // 3. Re-prepare the SAME request over the socket. Owner idempotence is the only repeat mechanism.
    //    The store is counted immediately before and immediately after, and nothing else runs
    //    between the two snapshots and the command.
    let before_repeat = domain_row_counts(&runtime.store);
    let repeat = client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .unwrap_or_else(|error| panic!("C6T.PUBLISHED.THE_REPEAT_IS_ACCEPTED: {error}"));
    let after_repeat = domain_row_counts(&runtime.store);

    assert_eq!(
        repeat, first,
        "C6T.PUBLISHED.THE_REPEAT_RETURNS_THE_SAME_RECEIPT"
    );
    assert_eq!(
        (
            repeat.manifest_id,
            repeat.manifest_digest.value.clone(),
            repeat.candidate_artifact_digest.value.clone(),
            repeat.candidate_content_digest.value.clone(),
            repeat.candidate_content_bytes,
        ),
        (
            first.manifest_id,
            first.manifest_digest.value.clone(),
            first.candidate_artifact_digest.value.clone(),
            first.candidate_content_digest.value.clone(),
            first.candidate_content_bytes,
        ),
        "C6T.PUBLISHED.EVERY_HELD_IDENTITY_IS_UNCHANGED"
    );

    // 4. Nothing was written again -- MEASURED, on the three tables the claim names. An unmoved
    //    creation Event says the re-read returned the original Artifact; it cannot fail on a second
    //    manifest row, a second Artifact row, or a new Event anywhere else, so it kept a promise it
    //    was not making (non-author review, DC6T). Both facts are now asserted, each under the name
    //    that is true of it.
    assert_eq!(
        after_repeat, before_repeat,
        "C6T.PUBLISHED.NO_SECOND_MANIFEST_ARTIFACT_OR_EVENT_WRITE"
    );
    let after = outcomes
        .candidate(&first.manifest_digest.value)
        .expect("the candidate is still readable");
    assert_eq!(
        after.creation_event_seq(),
        creation_event_seq,
        "C6T.PUBLISHED.THE_ORIGINAL_ARTIFACT_CREATION_EVENT_IS_UNMOVED"
    );

    // 5. And the publication is exactly as it was: same version, same count, same bytes.
    let still = outcomes
        .get(fixture.mission_id)
        .expect("C6T.PUBLISHED.THE_PUBLICATION_IS_STILL_READABLE");
    assert_eq!(
        (
            still.publication_version(),
            still.publication_version_count()
        ),
        (published_version, published_count),
        "C6T.PUBLISHED.THE_PUBLICATION_VERSION_IS_UNCHANGED"
    );
    assert_eq!(
        outcomes
            .export_markdown(fixture.mission_id)
            .expect("the export is still readable")
            .bytes(),
        published_bytes.as_slice(),
        "C6T.PUBLISHED.THE_PUBLISHED_BYTES_ARE_UNCHANGED"
    );
    assert_eq!(
        client.mission_publications(fixture.mission_id).unwrap(),
        summaries,
        "C6T.PUBLISHED.THE_PUBLICATION_HISTORY_IS_UNCHANGED"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The C6 fixture plus a SECOND, independent Resident, so an Evaluation can name an evaluator that is
/// not the Resident accountable for the completion.
///
/// Public constructors only: `accept_starter_organization` with two drafts, `activate_resident`,
/// `inspect_local_model`/`activate_local_model`, `configure_resident` — the same client methods the
/// single-Resident fixture uses, called once more.
fn prepare_c6_fixture_with_reviewer(client: &LocalSupervisorClient) -> PreparedC6Fixture {
    client
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    client.complete_operator_setup(true).unwrap();
    let residents = client
        .accept_starter_organization(
            1,
            vec![
                draft(StarterRole::Lead, Some("Northstar")),
                draft(StarterRole::Reviewer, Some("Second Reader")),
            ],
        )
        .unwrap()
        .residents;
    let lead = residents[0].id;
    let reviewer = residents[1].id;
    let review = client
        .inspect_local_model(MODEL_NAME.to_owned(), MODEL_DIGEST.to_owned())
        .unwrap();
    let activation = client
        .activate_local_model(review, vec![ProviderFeature::Completion])
        .unwrap();
    let configure = |resident_id: Uuid| {
        client.activate_resident(resident_id, None).unwrap();
        client
            .configure_resident(
                resident_id,
                None,
                activation.id,
                vec![BeaconCapability::WorkCreateMission],
            )
            .unwrap()
            .id
    };
    let lead_configuration = configure(lead);
    let reviewer_configuration = configure(reviewer);

    let project = client
        .create_project(project_draft("Beacon Alpha", AutonomyProfile::Supervised))
        .unwrap();
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
    let formed = client
        .form_team(TeamDraft {
            project_id: project.id,
            name: "Alpha Team".to_owned(),
            // Both Residents: a Mission's members must be the Team's, and the evaluator has to be on
            // the Mission's admission roster for its configuration to be citable.
            members: vec![
                TeamMemberDraft {
                    resident_id: lead,
                    expected_configuration_id: lead_configuration,
                },
                TeamMemberDraft {
                    resident_id: reviewer,
                    expected_configuration_id: reviewer_configuration,
                },
            ],
        })
        .unwrap();
    let fixture = PreparedC6Fixture {
        workspace_id: project.workspace_id,
        project_id: project.id,
        charter_revision: project.charter.revision,
        policy_revision: policy.revision,
        team_id: formed.id,
        resident_id: lead,
        configuration_id: lead_configuration,
        reviewer_resident_id: reviewer,
        reviewer_configuration_id: reviewer_configuration,
        mission_id: Uuid::nil(),
    };
    let mission_id = admit_another_mission(client, &fixture, "Ship one reviewed feature");
    client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id,
            impact: "high".to_owned(),
            reason: "Touches the operator credential path.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_CLASSIFICATION_IS_RECORDED: {error}"));
    complete_mission(client, &fixture, mission_id);
    PreparedC6Fixture {
        mission_id,
        ..fixture
    }
}

/// Preparing, and re-preparing, leaves the publication relation exactly as it was.
///
/// The reachable state here is "no publication", and that is what the bar proves: preparing a
/// candidate twice adds nothing to the Mission's publication history. Publishing itself needs a real
/// independent Evaluation, which is a later prerequisite outside this lane, so the already-PUBLISHED
/// variant of this bar is reported NOT PROBED rather than faked with a planted relation.
#[test]
fn preparing_a_candidate_changes_no_publication_history() {
    let runtime = TestRuntime::new("c6pub");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let fixture = prepare_c6_fixture(&client);

    let before = client.mission_publications(fixture.mission_id).unwrap();
    assert!(
        before.is_empty(),
        "C6T.PUBLICATION.THE_FIXTURE_HAS_NO_PUBLICATION"
    );

    client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .unwrap_or_else(|error| panic!("C6T.PUBLICATION.THE_FIRST_PREPARE_IS_ACCEPTED: {error}"));
    client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .unwrap_or_else(|error| panic!("C6T.PUBLICATION.THE_REPEAT_IS_ACCEPTED: {error}"));
    client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::Include)
        .unwrap_or_else(|error| panic!("C6T.PUBLICATION.THE_OTHER_APPENDIX_IS_ACCEPTED: {error}"));

    assert_eq!(
        client.mission_publications(fixture.mission_id).unwrap(),
        before,
        "C6T.PUBLICATION.PREPARING_PUBLISHES_NOTHING"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The three sequencing refusals, each by its own name, and the opacity of a Mission this Workspace
/// never admitted.
#[test]
fn the_preparation_sequencing_refusals_are_separately_named() {
    let runtime = TestRuntime::new("c6seq");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let base = prepare_c6_fixture(&client);

    // 1. A Mission with no completion at all.
    let bare = admit_another_mission(&client, &base, "Ship a second reviewed feature");
    assert_remote_code(
        client.prepare_outcome_candidate(bare, AppendixRequestDto::PolicyDefault),
        LocalErrorCode::OutcomeNotCompleted,
    );

    // 2. Completed, never classified.
    let unclassified = admit_another_mission(&client, &base, "Ship a third reviewed feature");
    plant_legacy_completion(&runtime.store, &client, unclassified);
    assert_remote_code(
        client.prepare_outcome_candidate(unclassified, AppendixRequestDto::PolicyDefault),
        LocalErrorCode::OutcomeNotClassified,
    );

    // 3. Classified only AFTER the completion: the boundary cannot be rewritten, and another
    // classification will not change it.
    let late = admit_another_mission(&client, &base, "Ship a fourth reviewed feature");
    plant_legacy_completion(&runtime.store, &client, late);
    client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id: late,
            impact: "high".to_owned(),
            reason: "Classified after the completion was recorded.".to_owned(),
            expected_policy_revision: Some(base.policy_revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| {
            panic!("C6T.SEQUENCE.THE_LATE_CLASSIFICATION_IS_RECORDED: {error}")
        });
    assert_remote_code(
        client.prepare_outcome_candidate(late, AppendixRequestDto::PolicyDefault),
        LocalErrorCode::OutcomeNoClassificationAtCompletion,
    );

    // 4. A Mission this Workspace never admitted is opaque: it answers exactly as an absent one, and
    // neither says whether it exists anywhere.
    let unknown =
        client.prepare_outcome_candidate(Uuid::new_v4(), AppendixRequestDto::PolicyDefault);
    let foreign = client.prepare_outcome_candidate(Uuid::new_v4(), AppendixRequestDto::Include);
    let line = |result: &Result<beacond::PreparedOutcomeCandidateDto, LocalTransportError>| {
        result.as_ref().err().map(std::string::ToString::to_string)
    };
    assert_eq!(
        line(&unknown),
        line(&foreign),
        "C6T.SCOPE.ABSENCE_AND_FOREIGN_ARE_BYTE_IDENTICAL"
    );
    assert_remote_code(unknown, LocalErrorCode::OutcomeNotCompleted);

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// What one C6 fixture holds: the Workspace and Project it minted, the Policy revision in force, the
/// Resident and configuration every later Mission reuses, and one Mission already classified and
/// completed in the order the owner requires.
struct PreparedC6Fixture {
    workspace_id: Uuid,
    project_id: Uuid,
    charter_revision: u32,
    policy_revision: u32,
    team_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    /// A SECOND Resident, for a bar that needs an evaluator who is not the accountable Resident. The
    /// single-Resident fixture fills these with the lead's own ids and no bar of its own uses them.
    reviewer_resident_id: Uuid,
    reviewer_configuration_id: Uuid,
    mission_id: Uuid,
}

/// Everything through the PUBLIC services, in the owner's required order: Policy, then classification,
/// then completion. No SQL planting anywhere in this path.
fn prepare_c6_fixture(client: &LocalSupervisorClient) -> PreparedC6Fixture {
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
    let fixture = PreparedC6Fixture {
        workspace_id: project.workspace_id,
        project_id: project.id,
        charter_revision: project.charter.revision,
        policy_revision: policy.revision,
        team_id: formed.id,
        resident_id,
        configuration_id: configuration.id,
        reviewer_resident_id: resident_id,
        reviewer_configuration_id: configuration.id,
        mission_id: Uuid::nil(),
    };
    let mission_id = admit_another_mission(client, &fixture, "Ship one reviewed feature");

    // HIGH impact, so `policy_default` already resolves to `included` -- which is what makes the
    // distinct-identity bar above meaningful.
    client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id,
            impact: "high".to_owned(),
            reason: "Touches the operator credential path.".to_owned(),
            expected_policy_revision: Some(policy.revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_CLASSIFICATION_IS_RECORDED: {error}"));
    complete_mission(client, &fixture, mission_id);

    PreparedC6Fixture {
        mission_id,
        ..fixture
    }
}

/// A completion written the way a PRE-GUARD store already holds one: through the shipped byte
/// writer, with no classification. B15 F1 makes this state unreachable through the command, and the
/// prepare-sequencing refusals still have to be reachable to be proved.
fn plant_legacy_completion(
    store: &std::path::Path,
    client: &LocalSupervisorClient,
    mission_id: Uuid,
) {
    let mission = client
        .mission(mission_id)
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_MISSION_READS: {error}"));
    let member = &mission.members[0];
    let mut connection = rusqlite::Connection::open(store).unwrap();
    let boundary: u64 = connection
        .query_row(
            "SELECT min(sequence) FROM events WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?1",
            [mission_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).unwrap())
        .unwrap();
    let record = beacond::MissionCompletionV1 {
        workspace_id: mission.workspace_id,
        project_id: mission.project_id,
        mission_id,
        completion_id: Uuid::new_v4(),
        actor: beacond::CompletionActor::Operator,
        accountable_resident_id: member.resident_id,
        accountable_resident_configuration_id: member.admission_configuration_id,
        boundary_event_seq: boundary,
        disposition: beacond::CompletionDisposition::Concluded,
        declared_conditions_sha256: beacond::declared_conditions_digest(
            &mission.success_conditions,
        ),
        condition_results: mission
            .success_conditions
            .iter()
            .enumerate()
            .map(|(ordinal, _)| beacond::ConditionResultRecord {
                ordinal: u32::try_from(ordinal).unwrap(),
                outcome: beacond::ConditionOutcome::Met,
                support: Vec::new(),
                note: None,
            })
            .collect(),
        delivered_artifacts: Vec::new(),
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        spend: beacond::SpendReference::NotMeasured("no meter on this Mission".to_owned()),
        supersedes: beacond::CompletionSupersedes::None,
        summary: "A completion written before the order guard existed.".to_owned(),
        completed_at_ms: 9_000,
    };
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    beacond::record_completion(&mut connection, &record)
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_LEGACY_COMPLETION_IS_PLANTED: {error:?}"));
}

fn admit_another_mission(
    client: &LocalSupervisorClient,
    fixture: &PreparedC6Fixture,
    outcome: &str,
) -> Uuid {
    client
        .admit_mission(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: fixture.charter_revision,
            team_id: fixture.team_id,
            desired_outcome: outcome.to_owned(),
            success_conditions: vec!["Relevant gates pass".to_owned()],
            constraints: vec![],
            prohibitions: vec![],
            members: if fixture.reviewer_resident_id == fixture.resident_id {
                vec![MissionMemberDraft {
                    resident_id: fixture.resident_id,
                    expected_configuration_id: fixture.configuration_id,
                }]
            } else {
                // The evaluator's configuration is keyed at THIS Mission's admission -- the store's
                // own foreign key says so -- so an independent evaluator has to be admitted with it.
                vec![
                    MissionMemberDraft {
                        resident_id: fixture.resident_id,
                        expected_configuration_id: fixture.configuration_id,
                    },
                    MissionMemberDraft {
                        resident_id: fixture.reviewer_resident_id,
                        expected_configuration_id: fixture.reviewer_configuration_id,
                    },
                ]
            },
        })
        .unwrap()
        .id
}

/// Classify, then preview, then record with the token the preview returned.
///
/// B15 F1 added the classification step: a completion may not be recorded before its Mission is
/// classified, so the ordinary path now begins one step earlier. Bars that need a
/// completed-but-unclassified Mission -- a state only a pre-guard store can hold -- use
/// `plant_legacy_completion` instead.
fn complete_mission(client: &LocalSupervisorClient, fixture: &PreparedC6Fixture, mission_id: Uuid) {
    // Only if this Mission has none: several fixtures classify deliberately (a HIGH impact, say)
    // before completing, and a second classification would be `AlreadyClassified`.
    let observed = client
        .get_mission_classification(mission_id)
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_CLASSIFICATION_READS: {error}"));
    if observed.latest.is_none() {
        client
            .classify_mission_impact(ClassificationIntentRequest {
                mission_id,
                impact: "ordinary".to_owned(),
                reason: "One reviewed change, fully reversible.".to_owned(),
                expected_policy_revision: Some(fixture.policy_revision),
                supersedes: SupersedesDto::None,
            })
            .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_MISSION_IS_CLASSIFIED: {error}"));
    }
    let request = beacond::CompletionIntentRequest {
        mission_id,
        accountable_resident_id: fixture.resident_id,
        disposition: "concluded".to_owned(),
        condition_results: vec![beacond::ConditionResultDto {
            ordinal: 0,
            outcome: "met".to_owned(),
            support: Vec::new(),
            note: None,
        }],
        delivered_artifacts: Vec::new(),
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        spend: beacond::SpendDto::NotMeasured("no meter on this Mission".to_owned()),
        summary: "Shipped it.".to_owned(),
    };
    let preview = client
        .preview_mission_completion(request.clone())
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_COMPLETION_PREVIEWS: {error}"));
    client
        .record_mission_completion(preview.request.clone(), preview.intent_sha256)
        .unwrap_or_else(|error| panic!("C6T.FIXTURE.THE_COMPLETION_IS_RECORDED: {error}"));
}

/// C6's preparation command is KNOWN to the wire, for BOTH appendix choices, and answers with the
/// account's own refusal.
///
/// The smallest real socket bar for this slice and the one that must be RED before any wiring exists:
/// today `prepare_outcome_candidate` does not parse, so the supervisor answers `protocol_error` from
/// its own `Json` refusal. "No such command" and "that command refused you" are different answers,
/// and making that difference is exactly what the connection does.
///
/// Raw bytes rather than a client method, for the reason the C5 bar used them: at the moment this is
/// written there is no client method to call, and a test that cannot compile cannot be RED by a named
/// behaviour. Both closed appendix strings are exercised at the wire seam, because a route that
/// accepts one spelling and not the other is half a route.
#[test]
fn the_preparation_command_is_known_to_the_wire_and_authenticated() {
    let runtime = TestRuntime::new("c6route");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    // The token is read from its file and never printed: no assertion message below carries it.
    let token = fs::read_to_string(&runtime.token).unwrap();
    let mission_id = Uuid::new_v4();

    for appendix_request in ["policy_default", "include"] {
        let body = serde_json::to_vec(&json!({
            "protocol": 1,
            "token": token,
            "command": {
                "name": "prepare_outcome_candidate",
                "mission_id": mission_id,
                "appendix_request": appendix_request,
            },
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
            "C6T.ROUTE.IS_A_KNOWN_COMMAND: {appendix_request}"
        );
        assert_eq!(
            answered.get("result"),
            Some(&serde_json::Value::Null),
            "C6T.ROUTE.A_REFUSAL_CARRIES_NO_RESULT: {appendix_request}"
        );
    }

    // An appendix value outside the closed pair is refused as a malformed envelope, not silently
    // defaulted: there is no `omit`, no `default` and no third arm on this wire.
    let widened = serde_json::to_vec(&json!({
        "protocol": 1,
        "token": token,
        "command": {
            "name": "prepare_outcome_candidate",
            "mission_id": mission_id,
            "appendix_request": "omit",
        },
    }))
    .unwrap();
    let mut stream = UnixStream::connect(&runtime.socket).unwrap();
    stream.write_all(&widened).unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).unwrap();
    let answered: serde_json::Value = serde_json::from_slice(&answer).unwrap();
    assert_eq!(
        answered
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str),
        Some("protocol_error"),
        "C6T.ROUTE.THE_APPENDIX_CHOICE_IS_A_CLOSED_PAIR"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The document that crosses the socket IS the owner's stored bytes, and reading changes nothing.
///
/// The premise is established independently: a second authenticated `OutcomeService` on the same
/// store reads the same manifest and candidate, and every one of the eight fields is compared against
/// THAT, never against the reply's own other fields. The bytes are compared after a base64 decode --
/// byte for byte, not by length or by digest, because a digest comparison would pass on two encodings
/// of the same content and the point is that the encoding round trip is exact.
#[test]
fn the_candidate_document_crosses_the_socket_as_the_owners_exact_bytes() {
    let runtime = TestRuntime::new("c7bytes");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let prepared_mission = prepare_c6_fixture(&client);
    let receipt = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C7T.READ.THE_CANDIDATE_IS_PREPARED: {error}"));

    // Counted BEFORE the read, on the settled store. The Artifact creation sequence alone would not
    // prove a read writes nothing -- these counts bracket the read itself.
    let before = domain_row_counts(&runtime.store);

    let document = client
        .read_outcome_candidate_document(&receipt.manifest_digest.value)
        .unwrap_or_else(|error| panic!("C7T.READ.THE_OWNER_ANSWERS: {error}"));

    let after = domain_row_counts(&runtime.store);
    assert_eq!(
        after, before,
        "C7T.READ.A_READ_WRITES_NOTHING: manifests/artifacts/events moved"
    );

    // The owner's own records, read independently of the transport.
    let accounts = Arc::new(OperatorAccountService::open(&runtime.store).unwrap());
    accounts
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap();
    let outcomes = OutcomeService::new(Arc::clone(&accounts), prepared_mission.workspace_id);
    let stored = outcomes
        .manifest(&receipt.manifest_digest.value)
        .expect("C7T.READ.THE_MANIFEST_IS_READABLE");
    let record = outcomes
        .candidate(&receipt.manifest_digest.value)
        .expect("C7T.READ.THE_CANDIDATE_IS_READABLE");
    let payload = record.payload();

    assert_eq!(
        document.mission_id, prepared_mission.mission_id,
        "C7T.DOCUMENT.MISSION_ID_IS_THE_FIXTURES"
    );
    assert_eq!(
        document.manifest_id,
        stored.manifest_id(),
        "C7T.DOCUMENT.MANIFEST_ID_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            document.manifest_digest.algorithm.as_str(),
            document.manifest_digest.canonicalizer.as_str(),
            document.manifest_digest.value.as_str(),
        ),
        (
            stored.digest().algorithm.as_str(),
            stored.digest().canonicalizer.as_str(),
            stored.digest().value.as_str(),
        ),
        "C7T.DOCUMENT.MANIFEST_DIGEST_ENVELOPE_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            document.candidate_artifact_digest.algorithm.as_str(),
            document.candidate_artifact_digest.canonicalizer.as_str(),
            document.candidate_artifact_digest.value.as_str(),
        ),
        (
            record.digest().algorithm.as_str(),
            record.digest().canonicalizer.as_str(),
            record.digest().value.as_str(),
        ),
        "C7T.DOCUMENT.ARTIFACT_DIGEST_ENVELOPE_IS_THE_OWNERS"
    );
    assert_eq!(
        (
            document.candidate_content_digest.algorithm.as_str(),
            document.candidate_content_digest.canonicalizer.as_str(),
            document.candidate_content_digest.value.as_str(),
        ),
        (
            payload.content_digest.algorithm.as_str(),
            payload.content_digest.canonicalizer.as_str(),
            payload.content_digest.value.as_str(),
        ),
        "C7T.DOCUMENT.CONTENT_DIGEST_ENVELOPE_IS_THE_OWNERS"
    );
    // TWO different envelopes over two different things. A DTO that filled both from one source
    // would satisfy every equality above only if the owner's two digests happened to agree -- they do
    // not, and this says so out loud.
    assert_ne!(
        document.candidate_artifact_digest.value, document.candidate_content_digest.value,
        "C7T.DOCUMENT.THE_RECORD_AND_ITS_CONTENT_ARE_DIFFERENT_IDENTITIES"
    );
    assert_ne!(
        document.candidate_artifact_digest.canonicalizer,
        document.candidate_content_digest.canonicalizer,
        "C7T.DOCUMENT.THE_TWO_DIGESTS_ARE_UNDER_DIFFERENT_CANONICALIZERS"
    );
    assert_eq!(
        document.candidate_content_bytes, payload.content_bytes,
        "C7T.DOCUMENT.CONTENT_BYTES_IS_THE_OWNERS"
    );
    assert_eq!(
        document.media_type, payload.media_type,
        "C7T.DOCUMENT.MEDIA_TYPE_IS_THE_OWNERS"
    );
    // The receipt C6 handed the Operator and the document C7 answered with name the same records.
    assert_eq!(
        (
            document.manifest_digest.value.as_str(),
            document.candidate_artifact_digest.value.as_str(),
            document.candidate_content_digest.value.as_str(),
            document.candidate_content_bytes,
        ),
        (
            receipt.manifest_digest.value.as_str(),
            receipt.candidate_artifact_digest.value.as_str(),
            receipt.candidate_content_digest.value.as_str(),
            receipt.candidate_content_bytes,
        ),
        "C7T.DOCUMENT.THE_HELD_RECEIPT_AND_THE_DOCUMENT_AGREE"
    );

    // And the bytes themselves.
    let decoded = data_encoding::BASE64
        .decode(document.candidate_content_base64.as_bytes())
        .unwrap_or_else(|error| panic!("C7T.DOCUMENT.THE_BODY_IS_CANONICAL_BASE64: {error}"));
    assert_eq!(
        decoded.as_slice(),
        record.content(),
        "C7T.DOCUMENT.THE_BYTES_ARE_THE_OWNERS_EXACTLY"
    );
    assert_eq!(
        decoded.len() as u64,
        document.candidate_content_bytes,
        "C7T.DOCUMENT.THE_DECODED_LENGTH_IS_THE_DECLARED_COUNT"
    );
    assert!(
        std::str::from_utf8(&decoded).is_ok(),
        "C7T.DOCUMENT.THE_BYTES_ARE_TEXT"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The Command journey retains its completion and candidate across a real stop, reopen and sign-in.
/// C6/C7 own the individual command and encoding cases; this bar owns their restart seam. All domain
/// setup uses their public-Command fixture, including classification BEFORE completion. The local
/// HTTP stub supplies model metadata only; there is no model invocation or live provider activation.
#[test]
fn the_command_journey_retains_completion_and_candidate_bytes_across_server_restart() {
    // Keep both servers joined even when a retention assertion panics. serve_until polls a
    // nonblocking listener, so setting its flag is sufficient; this guard adds no sleep or wake peer.
    struct ServerSession {
        running: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<Result<(), LocalTransportError>>>,
    }

    impl ServerSession {
        fn start(server: LocalSupervisorServer) -> Self {
            let running = Arc::new(AtomicBool::new(true));
            let server_running = Arc::clone(&running);
            Self {
                running,
                thread: Some(thread::spawn(move || server.serve_until(server_running))),
            }
        }

        fn stop(&mut self) {
            self.running.store(false, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                thread
                    .join()
                    .expect("B14.JOURNEY.SERVER_THREAD_JOINS")
                    .expect("B14.JOURNEY.SERVER_STOPS_CLEANLY");
            }
        }
    }

    impl Drop for ServerSession {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Release);
            if let Some(thread) = self.thread.take() {
                // Cleanup must not replace the first assertion failure with a second panic.
                let _ = thread.join();
            }
        }
    }

    let runtime = TestRuntime::retained("b14journey");
    let save_json = |name: &str, value: serde_json::Value| {
        write_journey_evidence(&runtime.root, name, value.to_string().as_bytes())
            .expect("B14.JOURNEY.EVIDENCE_IS_RETAINED_WITHOUT_OVERWRITE");
    };
    save_json(
        "inputs.json",
        json!({
            "synthetic": true,
            "desired_outcome": "Ship one reviewed feature",
            "success_conditions": ["Relevant gates pass"],
            "setup": "existing public prepare_c6_fixture; classification before completion",
            "completion_claim": "fixture declaration only; no performed work or passing gate implied",
            "spend": "not_measured",
            "model_invocation": "NOT_RUN",
            "evaluation": "NOT_RUN",
            "publication_delivery_pdf_par": "NOT_RUN",
            "host_wide_effects": "UNKNOWN",
            "whole_journey_deadline": "NOT_ESTABLISHED"
        }),
    );
    let mut metadata = JourneyMetadata::start(&runtime.root);
    let address = metadata.address;
    let bind_server = |phase| {
        assert!(
            matches!(fs::symlink_metadata(&runtime.socket), Err(error) if error.kind() == std::io::ErrorKind::NotFound),
            "B14.JOURNEY.BIND_NEVER_REMOVES_A_PREEXISTING_SOCKET"
        );
        LocalSupervisorServer::bind(LocalServerConfig {
            store_path: runtime.store.clone(),
            initial_workspace_name: "Personal".to_owned(),
            socket_path: runtime.socket.clone(),
            auth_token_path: runtime.token.clone(),
            lock_path: runtime.lock.clone(),
            local_model_endpoint: LocalModelEndpoint::new(address)
                .expect("B14.JOURNEY.METADATA_ENDPOINT_IS_LOCAL"),
        })
        .unwrap_or_else(|error| panic!("B14.JOURNEY.SERVER_BINDS: {phase}: {error}"))
    };
    let mut original_server = ServerSession::start(bind_server("original"));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token)
        .expect("B14.JOURNEY.ORIGINAL_CLIENT_CONNECTS");
    let fixture = prepare_c6_fixture(&client);
    let mission = client
        .mission(fixture.mission_id)
        .expect("B14.JOURNEY.MISSION_READS");
    let project = client
        .project(fixture.project_id)
        .expect("B14.JOURNEY.PROJECT_READS");
    let team = client
        .team(fixture.team_id)
        .expect("B14.JOURNEY.TEAM_READS");
    let classification = client
        .get_mission_classification(fixture.mission_id)
        .expect("B14.JOURNEY.CLASSIFICATION_READS")
        .latest
        .expect("B14.JOURNEY.CLASSIFICATION_EXISTS");
    save_json(
        "stored-inputs.json",
        json!({
            "synthetic": true, "mission": mission, "project": project,
            "team": team, "classification": classification
        }),
    );
    assert_eq!(
        mission.desired_outcome, "Ship one reviewed feature",
        "B14.JOURNEY.STORED_OUTCOME_MATCHES_INPUT"
    );
    assert_eq!(
        mission.success_conditions,
        ["Relevant gates pass"],
        "B14.JOURNEY.STORED_CONDITIONS_MATCH_INPUT"
    );
    assert_eq!(mission.workspace_id, fixture.workspace_id);
    assert_eq!(mission.project_id, project.id);
    assert_eq!(mission.charter_revision, fixture.charter_revision);
    assert_eq!(project.charter.revision, fixture.charter_revision);
    assert_eq!(project.workspace_id, fixture.workspace_id);
    assert_eq!(mission.team_id, team.id);
    assert_eq!(team.project_id, project.id);
    assert_eq!(team.workspace_id, fixture.workspace_id);
    assert_eq!(mission.admitted_by, MissionAdmissionActor::Operator);
    assert_eq!(team.formed_by, TeamFormationActor::Operator);
    assert_eq!(mission.members.len(), 1);
    assert_eq!(team.members.len(), 1);
    assert_eq!(mission.members[0].resident_id, fixture.resident_id);
    assert_eq!(
        mission.members[0].admission_configuration_id,
        fixture.configuration_id
    );
    assert_eq!(team.members[0].resident_id, fixture.resident_id);
    assert_eq!(
        team.members[0].formation_configuration_id,
        fixture.configuration_id
    );
    assert_eq!(classification.mission_id, fixture.mission_id);
    assert_eq!(classification.policy.project_id, fixture.project_id);
    assert_eq!(
        classification.policy.policy_revision,
        fixture.policy_revision
    );
    assert_eq!(
        classification.policy.charter_revision,
        fixture.charter_revision
    );
    assert_eq!(classification.impact, "high");
    assert_eq!(classification.basis, "operator_declared");
    assert!(
        client
            .mission_publications(fixture.mission_id)
            .expect("B14.JOURNEY.PUBLICATIONS_READ_BEFORE_PREPARE")
            .is_empty()
    );
    let completion_before = client
        .mission_completion(fixture.mission_id)
        .expect("B14.JOURNEY.COMPLETION_READS_BEFORE_RESTART")
        .expect("B14.JOURNEY.COMPLETION_EXISTS_BEFORE_RESTART");
    let intent_before = client
        .get_mission_completion_intent(fixture.mission_id)
        .expect("B14.JOURNEY.INTENT_READS_BEFORE_RESTART")
        .expect("B14.JOURNEY.INTENT_EXISTS_BEFORE_RESTART");
    let receipt_before = client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .expect("B14.JOURNEY.CANDIDATE_PREPARES_BEFORE_RESTART");
    let document_before = client
        .read_outcome_candidate_document(&receipt_before.manifest_digest.value)
        .expect("B14.JOURNEY.DOCUMENT_READS_BEFORE_RESTART");
    // Capture returned data even when the following independent owner comparisons fail. These
    // DTOs contain only this synthetic document/provenance, never a transport auth envelope.
    save_json(
        "before-restart.json",
        json!({
            "synthetic": true, "receipt": receipt_before, "document": document_before,
            "completion": completion_before
        }),
    );

    // Independent immutable records, not expectations reconstructed from transport fields. The
    // public completion view is deliberately partial, so its owner's verified reader supplies the
    // whole completion. This read-only connection executes no test SQL and plants no domain state.
    // Each call drops its services/connections, leaving only owned values across the server stop.
    let read_owner_records = |phase| {
        let accounts = Arc::new(
            OperatorAccountService::open(&runtime.store).expect("B14.JOURNEY.OWNER_STORE_OPENS"),
        );
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("B14.JOURNEY.OWNER_AUTHENTICATES");
        let outcomes = OutcomeService::new(accounts, fixture.workspace_id);
        let manifest = outcomes
            .manifest(&receipt_before.manifest_digest.value)
            .unwrap_or_else(|error| panic!("B14.JOURNEY.OWNER_MANIFEST_READS: {phase}: {error}"));
        let candidate = outcomes
            .candidate(&receipt_before.manifest_digest.value)
            .unwrap_or_else(|error| panic!("B14.JOURNEY.OWNER_CANDIDATE_READS: {phase}: {error}"));
        let connection = rusqlite::Connection::open_with_flags(
            &runtime.store,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("B14.JOURNEY.COMPLETION_READER_OPENS_READ_ONLY");
        let completion = beacond::verified_completion_within(
            &connection,
            fixture.workspace_id,
            &manifest.manifest().completion.digest.value,
        )
        .unwrap_or_else(|error| panic!("B14.JOURNEY.OWNER_COMPLETION_READS: {phase}: {error}"));
        (manifest, candidate, completion)
    };
    let (manifest_before, candidate_before, full_completion_before) =
        read_owner_records("before restart");
    let bytes_before = data_encoding::BASE64
        .decode(document_before.candidate_content_base64.as_bytes())
        .expect("B14.JOURNEY.DOCUMENT_DECODES_BEFORE_RESTART");
    for (name, bytes) in [
        ("candidate-before.md", bytes_before.clone()),
        (
            "manifest-owner.bin",
            manifest_before.canonical_bytes().to_vec(),
        ),
        (
            "artifact-owner.bin",
            candidate_before
                .payload()
                .canonical_bytes()
                .expect("B14.JOURNEY.ARTIFACT_CANONICAL_BYTES"),
        ),
        (
            "completion-owner.bin",
            full_completion_before
                .canonical_bytes()
                .expect("B14.JOURNEY.COMPLETION_CANONICAL_BYTES"),
        ),
    ] {
        write_journey_evidence(&runtime.root, name, &bytes)
            .expect("B14.JOURNEY.OWNER_BYTES_ARE_RETAINED");
    }
    assert_eq!(
        bytes_before.as_slice(),
        candidate_before.content(),
        "B14.JOURNEY.INITIAL_DOCUMENT_IS_THE_OWNERS_BYTES"
    );
    let manifest = manifest_before.manifest();
    assert_eq!(full_completion_before.workspace_id, fixture.workspace_id);
    assert_eq!(full_completion_before.project_id, fixture.project_id);
    assert_eq!(full_completion_before.mission_id, fixture.mission_id);
    assert_eq!(
        full_completion_before.actor,
        beacond::CompletionActor::Operator
    );
    assert_eq!(
        full_completion_before.accountable_resident_id,
        fixture.resident_id
    );
    assert_eq!(
        full_completion_before.accountable_resident_configuration_id,
        fixture.configuration_id
    );
    assert_eq!(
        full_completion_before.declared_conditions_sha256,
        beacond::declared_conditions_digest(&["Relevant gates pass".to_owned()])
    );
    assert_eq!(manifest.workspace_id, fixture.workspace_id);
    assert_eq!(manifest.mission_id, fixture.mission_id);
    assert_eq!(manifest.accountable_resident_id, fixture.resident_id);
    assert_eq!(
        manifest.boundary_event_seq,
        full_completion_before.boundary_event_seq
    );
    assert_eq!(
        manifest.completion_recorded_at_ms,
        full_completion_before.completed_at_ms
    );
    assert_eq!(manifest.completion.kind, beacond::SourceKind::Completion);
    assert_eq!(
        manifest
            .completion
            .identity
            .as_ref()
            .map(|identity| identity.object_id),
        Some(full_completion_before.completion_id)
    );
    let completion_digest = full_completion_before
        .digest()
        .expect("B14.JOURNEY.COMPLETION_DIGEST");
    assert_eq!(
        (
            &manifest.completion.digest.algorithm,
            &manifest.completion.digest.canonicalizer,
            &manifest.completion.digest.value
        ),
        (
            &completion_digest.algorithm,
            &completion_digest.canonicalizer,
            &completion_digest.value
        )
    );
    assert_eq!(
        manifest.policy_classification.kind,
        beacond::SourceKind::Policy
    );
    assert_eq!(
        manifest
            .policy_classification
            .identity
            .as_ref()
            .map(|identity| identity.object_id),
        Some(classification.classification_id)
    );
    assert_eq!(
        (
            &manifest.policy_classification.digest.algorithm,
            &manifest.policy_classification.digest.canonicalizer,
            &manifest.policy_classification.digest.value
        ),
        (
            &classification.digest.algorithm,
            &classification.digest.canonicalizer,
            &classification.digest.value
        )
    );
    assert!(manifest.policy_high_impact);
    let payload = candidate_before.payload();
    assert_eq!(payload.workspace_id, fixture.workspace_id);
    assert_eq!(payload.accountable_resident_id, fixture.resident_id);
    assert_eq!(
        payload.accountable_configuration_id,
        fixture.configuration_id
    );
    assert_eq!(
        payload.context,
        beacond::ArtifactContext::Mission {
            project_id: fixture.project_id,
            mission_id: fixture.mission_id
        }
    );
    assert_eq!(document_before.mission_id, fixture.mission_id);
    assert_eq!(receipt_before.mission_id, fixture.mission_id);
    assert_eq!(document_before.manifest_id, manifest_before.manifest_id());
    assert_eq!(receipt_before.manifest_id, document_before.manifest_id);
    assert_eq!(
        receipt_before.manifest_digest,
        document_before.manifest_digest
    );
    assert_eq!(
        receipt_before.candidate_artifact_digest,
        document_before.candidate_artifact_digest
    );
    assert_eq!(
        receipt_before.appendix_request,
        AppendixRequestDto::PolicyDefault
    );
    assert_eq!(
        serde_json::to_value(&receipt_before.appendix_mode)
            .expect("B14.JOURNEY.APPENDIX_MODE_SERIALIZES"),
        json!("included")
    );
    assert_eq!(
        (
            &document_before.manifest_digest.algorithm,
            &document_before.manifest_digest.canonicalizer,
            &document_before.manifest_digest.value
        ),
        (
            &manifest_before.digest().algorithm,
            &manifest_before.digest().canonicalizer,
            &manifest_before.digest().value
        )
    );
    assert_eq!(
        (
            &document_before.candidate_artifact_digest.algorithm,
            &document_before.candidate_artifact_digest.canonicalizer,
            &document_before.candidate_artifact_digest.value
        ),
        (
            &candidate_before.digest().algorithm,
            &candidate_before.digest().canonicalizer,
            &candidate_before.digest().value
        )
    );
    assert_eq!(
        document_before.candidate_artifact_digest.value,
        manifest_before.candidate_artifact_digest()
    );
    assert_eq!(document_before.candidate_content_digest.algorithm, "sha256");
    assert_eq!(
        document_before.candidate_content_digest.canonicalizer,
        "beacon.raw.v1"
    );
    assert_ne!(
        document_before.candidate_artifact_digest.canonicalizer,
        document_before.candidate_content_digest.canonicalizer
    );
    assert_eq!(
        document_before.candidate_content_digest.value,
        data_encoding::HEXLOWER.encode(&Sha256::digest(&bytes_before)),
        "B14.JOURNEY.RAW_CONTENT_HASH_IS_INDEPENDENT_OF_THE_RECORD_HASH"
    );
    assert_eq!(
        document_before.candidate_content_bytes,
        bytes_before.len() as u64
    );
    assert_eq!(
        document_before.candidate_content_digest.value,
        payload.content_digest.value
    );
    assert_eq!(
        receipt_before.candidate_content_digest,
        document_before.candidate_content_digest
    );
    assert_eq!(
        receipt_before.candidate_content_bytes,
        document_before.candidate_content_bytes
    );
    assert_eq!(
        full_completion_before.spend,
        beacond::SpendReference::NotMeasured("no meter on this Mission".to_owned())
    );
    assert!(
        matches!(&manifest.spend_ref, beacond::OutcomeState::NotEvaluated(reason) if reason == "no meter on this Mission")
    );
    let markdown = std::str::from_utf8(&bytes_before).expect("B14.JOURNEY.CANDIDATE_IS_UTF8");
    assert!(
        markdown.contains("Ship one reviewed feature"),
        "B14.JOURNEY.MARKDOWN_RETAINS_THE_OUTCOME"
    );
    assert!(
        markdown.contains("Relevant gates pass"),
        "B14.JOURNEY.MARKDOWN_RETAINS_THE_CONDITION"
    );
    assert!(
        markdown.contains(
            "No amount is stated. An unmeasured or absent spend record is not zero spend."
        ),
        "B14.JOURNEY.MARKDOWN_DOES_NOT_PROMOTE_UNMEASURED_TO_ZERO"
    );

    drop(client);
    original_server.stop();
    assert!(
        !runtime.socket.exists(),
        "B14.JOURNEY.ORIGINAL_SERVER_RELEASES_ITS_SOCKET"
    );
    save_json(
        "original-server-stopped.json",
        json!({
            "synthetic": true, "serve_until_joined": true,
            "socket_absent": true, "mechanism": "native server Drop unlinks its owned socket",
            "physical_exit_deadline": "NOT_ESTABLISHED"
        }),
    );
    let mut reopened_server = ServerSession::start(bind_server("reopened same store"));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token)
        .expect("B14.JOURNEY.REOPENED_CLIENT_CONNECTS");
    assert_eq!(
        client
            .status()
            .expect("B14.JOURNEY.REOPENED_STATUS_READS")
            .phase,
        OperatorSessionPhase::Locked,
        "B14.JOURNEY.REOPENED_SESSION_REQUIRES_SIGN_IN"
    );
    let locked_read = client.mission_completion(fixture.mission_id);
    assert!(
        matches!(
            locked_read,
            Err(LocalTransportError::Remote {
                code: LocalErrorCode::NotAuthenticated,
                ..
            })
        ),
        "B14.JOURNEY.REOPENED_COMPLETION_REQUIRES_SIGN_IN: {locked_read:?}"
    );
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .expect("B14.JOURNEY.REOPENED_SESSION_AUTHENTICATES");

    // Read the held records BEFORE repeating prepare; a repeat must not repair lost retention.
    assert_eq!(
        client
            .mission_completion(fixture.mission_id)
            .expect("B14.JOURNEY.COMPLETION_READS_AFTER_RESTART"),
        Some(completion_before),
        "B14.JOURNEY.COMPLETION_VIEW_SURVIVES_RESTART"
    );
    assert_eq!(
        client
            .get_mission_completion_intent(fixture.mission_id)
            .expect("B14.JOURNEY.INTENT_READS_AFTER_RESTART"),
        Some(intent_before),
        "B14.JOURNEY.REVIEWED_INTENT_SURVIVES_RESTART"
    );
    let document_after = client
        .read_outcome_candidate_document(&receipt_before.manifest_digest.value)
        .expect("B14.JOURNEY.HELD_DOCUMENT_READS_BEFORE_REPEAT");
    save_json(
        "after-restart-before-repeat.json",
        json!({
            "synthetic": true, "reopened_locked_then_authenticated": true,
            "repeat_prepare_issued": false, "document": document_after
        }),
    );
    assert_eq!(
        document_after, document_before,
        "B14.JOURNEY.FULL_DOCUMENT_AND_BYTES_SURVIVE_RESTART"
    );
    let (manifest_after, candidate_after, full_completion_after) =
        read_owner_records("after restart before repeat");
    assert_eq!(
        full_completion_after, full_completion_before,
        "B14.JOURNEY.FULL_COMPLETION_RECORD_SURVIVES_RESTART"
    );
    assert_eq!(
        manifest_after, manifest_before,
        "B14.JOURNEY.FULL_MANIFEST_AND_CANONICAL_BYTES_SURVIVE_RESTART"
    );
    assert_eq!(
        candidate_after, candidate_before,
        "B14.JOURNEY.FULL_CANDIDATE_AND_CREATION_EVENT_SURVIVE_RESTART"
    );

    // A narrow snapshot, not a whole-store/no-write claim. Owner authentication legitimately
    // writes account/security_events; these three domain tables exclude that bookkeeping.
    let counts = || {
        let mut connection = rusqlite::Connection::open_with_flags(
            &runtime.store,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .expect("B14.JOURNEY.CENSUS_OPENS_READ_ONLY");
        connection
            .busy_timeout(Duration::from_secs(5))
            .expect("B14.JOURNEY.CENSUS_BUSY_TIMEOUT");
        let read = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Deferred)
            .expect("B14.JOURNEY.CENSUS_SNAPSHOT");
        let row: (i64, i64, i64) = read.query_row(
            "SELECT (SELECT count(*) FROM outcome_manifests), (SELECT count(*) FROM artifacts), (SELECT count(*) FROM events)",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).expect("B14.JOURNEY.CENSUS_READS");
        read.commit().expect("B14.JOURNEY.CENSUS_CLOSES");
        row
    };
    let before_repeat_counts = counts();
    let receipt_after = client
        .prepare_outcome_candidate(fixture.mission_id, AppendixRequestDto::PolicyDefault)
        .expect("B14.JOURNEY.REPEAT_PREPARE_ACCEPTED_AFTER_RESTART");
    assert_eq!(
        receipt_after, receipt_before,
        "B14.JOURNEY.REPEAT_PREPARE_RETURNS_THE_SAME_IDENTITY"
    );
    assert_eq!(
        client
            .read_outcome_candidate_document(&receipt_before.manifest_digest.value)
            .expect("B14.JOURNEY.HELD_DOCUMENT_READS_AFTER_REPEAT"),
        document_before,
        "B14.JOURNEY.REPEAT_PREPARE_PRESERVES_FULL_DOCUMENT_AND_BYTES"
    );
    assert_eq!(
        read_owner_records("after repeat"),
        (manifest_before, candidate_before, full_completion_before),
        "B14.JOURNEY.REPEAT_PREPARE_PRESERVES_FULL_OWNER_RECORDS"
    );
    // The same receipt/document is public observational idempotence. The owner equality also
    // retains the Artifact creation event; neither proves that no unrelated row or Event was written.
    let after_repeat_counts = counts();
    let publications = client
        .mission_publications(fixture.mission_id)
        .expect("B14.JOURNEY.PUBLICATIONS_READ_AFTER_REPEAT");
    save_json(
        "after-repeat.json",
        json!({
            "synthetic": true, "receipt": receipt_after,
            "count_order": ["outcome_manifests", "artifacts", "events"],
            "before_counts": before_repeat_counts, "after_counts": after_repeat_counts,
            "publications": publications,
            "account_security_events": "AUTHENTICATION_WRITES_EXPECTED_NOT_CENSUSED",
            "other_tables_or_host_effects": "UNKNOWN"
        }),
    );
    assert_eq!(
        after_repeat_counts, before_repeat_counts,
        "B14.JOURNEY.REPEAT_PRESERVES_THREE_DOMAIN_COUNTS"
    );
    assert!(
        publications.is_empty(),
        "B14.JOURNEY.CANDIDATE_IS_NOT_PUBLICATION"
    );
    reopened_server.stop();
    let metadata_observation = metadata
        .stop()
        .expect("B14.JOURNEY.METADATA_JOINS_AND_RETAINS_COUNTS");
    assert_eq!(
        metadata_observation.failure, None,
        "B14.JOURNEY.METADATA_HAS_NO_FIXTURE_FAILURE"
    );
    assert_eq!(metadata_observation.accepted_requests, 6);
    assert_eq!(metadata_observation.completed_responses, 6);
    assert_eq!(
        metadata_observation.routes,
        [
            "GET /api/tags HTTP/1.1",
            "POST /api/show HTTP/1.1",
            "GET /api/tags HTTP/1.1",
            "GET /api/tags HTTP/1.1",
            "POST /api/show HTTP/1.1",
            "GET /api/tags HTTP/1.1"
        ]
    );
    assert!(
        runtime.root.is_dir() && runtime.store.is_file(),
        "B14.JOURNEY.ORIGINAL_FIXTURE_STILL_EXISTS"
    );
    save_json(
        "journey-assertions.json",
        json!({
            "synthetic": true, "assertions_reached_end": true,
            "both_native_servers_joined": true, "metadata_thread_joined": true,
            "scope": "declared synthetic records and candidate bytes survive same-store restart",
            "performed_work_evaluation_publication_delivery": "NOT_PROVEN",
            "process_physical_exit_or_global_effects": "UNKNOWN"
        }),
    );
}

/// A foreign Workspace's REAL manifest and a digest that never existed answer with the same bytes.
///
/// Compared as whole serialized replies, not as enum arms, because the disclosure this bar exists to
/// prevent is a difference a caller can SEE. The owner returns two distinct refusals for these two
/// cases (`verified_manifest_within` selects on the digest alone and only then compares the stored
/// Workspace), so the sameness is the transport's doing and has to be witnessed at the wire.
///
/// THE ONE FIXTURE WRITE NO PUBLIC OWNER CAN PRODUCE, and why it is unavoidable: this store bootstraps
/// exactly ONE Workspace, and the supervisor binds to it, so no sequence of public commands can put a
/// second Workspace's manifest in it. The manifest below is minted by the real owner through public
/// commands and remains in its original Workspace. The fixture inserts one second Workspace row so
/// the next supervisor bootstrap binds to that Workspace instead. The canonical records are untouched;
/// only the asking Workspace changes. The happy path is the separate bar above.
#[test]
fn an_absent_manifest_and_a_foreign_manifest_answer_identical_bytes() {
    let runtime = TestRuntime::new("c7opaque");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let prepared_mission = prepare_c6_fixture(&client);
    let receipt = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C7T.OPACITY.THE_CANDIDATE_IS_PREPARED: {error}"));
    let real_digest = receipt.manifest_digest.value.clone();

    // It reads as this Workspace's own first; the second Workspace below changes who is asking.
    client
        .read_outcome_candidate_document(&real_digest)
        .unwrap_or_else(|error| panic!("C7T.OPACITY.THE_MANIFEST_IS_OURS_FIRST: {error}"));

    // Stop the first supervisor before touching the store.
    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();

    // The second Workspace, inserted the way every multi-Workspace fixture in this crate inserts one
    // (`sample_project.rs:3159-3166` records why: `Supervisor::bootstrap` is the ONLY site that
    // creates a Workspace, and only when the store has none). Its `created_at_ms` is 0 so the next
    // bootstrap's `ORDER BY created_at_ms, id LIMIT 1` (`lib.rs:1730-1740`) binds to IT.
    //
    // NOTHING canonical is fabricated: the manifest, its Artifact and its whole Mission chain are the
    // real ones the owner minted above, and they keep belonging to the first Workspace. What changes
    // is only WHO is asking.
    let elsewhere = Uuid::new_v4();
    let connection = rusqlite::Connection::open(&runtime.store).unwrap();
    connection
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Second', 0)",
            rusqlite::params![elsewhere.to_string()],
        )
        .unwrap_or_else(|error| panic!("C7T.OPACITY.THE_SECOND_WORKSPACE_INSERTS: {error}"));
    drop(connection);

    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            AccessOrigin::SameMachine,
        )
        .unwrap_or_else(|error| panic!("C7T.OPACITY.THE_SESSION_REOPENS: {error}"));

    // Raw bytes on both sides: the comparison is of what the caller receives, not of what it parses.
    let token = fs::read_to_string(&runtime.token).unwrap();
    let ask = |digest: &str| -> Vec<u8> {
        let body = serde_json::to_vec(&json!({
            "protocol": 1,
            "token": token,
            "command": {
                "name": "read_outcome_candidate_document",
                "manifest_digest": digest,
            },
        }))
        .unwrap();
        let mut stream = UnixStream::connect(&runtime.socket).unwrap();
        stream.write_all(&body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        answer
    };

    // The control that makes this a CROSS-WORKSPACE bar rather than a second absence bar: the row is
    // STILL THERE, under the first Workspace, while the service now asking is the second. Without
    // this the two replies could be identical simply because both manifests were gone.
    let connection = rusqlite::Connection::open(&runtime.store).unwrap();
    let owner: String = connection
        .query_row(
            "SELECT workspace_id FROM outcome_manifests WHERE manifest_digest = ?1",
            rusqlite::params![real_digest],
            |row| row.get(0),
        )
        .unwrap_or_else(|error| {
            panic!("C7T.OPACITY.THE_REAL_MANIFEST_ROW_IS_STILL_PRESENT: {error}")
        });
    drop(connection);
    assert_eq!(
        owner,
        prepared_mission.workspace_id.to_string(),
        "C7T.OPACITY.THE_REAL_MANIFEST_STILL_BELONGS_TO_THE_FIRST_WORKSPACE"
    );
    assert_ne!(
        owner,
        elsewhere.to_string(),
        "C7T.OPACITY.THE_ASKING_WORKSPACE_IS_NOT_THE_OWNING_ONE"
    );

    let foreign = ask(&real_digest);
    let never_existed = ask("00000000000000000000000000000000000000000000000000000000000000ab");
    assert_eq!(
        String::from_utf8_lossy(&foreign),
        String::from_utf8_lossy(&never_existed),
        "C7T.OPACITY.A_FOREIGN_MANIFEST_AND_AN_ABSENT_ONE_ARE_ONE_ANSWER"
    );
    let answered: serde_json::Value = serde_json::from_slice(&foreign).unwrap();
    assert_eq!(
        answered
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str),
        Some("outcome_candidate_not_found"),
        "C7T.OPACITY.THE_SHARED_ANSWER_IS_NOT_FOUND"
    );
    // And the answer names nothing the caller did not send.
    let text = String::from_utf8_lossy(&foreign);
    for leaked in [
        elsewhere.to_string(),
        prepared_mission.mission_id.to_string(),
        prepared_mission.project_id.to_string(),
        prepared_mission.workspace_id.to_string(),
        real_digest.clone(),
    ] {
        assert!(
            !text.contains(&leaked),
            "C7T.OPACITY.THE_REFUSAL_NAMES_NOTHING_ELSE: {leaked}"
        );
    }

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// A candidate Artifact that has gone missing is `unavailable`, never absence -- and the stage the
/// fault ACTUALLY arises at, measured rather than assumed.
///
/// This bar first claimed the fault was `candidate()`'s `Artifact(NotFound)` at the second read. It is
/// not, and root's narrowing is what exposed it: calling the owner's own verifier after the deletion
/// returns `ManifestStoreRefusal::CandidateNotAccounted`, so the manifest **stops verifying at the
/// FIRST read**. A manifest's candidate Artifact IS its accounting -- there is no `outcome.prepared`
/// Event, so the Artifact is the manifest's position in the log (R18.7,
/// `outcome_store.rs:146-152`) -- and removing it makes the manifest unaccounted, not merely
/// candidate-less.
///
/// **Consequence, stated rather than papered over:** a genuine stage-2 `Artifact(NotFound)` is
/// structurally unreachable while the manifest verifies, because the state it needs -- a verifying
/// manifest whose Artifact is absent -- is the state the owner refuses to call verified. The stage-2
/// vocabulary is therefore witnessed by the labelled mapper bar
/// `the_candidate_read_stages_map_to_their_own_answers`, and reported NOT PROBED live with this
/// reason.
///
/// The same accounting is why a live non-UTF-8 candidate is unreachable too: `candidate_proves_manifest`
/// decodes the content and returns false when it is not UTF-8 (`outcome_store.rs:529-537`), and
/// `verified_manifest_within` calls it -- so such a store fails the FIRST read as well. The
/// transport's own decode check is defensive, and no bar here claims to have reached it.
///
/// What this bar DOES prove, and it is the behaviour that matters to a caller: a first-read refusal
/// that is not `Absent`/`CrossWorkspace` answers `outcome_candidate_unavailable` and never
/// `outcome_candidate_not_found`, so a broken record is never reported as an absent one.
///
/// THE CORRUPTION FIXTURE, labelled and bounded. The candidate is minted by the real owner through
/// public commands first; only then is one row removed. `artifacts` carries an unconditional
/// `BEFORE DELETE` trigger that aborts with "Artifacts are immutable"
/// (`migrations/0027_artifacts.sql:186-190`) -- the product's anti-tamper guard, and it refused this
/// delete when it was first attempted. The fixture therefore DROPS that one trigger, deletes the one
/// row, and RECREATES the trigger verbatim, asserting each step. Nothing in production changes, no
/// failpoint is added, and the owner happy path above is untouched: this store is already corrupt by
/// the time the read runs, which is the whole point.
#[test]
fn a_missing_candidate_artifact_is_unavailable_and_never_absence() {
    let runtime = TestRuntime::new("c7broken");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let prepared_mission = prepare_c6_fixture(&client);
    let receipt = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C7T.BROKEN.THE_CANDIDATE_IS_PREPARED: {error}"));

    // It reads as a real document FIRST, so what changes the answer below is the missing row and
    // nothing else.
    client
        .read_outcome_candidate_document(&receipt.manifest_digest.value)
        .unwrap_or_else(|error| panic!("C7T.BROKEN.THE_DOCUMENT_READS_FIRST: {error}"));
    let before = domain_row_counts(&runtime.store);

    let connection = rusqlite::Connection::open(&runtime.store).unwrap();
    // The guard refuses this delete while it stands -- asserted, not assumed, so the fixture cannot
    // quietly stop needing the drop.
    let guarded = connection.execute(
        "DELETE FROM artifacts WHERE digest_value = ?1",
        rusqlite::params![receipt.candidate_artifact_digest.value],
    );
    assert!(
        guarded
            .as_ref()
            .err()
            .is_some_and(|error| error.to_string().contains("Artifacts are immutable")),
        "C7T.BROKEN.THE_STORES_OWN_GUARD_REFUSES_FIRST: {guarded:?}"
    );
    connection
        .execute_batch("DROP TRIGGER artifacts_no_delete")
        .unwrap_or_else(|error| panic!("C7T.BROKEN.THE_FIXTURE_DROPS_ONE_TRIGGER: {error}"));
    // The manifest's own foreign key to its candidate Artifact refuses the delete as well -- which is
    // the point of the state being built: a manifest cannot legally exist without its candidate. The
    // pragma is per-CONNECTION and belongs to this fixture connection alone; no product code and no
    // other connection is affected.
    connection
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap_or_else(|error| {
            panic!("C7T.BROKEN.THE_FIXTURE_SUSPENDS_ITS_OWN_FK_CHECK: {error}")
        });
    let removed = connection
        .execute(
            "DELETE FROM artifacts WHERE digest_value = ?1",
            rusqlite::params![receipt.candidate_artifact_digest.value],
        )
        .unwrap_or_else(|error| panic!("C7T.BROKEN.THE_ARTIFACT_IS_REMOVED: {error}"));
    assert_eq!(removed, 1, "C7T.BROKEN.EXACTLY_ONE_ARTIFACT_REMOVED");
    // Put the product's guard back, verbatim from `0027_artifacts.sql:186-190`.
    connection
        .execute_batch(
            "CREATE TRIGGER artifacts_no_delete
             BEFORE DELETE ON artifacts
             BEGIN
                 SELECT RAISE(ABORT, 'Artifacts are immutable');
             END;",
        )
        .unwrap_or_else(|error| panic!("C7T.BROKEN.THE_GUARD_IS_RESTORED: {error}"));
    drop(connection);

    // WHICH STAGE, measured through the owner's own verifier -- the same `verified_manifest_within`
    // that `OutcomeService::manifest` calls. A row count is presence, not verification.
    let verifying = rusqlite::Connection::open(&runtime.store).unwrap();
    let refusal = beacond::verified_manifest_within(
        &verifying,
        prepared_mission.workspace_id,
        &receipt.manifest_digest.value,
    )
    .expect_err("C7T.BROKEN.THE_MANIFEST_NO_LONGER_VERIFIES");
    drop(verifying);
    assert!(
        matches!(
            refusal,
            beacond::ManifestStoreRefusal::CandidateNotAccounted { .. }
        ),
        "C7T.BROKEN.THE_FAULT_IS_THE_UNACCOUNTED_CANDIDATE_AT_THE_FIRST_READ: {refusal:?}"
    );
    // And it is emphatically NOT the absence vocabulary, which is why the wire answer below is
    // unavailable rather than not-found.
    assert!(
        !matches!(
            refusal,
            beacond::ManifestStoreRefusal::Absent | beacond::ManifestStoreRefusal::CrossWorkspace
        ),
        "C7T.BROKEN.A_BROKEN_RECORD_IS_NOT_AN_ABSENT_ONE: {refusal:?}"
    );

    let token = fs::read_to_string(&runtime.token).unwrap();
    let body = serde_json::to_vec(&json!({
        "protocol": 1,
        "token": token,
        "command": {
            "name": "read_outcome_candidate_document",
            "manifest_digest": receipt.manifest_digest.value,
        },
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
        Some("outcome_candidate_unavailable"),
        "C7T.BROKEN.A_MISSING_CANDIDATE_IS_UNAVAILABLE_NOT_ABSENT"
    );
    assert_eq!(
        answered.get("result"),
        Some(&serde_json::Value::Null),
        "C7T.BROKEN.NO_DOCUMENT_IS_RETURNED"
    );
    // The manifest is still there and still readable as a manifest: this is NOT an absent manifest
    // dressed up as a broken candidate.
    let still_there = rusqlite::Connection::open(&runtime.store).unwrap();
    let manifests: i64 = still_there
        .query_row(
            "SELECT count(*) FROM outcome_manifests WHERE manifest_digest = ?1",
            rusqlite::params![receipt.manifest_digest.value],
            |row| row.get(0),
        )
        .unwrap();
    drop(still_there);
    assert_eq!(manifests, 1, "C7T.BROKEN.THE_MANIFEST_ROW_IS_STILL_PRESENT");
    // And the refused read wrote nothing of its own: only the fixture's one deletion moved.
    let after = domain_row_counts(&runtime.store);
    assert_eq!(
        after,
        (before.0, before.1 - 1, before.2),
        "C7T.BROKEN.THE_REFUSAL_WRITES_NOTHING"
    );

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// The request is exactly one required field, and what a MALFORMED digest actually answers.
///
/// Stated as observed, not as designed: `verified_manifest_within` does no upfront hex validation
/// (`outcome_store.rs:554-581` selects on the string it was given), so a digest that could never be a
/// digest finds no row and returns the owner's real `Absent` -- which this transport answers as
/// `outcome_candidate_not_found`. That is the current behaviour and this bar pins it.
///
/// A TINY CONTRACT ISSUE, flagged rather than closed here: the brief asks for an "invalid digest
/// domain" to be refused before a document can return, and today it is refused by finding nothing
/// rather than by a domain check. The observable outcome is the same opaque not-found, and adding a
/// length/alphabet guard would introduce a NEW outward answer (`protocol_error`) for input the owner
/// currently treats as a miss -- a contract change, not an implementation detail. Not taken
/// unilaterally.
#[test]
fn the_candidate_read_request_is_one_required_field() {
    let runtime = TestRuntime::new("c7request");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let prepared_mission = prepare_c6_fixture(&client);
    let receipt = client
        .prepare_outcome_candidate(
            prepared_mission.mission_id,
            AppendixRequestDto::PolicyDefault,
        )
        .unwrap_or_else(|error| panic!("C7T.REQUEST.THE_CANDIDATE_IS_PREPARED: {error}"));
    let token = fs::read_to_string(&runtime.token).unwrap();
    let ask = |command: serde_json::Value| -> serde_json::Value {
        let body = serde_json::to_vec(&json!({
            "protocol": 1,
            "token": token,
            "command": command,
        }))
        .unwrap();
        let mut stream = UnixStream::connect(&runtime.socket).unwrap();
        stream.write_all(&body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        serde_json::from_slice(&answer).unwrap()
    };
    let code_of = |answer: &serde_json::Value| -> Option<String> {
        answer
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned)
    };

    // An authenticated session: the reply carries all eight keys and nothing else.
    let answered = ask(json!({
        "name": "read_outcome_candidate_document",
        "manifest_digest": receipt.manifest_digest.value,
    }));
    assert_eq!(
        answered
            .pointer("/result/kind")
            .and_then(serde_json::Value::as_str),
        Some("outcome_candidate_document"),
        "C7T.REQUEST.THE_RESULT_NAMES_ITS_OWN_KIND"
    );
    let document = answered
        .pointer("/result/document")
        .unwrap_or_else(|| panic!("C7T.REQUEST.THE_RESULT_CARRIES_A_DOCUMENT"));
    let mut keys = document
        .as_object()
        .expect("the document is an object")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "candidateArtifactDigest".to_owned(),
            "candidateContentBase64".to_owned(),
            "candidateContentBytes".to_owned(),
            "candidateContentDigest".to_owned(),
            "manifestDigest".to_owned(),
            "manifestId".to_owned(),
            "mediaType".to_owned(),
            "missionId".to_owned(),
        ],
        "C7T.REQUEST.EXACTLY_EIGHT_CAMELCASE_KEYS"
    );
    for (field, canonicalizer) in [
        ("manifestDigest", "beacon.outcome.manifest.v2"),
        ("candidateArtifactDigest", "beacon.artifact.v1"),
        ("candidateContentDigest", "beacon.raw.v1"),
    ] {
        let envelope = document.get(field).unwrap();
        let mut envelope_keys = envelope
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        envelope_keys.sort();
        assert_eq!(
            envelope_keys,
            vec![
                "algorithm".to_owned(),
                "canonicalizer".to_owned(),
                "value".to_owned()
            ],
            "C7T.REQUEST.THE_DIGEST_ENVELOPE_IS_WHOLE: {field}"
        );
        assert_eq!(
            envelope
                .get("algorithm")
                .and_then(serde_json::Value::as_str),
            Some("sha256"),
            "C7T.REQUEST.THE_ALGORITHM_IS_NAMED: {field}"
        );
        assert_eq!(
            envelope
                .get("canonicalizer")
                .and_then(serde_json::Value::as_str),
            Some(canonicalizer),
            "C7T.REQUEST.THE_CANONICALIZER_IS_ITS_OWN: {field}"
        );
        let value = envelope
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert!(
            value.len() == 64
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "C7T.REQUEST.THE_VALUE_IS_LOWER_HEX_64: {field}"
        );
    }
    assert_eq!(
        document
            .get("mediaType")
            .and_then(serde_json::Value::as_str),
        Some("text/markdown; charset=utf-8"),
        "C7T.REQUEST.THE_MEDIA_TYPE_IS_FIXED"
    );

    // A MALFORMED digest: the observed answer, which is the opaque not-found the owner's miss
    // produces. See the tiny contract issue in this test's own documentation.
    for malformed in [
        "not-a-digest",
        "",
        "AABBCCDDEEFF00112233445566778899AABBCCDDEEFF00112233445566778899",
        "zz00000000000000000000000000000000000000000000000000000000000000",
    ] {
        let answered = ask(json!({
            "name": "read_outcome_candidate_document",
            "manifest_digest": malformed,
        }));
        assert_eq!(
            code_of(&answered).as_deref(),
            Some("outcome_candidate_not_found"),
            "C7T.REQUEST.A_MALFORMED_DIGEST_IS_THE_SAME_OPAQUE_MISS: {malformed:?}"
        );
        assert_eq!(
            answered.get("result"),
            Some(&serde_json::Value::Null),
            "C7T.REQUEST.A_MALFORMED_DIGEST_RETURNS_NO_DOCUMENT: {malformed:?}"
        );
    }

    // A wrong TYPE is a malformed envelope, not a miss: the field is a string.
    for wrong in [json!(7), json!(null), json!(["a"])] {
        let answered = ask(json!({
            "name": "read_outcome_candidate_document",
            "manifest_digest": wrong,
        }));
        assert_eq!(
            code_of(&answered).as_deref(),
            Some("protocol_error"),
            "C7T.REQUEST.THE_DIGEST_IS_A_STRING: {wrong}"
        );
    }

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}

/// A document response may cross the socket larger than the default frame -- and its ceiling is still
/// finite.
///
/// The response ceiling is a CLIENT-side bound (`response_max` is read in `send_within` before the
/// reply is taken), so this bar uses a CONTROLLED LOCAL PEER rather than the supervisor: what is under
/// test is which frame the client accepts for which command, not how a document is produced. The peer
/// is a plain `UnixListener` replying with bytes this test wrote.
///
/// Two sides of the same ceiling, because one side alone would pass with no ceiling at all:
/// a reply well past the 64 KiB default is ACCEPTED, and a reply past
/// `MAX_CANDIDATE_DOCUMENT_RESPONSE_BYTES` is REFUSED.
///
/// What this bar deliberately does NOT claim: that an old command's refusal of the same bytes isolates
/// the frame. The client answers `Protocol` both for an over-frame reply and for a reply whose result
/// arm is simply the wrong one, so that comparison would prove nothing -- an earlier version of this
/// test asserted it anyway and was wrong. That every other command keeps the default domain is proved
/// directly instead, by the lib bar
/// `the_largest_candidate_document_fits_its_own_ceiling_and_not_the_default` reading `response_max`
/// for each family.
#[test]
fn a_document_response_may_exceed_the_default_frame_and_its_ceiling_is_finite() {
    let runtime = TestRuntime::new("c7frame");
    initialize_local_auth_token(&runtime.token).unwrap();
    let listener = UnixListener::bind(&runtime.socket).unwrap();

    // One document envelope whose body is `content_bytes` bytes of content, serialized as the wire
    // would carry it. Written here, never produced by the transport under test.
    fn document_reply(content_bytes: usize) -> Vec<u8> {
        let content = vec![b'x'; content_bytes];
        let digest = |canonicalizer: &str| {
            json!({
                "algorithm": "sha256",
                "canonicalizer": canonicalizer,
                "value": "f".repeat(64),
            })
        };
        serde_json::to_vec(&json!({
            "protocol": 1,
            "result": {
                "kind": "outcome_candidate_document",
                "document": {
                    "missionId": Uuid::new_v4(),
                    "manifestId": Uuid::new_v4(),
                    "manifestDigest": digest("beacon.outcome.manifest.v2"),
                    "candidateArtifactDigest": digest("beacon.artifact.v1"),
                    "candidateContentDigest": digest("beacon.raw.v1"),
                    "candidateContentBytes": content.len(),
                    "mediaType": "text/markdown; charset=utf-8",
                    "candidateContentBase64": data_encoding::BASE64.encode(&content),
                },
            },
            "error": null,
        }))
        .unwrap()
    }

    // The LARGEST accepted document: the owner's whole 1 MiB content ceiling, crossing a real socket.
    //
    // SYNTHETIC, and labelled: this body is 1,048,576 bytes written by this test, NOT a candidate any
    // renderer produced. No such fixture exists and none is claimed. What it proves is that the real
    // client accepts the real maximum over the real transport.
    let accepted = document_reply(1_048_576);
    assert!(
        accepted.len() > 64 * 1024 && accepted.len() < 1_403_904,
        "C7T.FRAME.THE_ACCEPTED_FIXTURE_IS_BETWEEN_THE_TWO_CEILINGS: {}",
        accepted.len()
    );
    // And one past the read's own ceiling.
    let refused = document_reply(1_060_000);
    assert!(
        refused.len() > 1_403_904,
        "C7T.FRAME.THE_REFUSED_FIXTURE_IS_PAST_THE_READS_CEILING: {}",
        refused.len()
    );

    let replies = vec![accepted.clone(), refused.clone()];
    let peer = thread::spawn(move || {
        for reply in replies {
            let (mut stream, _) = listener.accept().expect("the peer accepts");
            let mut request = Vec::new();
            // Read the request first: the client shuts its write half before reading.
            stream.read_to_end(&mut request).expect("the peer reads");
            // A client that refuses the frame stops reading, so a broken pipe here is that refusal
            // arriving, not a fixture fault.
            let _ = stream.write_all(&reply);
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });

    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();

    let document = client
        .read_outcome_candidate_document(&"a".repeat(64))
        .unwrap_or_else(|error| panic!("C7T.FRAME.THE_READ_ACCEPTS_A_LARGE_REPLY: {error}"));
    // Both counts written out as literals rather than derived from the reply.
    assert_eq!(
        document.candidate_content_bytes, 1_048_576,
        "C7T.FRAME.THE_WHOLE_BODY_ARRIVED"
    );
    assert_eq!(
        document.candidate_content_base64.len(),
        1_398_104,
        "C7T.FRAME.THE_BODY_IS_WHOLE_NOT_TRUNCATED"
    );
    assert_eq!(
        data_encoding::BASE64
            .decode(document.candidate_content_base64.as_bytes())
            .expect("C7T.FRAME.THE_LARGEST_BODY_IS_CANONICAL_BASE64")
            .len(),
        1_048_576,
        "C7T.FRAME.THE_LARGEST_BODY_DECODES_TO_ITS_DECLARED_COUNT"
    );

    let over = client
        .read_outcome_candidate_document(&"b".repeat(64))
        .expect_err("C7T.FRAME.THE_READS_CEILING_IS_FINITE");
    assert!(
        matches!(over, LocalTransportError::Protocol),
        "C7T.FRAME.A_REPLY_PAST_THE_CEILING_IS_REFUSED: {over:?}"
    );

    peer.join().unwrap();
}

/// C7's candidate-document read is KNOWN to the wire, and answers with the account's own refusal.
///
/// The smallest real socket bar for this slice and the one that must be RED before any wiring exists:
/// today `read_outcome_candidate_document` does not parse, so the supervisor answers `protocol_error`
/// from its own `Json` refusal. "No such command" and "that command exists and refused you" are
/// different answers, and making that difference is the whole of what the route adds.
///
/// Raw bytes rather than a client method, for the reason the C5 and C6 route bars used them: at the
/// moment this is written there is no client method to call, and a test that cannot compile cannot be
/// RED by a named behaviour.
///
/// The request carries exactly ONE field, and this bar pins that shape at the wire seam before any
/// reader exists, because the shape is what a later guard can only narrow: a bare lower-case 64-hex
/// manifest digest -- the value the accepted frontend loader already sends
/// (`session.ts` `readOutcomeCandidateDocument`, which passes `held.manifestDigest.value`) -- with no
/// Mission id, no appendix request and no evidence field. A route that tolerated those would accept a
/// request the client never sends and give a future caller somewhere to put a fact this command must
/// not take from it.
///
/// An ABSENT digest is used on purpose. The session is locked, so the account refuses before any
/// store read, and this bar therefore says nothing about whether a candidate exists -- which is
/// correct, because a locked caller must not be able to learn that either.
#[test]
fn the_candidate_document_read_is_known_to_the_wire_and_authenticated() {
    let runtime = TestRuntime::new("c7route");
    let server = runtime.server();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    // The token is read from its file and never printed: no assertion message below carries it.
    let token = fs::read_to_string(&runtime.token).unwrap();
    // A syntactically legal digest this store does not hold. Written out rather than derived, so the
    // bar does not depend on a helper that could change what it sends.
    let absent_digest = "00000000000000000000000000000000000000000000000000000000000000ab";
    assert_eq!(
        absent_digest.len(),
        64,
        "C7T.ROUTE.THE_REQUEST_CARRIES_A_64_HEX_DIGEST"
    );

    let ask = |command: serde_json::Value| -> serde_json::Value {
        let body = serde_json::to_vec(&json!({
            "protocol": 1,
            "token": token,
            "command": command,
        }))
        .unwrap();
        let mut stream = UnixStream::connect(&runtime.socket).unwrap();
        stream.write_all(&body).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        serde_json::from_slice(&answer).unwrap()
    };

    let answered = ask(json!({
        "name": "read_outcome_candidate_document",
        "manifest_digest": absent_digest,
    }));
    assert_eq!(
        answered
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str),
        Some("not_authenticated"),
        "C7T.ROUTE.IS_A_KNOWN_COMMAND"
    );
    assert_eq!(
        answered.get("result"),
        Some(&serde_json::Value::Null),
        "C7T.ROUTE.A_REFUSAL_CARRIES_NO_RESULT"
    );

    // The one field is REQUIRED: there is no default digest, and an omitted key is a malformed
    // envelope rather than a read of something unnamed.
    let without = ask(json!({ "name": "read_outcome_candidate_document" }));
    assert_eq!(
        without
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str),
        Some("protocol_error"),
        "C7T.ROUTE.THE_DIGEST_IS_REQUIRED"
    );

    // And the request is CLOSED. Each of these three is a fact the command must read from the
    // verified records instead, so each is refused at the envelope rather than ignored.
    for extra in [
        json!({
            "name": "read_outcome_candidate_document",
            "manifest_digest": absent_digest,
            "mission_id": Uuid::new_v4(),
        }),
        json!({
            "name": "read_outcome_candidate_document",
            "manifest_digest": absent_digest,
            "appendix_request": "policy_default",
        }),
        json!({
            "name": "read_outcome_candidate_document",
            "manifest_digest": absent_digest,
            "candidate_content_base64": "aGk=",
        }),
    ] {
        let label = extra
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| *key != "name" && *key != "manifest_digest")
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let widened = ask(extra);
        assert_eq!(
            widened
                .pointer("/error/code")
                .and_then(serde_json::Value::as_str),
            Some("protocol_error"),
            "C7T.ROUTE.THE_REQUEST_IS_CLOSED: {label}"
        );
    }

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
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

/// B15 F1 over the real socket: an unclassified Mission cannot be completed, and classifying it
/// makes the very same request record.
///
/// The ordinary command, the ordinary client method, the ordinary code -- no raw bytes needed,
/// because the guard is reached through the same path an Operator uses.
#[test]
fn classification_then_completion_is_required_over_the_real_socket() {
    let runtime = TestRuntime::new("b15order");
    let address = serve_ollama_inspections(2);
    let server = runtime.server_with_model_endpoint(LocalModelEndpoint::new(address).unwrap());
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running));
    let client = LocalSupervisorClient::connect(&runtime.socket, &runtime.token).unwrap();
    let fixture = prepare_c6_fixture(&client);

    // A SECOND Mission on the same fixture, deliberately left unclassified.
    let mission_id = admit_another_mission(&client, &fixture, "Ship a second reviewed feature");
    let request = beacond::CompletionIntentRequest {
        mission_id,
        accountable_resident_id: fixture.resident_id,
        disposition: "concluded".to_owned(),
        condition_results: vec![beacond::ConditionResultDto {
            ordinal: 0,
            outcome: "met".to_owned(),
            support: Vec::new(),
            note: None,
        }],
        delivered_artifacts: Vec::new(),
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        spend: beacond::SpendDto::NotMeasured("no meter on this Mission".to_owned()),
        summary: "Shipped it.".to_owned(),
    };
    let preview = client
        .preview_mission_completion(request.clone())
        .unwrap_or_else(|error| panic!("B15.ORDER.THE_REVIEW_IS_STILL_A_READ: {error}"));

    let refusal = client
        .record_mission_completion(preview.request.clone(), preview.intent_sha256.clone())
        .expect_err("B15.ORDER.REQUIRED_BEFORE_WRITE: the socket must refuse an unclassified cut");
    assert!(
        matches!(
            &refusal,
            LocalTransportError::Remote { code, .. }
                if *code == LocalErrorCode::CompletionClassificationRequired
        ),
        "B15.ORDER.REQUIRED_BEFORE_WRITE: {refusal:?}"
    );

    // Classify, and the SAME request records: the guard is an order, not a block.
    client
        .classify_mission_impact(ClassificationIntentRequest {
            mission_id,
            impact: "ordinary".to_owned(),
            reason: "One reviewed file, fully reversible.".to_owned(),
            expected_policy_revision: Some(fixture.policy_revision),
            supersedes: SupersedesDto::None,
        })
        .unwrap_or_else(|error| panic!("B15.ORDER.PROVED_RECORD_PERMITS: {error}"));
    client
        .record_mission_completion(preview.request, preview.intent_sha256)
        .unwrap_or_else(|error| panic!("B15.ORDER.PROVED_RECORD_PERMITS: {error}"));

    running.store(false, Ordering::Release);
    UnixStream::connect(&runtime.socket).ok();
    thread.join().unwrap().unwrap();
}
