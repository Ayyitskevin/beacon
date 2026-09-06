use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

use beacond::{
    BeaconCapability, BudgetBoundary, LocalModelActivation, LocalModelActivationService,
    LocalModelEndpoint, OperatorAccountService, OperatorSessionPhase, ProviderFeature,
    ResidentConfigurationError, ResidentConfigurationService, ResidentConfigurationState,
    ResidentLifecycleService, SecretString, StarterOrganizationService, StarterResidentDraft,
    StarterRole, StartupConfig, Supervisor, TeamIneligibilityReason,
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
            "beacond-resident-configuration-{label}-{}.sqlite3",
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
    accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert_eq!(
        accounts.complete_operator_setup(true).unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    accounts
}

fn admit_lead(store: &TestStore, accounts: &Arc<OperatorAccountService>) -> Uuid {
    StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id)
        .accept(
            1,
            vec![StarterResidentDraft {
                role: StarterRole::Lead,
                display_name: Some("Northstar".to_owned()),
            }],
        )
        .unwrap()
        .residents[0]
        .id
}

/// Two Residents in one Workspace, so a startup bar can tell "the failing row" from "row 0".
fn admit_two(store: &TestStore, accounts: &Arc<OperatorAccountService>) -> Vec<Uuid> {
    StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id)
        .accept(
            1,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: Some("Northstar".to_owned()),
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: Some("Southstar".to_owned()),
                },
            ],
        )
        .unwrap()
        .residents
        .iter()
        .map(|resident| resident.id)
        .collect()
}

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

fn serve_ollama_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..count {
            for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(
                    request.starts_with(expected),
                    "unexpected request: {request}"
                );
                let body = if expected.starts_with("POST") {
                    json!({ "capabilities": ["completion", "tools"] }).to_string()
                } else {
                    tags_response()
                };
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
    (address, handle)
}

fn read_http_request(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let size = stream.read(&mut chunk).unwrap();
        assert!(size > 0);
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

fn activate_model(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
    approved_features: Vec<ProviderFeature>,
) -> LocalModelActivation {
    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(accounts),
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let activation = models.activate(review, approved_features).unwrap();
    ollama.join().unwrap();
    activation
}

#[test]
fn operator_configures_an_active_resident_for_future_team_eligibility_without_execution() {
    let store = TestStore::new("tracer");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .activate(resident_id, None)
        .unwrap();

    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(&accounts),
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let activation = models
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();

    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let configured = configurations
        .configure(
            resident_id,
            None,
            activation.id,
            vec![
                BeaconCapability::ArtifactEvaluate,
                BeaconCapability::WorkCreateMission,
                BeaconCapability::EvidenceDevelop,
            ],
        )
        .unwrap();

    assert_eq!(configured.state, ResidentConfigurationState::Configured);
    assert_eq!(
        configured.binding.as_ref().unwrap().activation_id,
        activation.id
    );
    assert_eq!(
        configured.binding.as_ref().unwrap().budget_boundary,
        BudgetBoundary::NoSpend
    );
    assert_eq!(
        configured.binding.as_ref().unwrap().capabilities,
        [
            BeaconCapability::WorkCreateMission,
            BeaconCapability::ArtifactEvaluate,
            BeaconCapability::EvidenceDevelop,
        ]
    );
    assert!(configured.eligible_for_team);
    assert!(configured.ineligibility_reasons.is_empty());
    assert_eq!(configurations.list().unwrap(), [configured]);
}

#[test]
fn lifecycle_changes_eligibility_without_rewriting_configuration() {
    let store = TestStore::new("lifecycle-independence");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);

    let configured = configurations
        .configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    assert!(!configured.eligible_for_team);
    assert_eq!(
        configured.ineligibility_reasons,
        [TeamIneligibilityReason::ResidentInactive]
    );

    let active = lifecycle.activate(resident_id, None).unwrap();
    let eligible = configurations.list().unwrap().remove(0);
    assert_eq!(eligible.id, configured.id);
    assert!(eligible.eligible_for_team);

    let suspended = lifecycle
        .suspend(resident_id, active.latest_decision_id)
        .unwrap();
    let ineligible = configurations.list().unwrap().remove(0);
    assert_eq!(ineligible.id, configured.id);
    assert_eq!(
        ineligible.ineligibility_reasons,
        [TeamIneligibilityReason::ResidentSuspended]
    );

    lifecycle
        .reactivate(resident_id, suspended.latest_decision_id)
        .unwrap();
    let eligible_again = configurations.list().unwrap().remove(0);
    assert_eq!(eligible_again.id, configured.id);
    assert!(eligible_again.eligible_for_team);
}

