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
/// Sorts before `MODEL_NAME` under this pass's `ORDER BY ... model_name`, which is the point.
const OTHER_MODEL_NAME: &str = "aaa-model:1b";
const OTHER_MODEL_DIGEST: &str = "11130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

fn two_model_inspection_responses() -> Vec<(&'static str, String)> {
    vec![
        ("GET /api/tags HTTP/1.1\r\n", two_model_tags_response()),
        (
            "POST /api/show HTTP/1.1\r\n",
            json!({
                "capabilities": ["completion", "tools", "thinking"],
                "modified_at": "2026-08-14T14:38:20.13373148-04:00"
            })
            .to_string(),
        ),
        ("GET /api/tags HTTP/1.1\r\n", two_model_tags_response()),
    ]
}

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

fn model_entry(name: &str, digest: &str) -> serde_json::Value {
    json!({
        "name": name,
        "model": name,
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
    })
}

/// Both models in one listing, so a fixture can activate two and give a startup bar two rows.
fn two_model_tags_response() -> String {
    json!({
        "models": [
            model_entry(OTHER_MODEL_NAME, OTHER_MODEL_DIGEST),
            model_entry(MODEL_NAME, MODEL_DIGEST),
        ]
    })
    .to_string()
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
    // R44: BOTH ENDS. Tampering only the last-sorting decision made "names the failing
    // row" and "names the last row" the same assertion; `rows.last()` survived it.
    for (end, order) in [("last", "DESC"), ("first", "ASC")] {
        let store = TestStore::new(&format!("missing-event-{end}"));
        let accounts = enrolled_accounts(&store);
        let mut responses = two_model_inspection_responses();
        responses.extend(two_model_inspection_responses());
        responses.extend(two_model_inspection_responses());
        responses.extend(two_model_inspection_responses());
        let (address, server) = serve_ollama(responses);
        let service = LocalModelActivationService::new(
            accounts,
            store.workspace_id,
            LocalModelEndpoint::new(address).unwrap(),
        );
        let other = service
            .inspect(OTHER_MODEL_NAME, OTHER_MODEL_DIGEST)
            .unwrap();
        service
            .activate(other, vec![ProviderFeature::Completion])
            .unwrap();
        let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
        let activation = service
            .activate(review, vec![ProviderFeature::Completion])
            .unwrap();
        server.join().unwrap();
        drop(service);

        // R39.3 (kimi's HOLD): TWO real decisions, and the tampered one is NOT the row this pass
        // reads first. It enumerates `ORDER BY workspace_id, provider, endpoint, model_name,
        // sequence`, and `aaa-model:1b` sorts before `qwen3.8:27b`, so the honest second activation
        // keeps its Event and the FIRST-sorting one is not the failure. With a single decision a
        // mutant naming `rows[0]` was indistinguishable from naming the failing row.
        let connection = Connection::open(&store.path).unwrap();
        // R44: BOTH ENDS -- the target is read back from the pass's own ORDER BY, one end per run.
        let target: String = connection
            .query_row(
                &format!(
                    "SELECT id FROM local_model_activation_decisions
                 ORDER BY workspace_id {order}, provider {order}, endpoint {order},
                          model_name {order}, sequence {order} LIMIT 1"
                ),
                [],
                |row| row.get(0),
            )
            .unwrap();
        if end == "last" {
            assert_eq!(
                target,
                activation.id.to_string(),
                "{end}: the qwen activation sorts last"
            );
        } else {
            assert_ne!(
                target,
                activation.id.to_string(),
                "{end}: the aaa activation sorts first"
            );
        }
        let target_id = Uuid::parse_str(&target).unwrap();
        let removed = connection
            .execute(
                "DELETE FROM events WHERE kind = 'local_model.activated'
             AND payload_json LIKE '%' || ?1 || '%'",
                rusqlite::params![target],
            )
            .unwrap();
        assert_eq!(removed, 1, "the plant removed the wrong number of Events");
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        // The variant alone let three mutants through: the row it names and the fault NAME it ends in
        // are what R28.1/R28.4 buy, so both are asserted here rather than read.
        let SupervisorError::LocalModelActivationStateMismatch {
            activation_id: named,
            ..
        } = &error
        else {
            panic!("expected a named activation refusal, got {error:?}");
        };
        assert_eq!(*named, target_id, "{end}: the wrong activation was named");
        let line = error.to_string();
        assert!(
            line.contains(&target),
            "the line does not name the activation: {line}"
        );
        let tail = line.rsplit(": ").next().unwrap();
        assert_eq!(tail, "InvalidStore", "wrong fault name: {line}");
        // blue's HOLD: the carried reader's OWN sentence never reaches the line (R28.4).
        assert!(
            !line.contains("local model activation state is invalid"),
            "the reader's own sentence reached the line: {line}"
        );
    }
}

