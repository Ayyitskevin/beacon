use std::fs;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use beacond::{
    DiscoveryStatus, LocalModelEndpoint, ModelActivation, ModelProvider, OperatorAccountService,
    OperatorSessionPhase, ResourceDiscoveryError, ResourceDiscoveryService, SecretString,
    StartupConfig, Supervisor,
};
use serde_json::{Value, json};
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-resource-discovery-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
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

fn enrolled_accounts(label: &str) -> (TestStore, Arc<OperatorAccountService>) {
    let store = TestStore::new(label);
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
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
    (store, accounts)
}

fn serve_response(status: &'static str, body: impl Into<String>) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let body = body.into();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let size = stream.read(&mut request).unwrap();
        let request = std::str::from_utf8(&request[..size]).unwrap();
        assert!(request.starts_with("GET /api/tags HTTP/1.1\r\n"));
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    address
}

fn serve_json(body: impl Into<String>) -> SocketAddr {
    serve_response("200 OK", body)
}

fn discover(
    accounts: &Arc<OperatorAccountService>,
    status: &'static str,
    body: impl Into<String>,
) -> beacond::LocalModelDiscoveryReport {
    let endpoint = LocalModelEndpoint::new(serve_response(status, body)).unwrap();
    ResourceDiscoveryService::new(Arc::clone(accounts), endpoint)
        .discover_local_models()
        .unwrap()
}

fn model(name: &str) -> Value {
    json!({
        "name": name,
        "model": name,
        "modified_at": "2026-08-14T14:38:20.13373148-04:00",
        "size": 17_741_872_154_u64,
        "digest": "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
        "details": {
            "format": "gguf",
            "family": "qwen35",
            "families": ["qwen35"],
            "parameter_size": "27.3B",
            "quantization_level": "Q4_K_M"
        }
    })
}

#[test]
fn authenticated_discovery_reads_and_normalizes_the_real_ollama_http_shape() {
    let (_store, accounts) = enrolled_accounts("available");
    let address = serve_json(
        r#"{
          "models": [{
            "name": "qwen3.8:27b",
            "model": "qwen3.8:27b",
            "modified_at": "2026-08-14T14:38:20.13373148-04:00",
            "size": 17741872154,
            "digest": "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
            "details": {
              "format": "gguf",
              "family": "qwen35",
              "families": ["qwen35"],
              "parameter_size": "27.3B",
              "quantization_level": "Q4_K_M"
            },
            "capabilities": ["completion", "tools", "thinking", "vision"]
          }],
          "future_field": true
        }"#,
    );
    let endpoint = LocalModelEndpoint::new(address).unwrap();
    let discovery = ResourceDiscoveryService::new(accounts, endpoint);

    let report = discovery.discover_local_models().unwrap();

    assert_eq!(report.status, DiscoveryStatus::Available);
    assert_eq!(report.endpoint, format!("http://{address}"));
    assert_eq!(report.models.len(), 1);
    let model = &report.models[0];
    assert_eq!(model.provider, ModelProvider::Ollama);
    assert_eq!(model.activation, ModelActivation::Inactive);
    assert_eq!(model.name, "qwen3.8:27b");
    assert_eq!(model.size_bytes, 17_741_872_154);
    assert_eq!(model.details.format, "gguf");
    assert_eq!(model.details.family, "qwen35");
    assert_eq!(model.details.parameter_size, "27.3B");
    assert_eq!(model.details.quantization_level, "Q4_K_M");
}

#[test]
fn endpoint_and_session_boundaries_precede_discovery() {
    assert!(matches!(
        LocalModelEndpoint::new(SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)),
            11434
        )),
        Err(ResourceDiscoveryError::EndpointNotLoopback)
    ));

    let (_store, accounts) = enrolled_accounts("locked");
    accounts.lock().unwrap();
    let unreachable = LocalModelEndpoint::new(SocketAddr::from((Ipv4Addr::LOCALHOST, 9))).unwrap();
    let discovery = ResourceDiscoveryService::new(accounts, unreachable);
    assert!(matches!(
        discovery.discover_local_models(),
        Err(ResourceDiscoveryError::Account(_))
    ));
}

#[test]
fn a_session_that_locks_during_http_withholds_the_discovery_result() {
    let (_store, accounts) = enrolled_accounts("lock-during-request");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0_u8; 1024];
        let request_size = stream.read(&mut request).unwrap();
        assert!(request_size > 0);
        request_seen_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        let body = json!({"models": [model("qwen3.8:27b")]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    });
    let discovery = ResourceDiscoveryService::new(
        Arc::clone(&accounts),
        LocalModelEndpoint::new(address).unwrap(),
    );
    let request = thread::spawn(move || discovery.discover_local_models());
    request_seen_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("discovery did not reach the loopback fixture");

    let accounts_for_lock = Arc::clone(&accounts);
    let (locked_tx, locked_rx) = mpsc::channel();
    let locking = thread::spawn(move || locked_tx.send(accounts_for_lock.lock()).unwrap());
    // The property is ordering, not speed: the fixture holds the HTTP response until
    // `release_tx` fires below, so a lock that completes at all before that point cannot
    // have waited on discovery. The budget only has to outlast a durable `lock()` —
    // it takes the state mutex and writes a security event to SQLite — on a loaded CI
    // runner. 250 ms did not (run 33274424854, 2026-08-29): the same commit passed in
    // the parallel run, and the panic read as "held the mutex" when nothing was held.
    let lock_result = locked_rx.recv_timeout(Duration::from_secs(5));
    release_tx.send(()).unwrap();
    server.join().unwrap();
    locking.join().unwrap();

    assert!(
        lock_result
            .expect("discovery held the account mutex during HTTP")
            .is_ok(),
        "session lock failed"
    );
    assert!(matches!(
        request.join().unwrap(),
        Err(ResourceDiscoveryError::Account(_))
    ));
}