#[test]
fn clear_and_reconfigure_are_explicit_stale_safe_append_only_decisions() {
    let store = TestStore::new("clear-reconfigure");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .activate(resident_id, None)
        .unwrap();
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let configured = configurations
        .configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();

    let cleared = configurations.clear(resident_id, configured.id).unwrap();
    assert_eq!(cleared.state, ResidentConfigurationState::Cleared);
    assert_eq!(cleared.binding, None);
    assert_eq!(cleared.predecessor_id, Some(configured.id));
    assert_eq!(
        cleared.ineligibility_reasons,
        [TeamIneligibilityReason::ConfigurationCleared]
    );
    assert!(matches!(
        configurations.clear(resident_id, configured.id),
        Err(ResidentConfigurationError::StaleDecision)
    ));

    let reconfigured = configurations
        .configure(
            resident_id,
            Some(cleared.id),
            activation.id,
            vec![
                BeaconCapability::WorkCreateMission,
                BeaconCapability::WorkFormTeam,
            ],
        )
        .unwrap();
    assert_eq!(reconfigured.predecessor_id, Some(cleared.id));
    assert!(reconfigured.eligible_for_team);

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM resident_configuration_decisions",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind LIKE 'resident.configuration_%'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        3
    );
    assert!(
        connection
            .execute(
                "DELETE FROM resident_configuration_decisions WHERE id = ?1",
                [configured.id.to_string()],
            )
            .is_err()
    );
}

#[test]
fn invalid_targets_capabilities_and_locked_sessions_fail_without_writing() {
    let store = TestStore::new("refusals");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);

    assert!(matches!(
        configurations.configure(resident_id, None, activation.id, vec![]),
        Err(ResidentConfigurationError::InvalidCapabilities)
    ));
    assert!(matches!(
        configurations.configure(
            resident_id,
            None,
            activation.id,
            vec![
                BeaconCapability::WorkCreateMission,
                BeaconCapability::WorkCreateMission,
            ],
        ),
        Err(ResidentConfigurationError::InvalidCapabilities)
    ));
    assert!(matches!(
        configurations.configure(
            Uuid::new_v4(),
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        Err(ResidentConfigurationError::ResidentNotFound)
    ));
    assert!(matches!(
        configurations.configure(
            resident_id,
            None,
            Uuid::new_v4(),
            vec![BeaconCapability::WorkCreateMission],
        ),
        Err(ResidentConfigurationError::ModelActivationUnavailable)
    ));

    accounts.lock().unwrap();
    assert!(matches!(
        configurations.catalog(),
        Err(ResidentConfigurationError::Account(_))
    ));
    assert!(matches!(
        configurations.list(),
        Err(ResidentConfigurationError::Account(_))
    ));
    assert!(matches!(
        configurations.configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        Err(ResidentConfigurationError::Account(_))
    ));

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM resident_configuration_decisions",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn model_requires_approved_completion_before_it_can_configure_an_agent() {
    let store = TestStore::new("completion-required");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Tools]);
    let configurations = ResidentConfigurationService::new(accounts, store.workspace_id);

    assert!(matches!(
        configurations.configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        Err(ResidentConfigurationError::ModelMissingCompletion)
    ));
}

#[test]
fn retired_residents_refuse_new_configuration_decisions_but_keep_history_visible() {
    let store = TestStore::new("retired-resident");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let configured = configurations
        .configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .retire(resident_id, None)
        .unwrap();

    let historical = configurations.list().unwrap().remove(0);
    assert_eq!(historical.id, configured.id);
    assert_eq!(
        historical.ineligibility_reasons,
        [TeamIneligibilityReason::ResidentRetired]
    );
    assert!(matches!(
        configurations.configure(
            resident_id,
            Some(configured.id),
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        ),
        Err(ResidentConfigurationError::ResidentRetired)
    ));
    assert!(matches!(
        configurations.clear(resident_id, configured.id),
        Err(ResidentConfigurationError::ResidentRetired)
    ));

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM resident_configuration_decisions",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn deactivation_and_reactivation_never_resurrect_the_old_binding() {
    let store = TestStore::new("model-revocation");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .activate(resident_id, None)
        .unwrap();
    let (address, ollama) = serve_ollama_inspections(4);
    let models = LocalModelActivationService::new(
        Arc::clone(&accounts),
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let first_review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let first = models
        .activate(first_review, vec![ProviderFeature::Completion])
        .unwrap();
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let configured = configurations
        .configure(
            resident_id,
            None,
            first.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    assert!(configured.eligible_for_team);

    models.deactivate(first.id).unwrap();
    let revoked = configurations.list().unwrap().remove(0);
    assert_eq!(revoked.id, configured.id);
    assert_eq!(
        revoked.ineligibility_reasons,
        [TeamIneligibilityReason::ModelActivationUnavailable]
    );

    let second_review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let second = models
        .activate(second_review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();
    assert_ne!(second.id, first.id);
    let still_revoked = configurations.list().unwrap().remove(0);
    assert_eq!(still_revoked.binding.unwrap().activation_id, first.id);
    assert_eq!(
        still_revoked.ineligibility_reasons,
        [TeamIneligibilityReason::ModelActivationUnavailable]
    );

    let restored = configurations
        .configure(
            resident_id,
            Some(configured.id),
            second.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    assert!(restored.eligible_for_team);
    assert_eq!(restored.binding.unwrap().activation_id, second.id);
}

#[test]
fn concurrent_initial_configuration_has_one_winner() {
    let store = TestStore::new("concurrent");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let configurations = Arc::new(ResidentConfigurationService::new(
        accounts,
        store.workspace_id,
    ));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let configurations = Arc::clone(&configurations);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                configurations.configure(
                    resident_id,
                    None,
                    activation.id,
                    vec![BeaconCapability::WorkCreateMission],
                )
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ResidentConfigurationError::StaleDecision)))
            .count(),
        1
    );
}