/// R45.1: activation history is ordered by a service-generated positive sequence. A stored
/// negative key must be refused and named, not accepted as an alternative history.
#[test]
fn sol1_startup_refuses_minimum_activation_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let mut responses = two_model_inspection_responses();
    responses.extend(two_model_inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let active = service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'local_model_activation_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER local_model_activation_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE local_model_activation_decisions SET sequence = ?1 WHERE id = ?2",
                rusqlite::params![i64::MIN, active.id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MIN sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted activation sequence i64::MIN as Ready");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the activation ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a local-model activation decision's ordering key is not readable"
    );
    assert!(raw.contains(&active.id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, i64::MIN);
}

#[test]
fn astra_startup_refuses_zero_activation_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let mut responses = two_model_inspection_responses();
    responses.extend(two_model_inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let active = service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'local_model_activation_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER local_model_activation_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE local_model_activation_decisions SET sequence = ?1 WHERE id = ?2",
                rusqlite::params![0_i64, active.id.to_string()],
            )
            .unwrap(),
        1,
        "the 0_i64 sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("R45.ACTIVATION.ZERO_REFUSED");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the activation ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a local-model activation decision's ordering key is not readable"
    );
    assert!(raw.contains(&active.id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, 0, "R45.ACTIVATION.ZERO_KEY");
    assert!(
        error.to_string().contains("sequence 0"),
        "R45.ACTIVATION.ZERO_LINE_KEY"
    );
}

#[test]
fn astra_startup_accepts_maximum_activation_sequence() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let mut responses = two_model_inspection_responses();
    responses.extend(two_model_inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let active = service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'local_model_activation_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER local_model_activation_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE local_model_activation_decisions SET sequence = ?1 WHERE id = ?2",
                rusqlite::params![i64::MAX, active.id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MAX sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let result = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        result.is_ok(),
        "R45.ACTIVATION.MAX_POSITIVE_ORDER_KEY: {result:?}"
    );
}

#[test]
fn astra_minimum_activation_sequence_outward_key_is_exact() {
    let store = TestStore::new("sol1-min-sequence");
    let accounts = enrolled_accounts(&store);
    let mut responses = two_model_inspection_responses();
    responses.extend(two_model_inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let active = service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'local_model_activation_decisions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER local_model_activation_decisions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE local_model_activation_decisions SET sequence = ?1 WHERE id = ?2",
                rusqlite::params![i64::MIN, active.id.to_string()],
            )
            .unwrap(),
        1,
        "the i64::MIN sequence did not plant"
    );
    connection.execute_batch(&trigger).unwrap();
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted activation sequence i64::MIN as Ready");
    let SupervisorError::StoredIntegerKeyUnreadable {
        what,
        raw,
        key_name,
        key,
    } = &error
    else {
        panic!("the activation ordering key was refused under the wrong name: {error:?}");
    };
    assert_eq!(
        *what,
        "a local-model activation decision's ordering key is not readable"
    );
    assert!(raw.contains(&active.id.to_string()));
    assert_eq!(*key_name, "sequence");
    assert_eq!(*key, i64::MIN);
    let line = error.to_string();
    eprintln!("OUTWARD {line}");
    assert!(
        line.contains(&i64::MIN.to_string()),
        "R45.ACTIVATION.MIN_LINE_KEY_EXACT: {line}"
    );
}

/// sol1 cold-read probe: a 36-character non-UUID activation id passes the schema and reaches the
/// pass. The refusal must still NAME the owner rather than collapsing to an unnamed unreadable row.
///
/// R42 amended what this bar requires, exactly as it amended the two `sol1_*` bars in `lib.rs`:
/// "the line contains an escaped fragment of the stored id" is a requirement to DISCLOSE a
/// substring of store text. Escaping and bounding limited that disclosure; they did not remove it.
/// What is kept is the invariant that always mattered -- one line, bounded, naming which owner
/// could not read its row -- plus the stored text absent from the line and still whole in `Debug`,
/// where this suite's sibling bars already read it.
#[test]
fn sol1_activation_with_unreadable_identity_is_still_named() {
    let store = TestStore::new("sol1-unreadable-id");
    let accounts = enrolled_accounts(&store);
    let mut responses = two_model_inspection_responses();
    responses.extend(two_model_inspection_responses());
    let (address, server) = serve_ollama(responses);
    let service = LocalModelActivationService::new(
        accounts,
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = service.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let active = service
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    server.join().unwrap();
    drop(service);

    let malformed = format!("{}\r\u{1b}[2J", "x".repeat(31));
    assert_eq!(malformed.chars().count(), 36);
    let connection = Connection::open(&store.path).unwrap();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('local_model_activation_decisions')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let names = columns.join(", ");
    let values = columns
        .iter()
        .map(|column| {
            if column == "sequence" {
                "NULL".to_owned()
            } else if column == "id" {
                "?1".to_owned()
            } else {
                column.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let inserted = connection
        .execute(
            &format!(
                "INSERT INTO local_model_activation_decisions ({names})
                 SELECT {values} FROM local_model_activation_decisions WHERE id = ?2"
            ),
            rusqlite::params![malformed, active.id.to_string()],
        )
        .unwrap();
    assert_eq!(inserted, 1, "the malformed activation row did not plant");
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("the malformed activation id reopened Ready");
    let line = error.to_string();
    assert!(
        !line.contains('\n') && !line.contains('\r') && !line.contains('\u{1b}'),
        "R42S2.IDENTITY.Activation.ONE_LINE: {line:?}"
    );
    assert!(
        !line.contains(&"x".repeat(31)),
        "R42S2.IDENTITY.Activation.NO_NEEDLE: {line:?}"
    );
    assert_eq!(
        line, "a local-model activation row's identity is not readable: StoredIdentityUnreadable",
        "R42S2.IDENTITY.Activation.PROJECTION"
    );
    assert!(
        format!("{error:?}").contains(&"x".repeat(31)),
        "R42S2.IDENTITY.Activation.DEBUG_RETAINS"
    );
}