#[test]
fn discovery_fails_closed_across_the_real_http_response_matrix() {
    let (_store, accounts) = enrolled_accounts("matrix");

    let empty = discover(&accounts, "200 OK", json!({"models": []}).to_string());
    assert_eq!(empty.status, DiscoveryStatus::Available);
    assert!(empty.models.is_empty());

    let sorted = discover(
        &accounts,
        "200 OK",
        json!({"models": [model("zeta:latest"), model("alpha:latest")]}).to_string(),
    );
    assert_eq!(sorted.status, DiscoveryStatus::Available);
    assert_eq!(
        sorted
            .models
            .iter()
            .map(|model| model.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha:latest", "zeta:latest"]
    );

    let same_digest_aliases = discover(
        &accounts,
        "200 OK",
        json!({"models": [model("alpha:latest"), model("alpha:v1")]}).to_string(),
    );
    assert_eq!(same_digest_aliases.status, DiscoveryStatus::Available);
    assert_eq!(same_digest_aliases.models.len(), 2);

    let mut future_model = model("future:latest");
    future_model["capabilities"] = json!(["completion"]);
    future_model["future_model_field"] = json!({"safe_to_ignore": true});
    let future_fields = discover(
        &accounts,
        "200 OK",
        json!({"models": [future_model], "future_top_level": 42}).to_string(),
    );
    assert_eq!(future_fields.status, DiscoveryStatus::Available);

    let invalid_documents = [
        ("non-JSON", "not json".to_owned()),
        ("missing models", json!({}).to_string()),
        ("null models", json!({"models": null}).to_string()),
        (
            "duplicate names",
            json!({"models": [model("dup:latest"), model("dup:latest")]}).to_string(),
        ),
        (
            "different name and model identity",
            {
                let mut value = model("one:latest");
                value["model"] = json!("two:latest");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank name",
            {
                let mut value = model("valid:latest");
                value["name"] = json!("");
                value["model"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "padded name",
            {
                let mut value = model("valid:latest");
                value["name"] = json!(" valid:latest ");
                value["model"] = json!(" valid:latest ");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "control character name",
            {
                let mut value = model("valid:latest");
                value["name"] = json!("bad\u{0000}name");
                value["model"] = json!("bad\u{0000}name");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "oversized name",
            {
                let name = "x".repeat(257);
                let mut value = model("valid:latest");
                value["name"] = json!(name);
                value["model"] = value["name"].clone();
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "zero size",
            {
                let mut value = model("valid:latest");
                value["size"] = json!(0);
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "short digest",
            {
                let mut value = model("valid:latest");
                value["digest"] = json!("abc123");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "uppercase digest",
            {
                let mut value = model("valid:latest");
                value["digest"] = json!(
                    "22130167C4C20E20C7B71454612966CA8E8171E9B3CC8AB6CE8AA6CBFEC79643"
                );
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "missing details",
            {
                let mut value = model("valid:latest");
                value.as_object_mut().unwrap().remove("details");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank format",
            {
                let mut value = model("valid:latest");
                value["details"]["format"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank family",
            {
                let mut value = model("valid:latest");
                value["details"]["family"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank parameter size",
            {
                let mut value = model("valid:latest");
                value["details"]["parameter_size"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank quantization",
            {
                let mut value = model("valid:latest");
                value["details"]["quantization_level"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "blank modified time",
            {
                let mut value = model("valid:latest");
                value["modified_at"] = json!("");
                json!({"models": [value]}).to_string()
            },
        ),
        (
            "too many models",
            json!({"models": (0..513).map(|index| model(&format!("model-{index}"))).collect::<Vec<_>>()}).to_string(),
        ),
    ];
    for (label, body) in invalid_documents {
        let report = discover(&accounts, "200 OK", body);
        assert_eq!(
            report.status,
            DiscoveryStatus::InvalidResponse,
            "{label} must fail closed"
        );
        assert!(report.models.is_empty(), "{label} leaked partial models");
    }

    for status in ["302 Found", "401 Unauthorized", "500 Internal Server Error"] {
        let report = discover(&accounts, status, json!({"models": []}).to_string());
        assert_eq!(report.status, DiscoveryStatus::Unavailable, "{status}");
        assert!(report.models.is_empty());
    }

    let oversized = discover(&accounts, "200 OK", " ".repeat(1024 * 1024 + 1));
    assert_eq!(oversized.status, DiscoveryStatus::InvalidResponse);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let unavailable_address = listener.local_addr().unwrap();
    drop(listener);
    let unavailable = ResourceDiscoveryService::new(
        Arc::clone(&accounts),
        LocalModelEndpoint::new(unavailable_address).unwrap(),
    )
    .discover_local_models()
    .unwrap();
    assert_eq!(unavailable.status, DiscoveryStatus::Unavailable);
    assert!(unavailable.models.is_empty());
}