#[test]
fn startup_refuses_missing_or_tampered_configuration_events() {
    for (label, tamper) in [
        (
            "missing-event",
            "DELETE FROM events WHERE kind = 'resident.configuration_set'
             AND payload_json LIKE '%' || ?1 || '%'",
        ),
        (
            "tampered-event",
            "UPDATE events SET payload_json = '{}'
             WHERE kind = 'resident.configuration_set'
               AND payload_json LIKE '%' || ?1 || '%'",
        ),
    ] {
        let store = TestStore::new(label);
        let accounts = enrolled_accounts(&store);
        // R39.3 (kimi's HOLD): TWO configured Residents, so the failing decision is not row 0.
        let resident_ids = admit_two(&store, &accounts);
        let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
        let configurations =
            ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
        let decisions: Vec<_> = resident_ids
            .iter()
            .map(|resident_id| {
                configurations
                    .configure(
                        *resident_id,
                        None,
                        activation.id,
                        vec![BeaconCapability::WorkCreateMission],
                    )
                    .unwrap()
            })
            .collect();
        drop(configurations);
        drop(accounts);

        let connection = Connection::open(&store.path).unwrap();
        // This pass enumerates `ORDER BY workspace_id, resident_id, sequence`; the tamper hits the
        // decision that sorts LAST, read back from the store rather than guessed.
        let last_id: String = connection
            .query_row(
                "SELECT id FROM resident_configuration_decisions
                 ORDER BY workspace_id, resident_id DESC, sequence DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("the fixture recorded two decisions");
        let configuration = decisions
            .iter()
            .find(|decision| decision.id.to_string() == last_id)
            .expect("the last-sorting decision is one the fixture made");
        let touched = connection
            .execute(tamper, rusqlite::params![last_id])
            .unwrap();
        assert_eq!(touched, 1, "{label}: the plant touched the wrong row count");
        drop(connection);

        let error = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .unwrap_err();
        // Which guard reaches this depends on the tamper: the log walk runs first, so an Event
        // tamper is refused there. When the state pass IS the refuser, it must name the decision
        // and end in a fault NAME -- the variant alone let three mutants through.
        match &error {
            beacond::SupervisorError::ResidentConfigurationStateMismatch {
                decision_id: named,
                ..
            } => {
                assert_eq!(
                    named,
                    &configuration.id.to_string(),
                    "{label}: the wrong decision was named"
                );
                let line = error.to_string();
                assert!(
                    line.contains(&configuration.id.to_string()),
                    "{label}: the line does not name the decision: {line}"
                );
                let tail = line.rsplit(": ").next().unwrap();
                assert_eq!(tail, "InvalidStore", "{label}: wrong fault name: {line}");
                // blue's HOLD: the reader's OWN sentence never reaches the line (R28.4).
                assert!(
                    !line.contains("Resident configuration state is invalid"),
                    "{label}: the reader's own sentence reached the line: {line}"
                );
            }
            beacond::SupervisorError::ResidentConfigurationEventMismatch(_)
            | beacond::SupervisorError::Json(_) => {}
            other => panic!("{label} failed through the wrong guard: {other:?}"),
        }
    }
}

#[test]
fn startup_refuses_capabilities_added_outside_the_operator_decision() {
    let store = TestStore::new("capability-tamper");
    let accounts = enrolled_accounts(&store);
    let resident_id = admit_lead(&store, &accounts);
    let activation = activate_model(&store, &accounts, vec![ProviderFeature::Completion]);
    let configured = ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id)
        .configure(
            resident_id,
            None,
            activation.id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    drop(accounts);

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO resident_configuration_capabilities (
                workspace_id, resident_id, configuration_id, capability
             ) VALUES (?1, ?2, ?3, 'artifact.evaluate')",
            rusqlite::params![
                store.workspace_id.to_string(),
                resident_id.to_string(),
                configured.id.to_string(),
            ],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::ResidentConfigurationEventMismatch(_))
            | Err(beacond::SupervisorError::ResidentConfigurationStateMismatch { .. })
    ));
}
