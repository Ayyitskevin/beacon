use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Barrier;
use std::thread::{self, JoinHandle};

use beacond::{
    LocalModelActivationError, LocalModelActivationService, LocalModelActivationState,
    LocalModelEndpoint, ModelProvider, OperatorAccountService, OperatorSessionPhase,
    ProviderFeature, SecretString, StartupConfig, Supervisor, SupervisorError,
};
use rusqlite::Connection;
use serde_json::json;
use uuid::Uuid;

const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

struct TestStore {
    path: PathBuf,
    workspace_id: Uuid,
}

impl TestStore {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "beacond-local-model-activation-{label}-{}.sqlite3",
            Uuid::new_v4()
        ));
        let report = Supervisor::bootstrap(StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();
        Self {
            path,
            workspace_id: report.workspace.id,
        }
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = fs::remove_file(format!("{}{suffix}", self.path.display()));
        }
    }
}

fn enrolled_accounts(store: &TestStore) -> Arc<OperatorAccountService> {
    let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
    let recovery = accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert!(!recovery.expose().is_empty());
    assert_eq!(
        accounts.complete_operator_setup(true).unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    accounts
}

fn tags_response(digest: &str) -> String {
    json!({
        "models": [{
            "name": MODEL_NAME,
            "model": MODEL_NAME,
            "modified_at": "2026-08-14T14:38:20.13373148-04:00",
            "size": 17_741_872_154_u64,
            "digest": digest,
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

fn serve_ollama(responses: Vec<(&'static str, String)>) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for (expected_request_line, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            assert!(
                request.starts_with(expected_request_line),
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
    });
    (address, handle)
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

fn inspection_responses() -> Vec<(&'static str, String)> {
    vec![
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        (
            "POST /api/show HTTP/1.1\r\n",
            json!({
                "capabilities": ["completion", "tools", "thinking"],
                "modified_at": "2026-08-14T14:38:20.13373148-04:00"
            })
            .to_string(),
        ),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
    ]
}

fn serve_ollama_requests(count: usize) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_http_request(&mut stream);
            let body = if request.starts_with("GET /api/tags HTTP/1.1\r\n") {
                tags_response(MODEL_DIGEST)
            } else {
                assert!(request.starts_with("POST /api/show HTTP/1.1\r\n"));
                json!({ "capabilities": ["completion", "tools"] }).to_string()
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    (address, handle)
}

#[test]
fn owner_inspects_an_exact_model_without_activating_or_invoking_it() {
    let store = TestStore::new("inspect");
    let accounts = enrolled_accounts(&store);
    let show = json!({
        "capabilities": ["tools", "future_audio", "completion", "tools"],
        "modified_at": "2026-08-14T14:38:20.13373148-04:00",
        "details": {
            "format": "gguf",
            "family": "qwen35",
            "parameter_size": "27.3B",
            "quantization_level": "Q4_K_M"
        }
    })
    .to_string();
    let (address, server) = serve_ollama(vec![
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        ("POST /api/show HTTP/1.1\r\n", show),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
    ]);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );

    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    server.join().unwrap();

    assert_eq!(review.binding.provider, ModelProvider::Ollama);
    assert_eq!(review.binding.endpoint, format!("http://{address}"));
    assert_eq!(review.binding.model_name, MODEL_NAME);
    assert_eq!(review.binding.model_digest, MODEL_DIGEST);
    assert_eq!(
        review.provider_features,
        [ProviderFeature::Completion, ProviderFeature::Tools]
    );
    assert_eq!(review.unrecognized_features, ["future_audio"]);
    assert_eq!(review.binding.inspection_digest.len(), 64);
    assert!(
        review
            .binding
            .inspection_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert!(service.list().unwrap().is_empty());
}

#[test]
fn owner_activates_a_reviewed_subset_and_can_deactivate_without_the_provider() {
    let store = TestStore::new("activation-lifecycle");
    let accounts = enrolled_accounts(&store);
    let mut responses = inspection_responses();
    responses.extend(inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();

    let active = service
        .activate(
            review,
            vec![ProviderFeature::Completion, ProviderFeature::Tools],
        )
        .unwrap();
    server.join().unwrap();

    assert_eq!(active.state, LocalModelActivationState::Active);
    assert_eq!(
        active.approved_features,
        [ProviderFeature::Completion, ProviderFeature::Tools]
    );
    assert_eq!(active.predecessor_id, None);
    let listed = service.list().unwrap();
    assert_eq!(listed.as_slice(), std::slice::from_ref(&active));

    let connection = Connection::open(&store.path).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE local_model_activation_decisions
                 SET approved_features_json = '[\"tools\"]'
                 WHERE id = ?1",
                [active.id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM local_model_activation_decisions WHERE id = ?1",
                [active.id.to_string()],
            )
            .is_err()
    );
    drop(connection);

    let inactive = service.deactivate(active.id).unwrap();

    assert_eq!(inactive.state, LocalModelActivationState::Inactive);
    assert_eq!(inactive.predecessor_id, Some(active.id));
    assert_eq!(inactive.binding, active.binding);
    assert_eq!(inactive.approved_features, active.approved_features);
    assert_eq!(service.list().unwrap(), [inactive]);
    assert!(matches!(
        service.deactivate(active.id),
        Err(LocalModelActivationError::StaleDecision)
    ));
}

#[test]
fn inspection_fails_closed_when_the_installed_digest_changes_mid_review() {
    let store = TestStore::new("drift");
    let accounts = enrolled_accounts(&store);
    let changed_digest = "33130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";
    let (address, server) = serve_ollama(vec![
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        (
            "POST /api/show HTTP/1.1\r\n",
            json!({ "capabilities": ["completion"] }).to_string(),
        ),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(changed_digest)),
    ]);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );

    assert!(matches!(
        service.inspect(MODEL_NAME, MODEL_DIGEST),
        Err(LocalModelActivationError::ModelDrift)
    ));
    server.join().unwrap();
    assert!(service.list().unwrap().is_empty());
}

#[test]
fn concurrent_activation_of_one_resource_records_exactly_one_active_decision() {
    let store = TestStore::new("concurrent");
    let accounts = enrolled_accounts(&store);
    let (address, server) = serve_ollama_requests(9);
    let service = Arc::new(LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    ));
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let service = Arc::clone(&service);
            let review = review.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                service.activate(review, vec![ProviderFeature::Completion])
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    server.join().unwrap();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(LocalModelActivationError::AlreadyActive)))
            .count(),
        1
    );
    let current = service.list().unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].state, LocalModelActivationState::Active);
}

#[test]
fn activation_refuses_a_review_when_provider_features_change() {
    let store = TestStore::new("feature-drift");
    let accounts = enrolled_accounts(&store);
    let show_one = json!({ "capabilities": ["completion"] }).to_string();
    let show_two = json!({ "capabilities": ["completion", "tools"] }).to_string();
    let (address, server) = serve_ollama(vec![
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        ("POST /api/show HTTP/1.1\r\n", show_one),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
        ("POST /api/show HTTP/1.1\r\n", show_two),
        ("GET /api/tags HTTP/1.1\r\n", tags_response(MODEL_DIGEST)),
    ]);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();

    assert!(matches!(
        service.activate(review, vec![ProviderFeature::Completion]),
        Err(LocalModelActivationError::ModelDrift)
    ));
    server.join().unwrap();
    assert!(service.list().unwrap().is_empty());
}

#[test]
fn restart_refuses_an_activation_decision_without_its_exact_event() {
    let store = TestStore::new("missing-event");
    let accounts = enrolled_accounts(&store);
    let mut responses = inspection_responses();
    responses.extend(inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "DELETE FROM events WHERE kind = 'local_model.activated'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(SupervisorError::LocalModelActivationStateMismatch { .. })
    ));
}
