use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, LocalModelActivationService, LocalModelEndpoint,
    OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft, ProjectDraft,
    ProjectService, ProviderFeature, ResidentConfigurationService, ResidentLifecycleService,
    SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    Supervisor, TeamDraft, TeamError, TeamFormationActor, TeamIneligibilityReason, TeamMemberDraft,
    TeamService,
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
        let path =
            std::env::temp_dir().join(format!("beacond-team-{label}-{}.sqlite3", Uuid::new_v4()));
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

fn create_project(accounts: &Arc<OperatorAccountService>, workspace_id: Uuid) -> Uuid {
    ProjectService::new(Arc::clone(accounts), workspace_id)
        .create(ProjectDraft {
            name: "Beacon Alpha".to_owned(),
            charter: ProjectCharterDraft {
                purpose: "Prove the local agent office.".to_owned(),
                desired_outcomes: vec!["Form one attributable Team".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        })
        .unwrap()
        .id
}

fn admit_residents(accounts: &Arc<OperatorAccountService>, workspace_id: Uuid) -> Vec<Uuid> {
    StarterOrganizationService::new(Arc::clone(accounts), workspace_id)
        .accept(
            1,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: Some("Northstar".to_owned()),
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: Some("Forge".to_owned()),
                },
                StarterResidentDraft {
                    role: StarterRole::Reviewer,
                    display_name: Some("Lens".to_owned()),
                },
            ],
        )
        .unwrap()
        .residents
        .into_iter()
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
) -> (LocalModelActivationService, Uuid) {
    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(accounts),
        store.workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
    let activation = models
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();
    (models, activation.id)
}

#[test]
fn operator_forms_a_project_team_from_exact_eligible_configurations() {
    let store = TestStore::new("form");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_ids = admit_residents(&accounts, store.workspace_id);
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    for resident_id in &resident_ids {
        lifecycle.activate(*resident_id, None).unwrap();
    }

    let (models, activation_id) = activate_model(&store, &accounts);

    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let members = resident_ids
        .iter()
        .map(|resident_id| {
            let configuration = configurations
                .configure(
                    *resident_id,
                    None,
                    activation_id,
                    vec![BeaconCapability::WorkCreateMission],
                )
                .unwrap();
            TeamMemberDraft {
                resident_id: *resident_id,
                expected_configuration_id: configuration.id,
            }
        })
        .collect();

    let teams = TeamService::new(Arc::clone(&accounts), store.workspace_id);
    let formed = teams
        .form(TeamDraft {
            project_id,
            name: "  Alpha Team  ".to_owned(),
            members,
        })
        .unwrap();

    assert_eq!(formed.project_id, project_id);
    assert_eq!(formed.name, "Alpha Team");
    assert_eq!(formed.formed_by, TeamFormationActor::Operator);
    assert_eq!(formed.members.len(), 3);
    assert!(formed.members.iter().all(|member| member.eligible_now));
    assert!(
        formed
            .members
            .iter()
            .all(|member| member.current_configuration_id == member.formation_configuration_id)
    );
    assert_eq!(teams.get(formed.id).unwrap(), formed);
    assert_eq!(
        teams.list(project_id).unwrap(),
        std::slice::from_ref(&formed)
    );

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'team.formed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    drop(connection);
    drop(teams);
    drop(configurations);
    drop(models);
    drop(lifecycle);
    drop(accounts);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.schema_version, 30);
}

#[test]
fn team_formation_refuses_ambiguous_or_ineligible_inputs_without_partial_writes() {
    let store = TestStore::new("refusals");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_id = admit_residents(&accounts, store.workspace_id)[0];
    let teams = TeamService::new(Arc::clone(&accounts), store.workspace_id);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);

    assert!(matches!(
        teams.form(TeamDraft {
            project_id,
            name: "Empty".to_owned(),
            members: vec![],
        }),
        Err(TeamError::InvalidMembers)
    ));
    assert!(matches!(
        teams.form(TeamDraft {
            project_id: Uuid::new_v4(),
            name: "Wrong Project".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id,
                expected_configuration_id: Uuid::new_v4(),
            }],
        }),
        Err(TeamError::ProjectNotFound)
    ));
    assert!(matches!(
        teams.form(TeamDraft {
            project_id,
            name: "Unconfigured".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id,
                expected_configuration_id: Uuid::new_v4(),
            }],
        }),
        Err(TeamError::ResidentConfigurationUnavailable)
    ));

    let (_models, activation_id) = activate_model(&store, &accounts);
    let inactive = configurations
        .configure(
            resident_id,
            None,
            activation_id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    let member = TeamMemberDraft {
        resident_id,
        expected_configuration_id: inactive.id,
    };
    assert!(matches!(
        teams.form(TeamDraft {
            project_id,
            name: "Inactive".to_owned(),
            members: vec![member.clone()],
        }),
        Err(TeamError::ResidentIneligible(id)) if id == resident_id
    ));
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .activate(resident_id, None)
        .unwrap();
    let current = configurations
        .configure(
            resident_id,
            Some(inactive.id),
            activation_id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    assert!(matches!(
        teams.form(TeamDraft {
            project_id,
            name: "Stale".to_owned(),
            members: vec![member],
        }),
        Err(TeamError::StaleResidentConfiguration)
    ));
    assert!(matches!(
        teams.form(TeamDraft {
            project_id,
            name: "Duplicate".to_owned(),
            members: vec![
                TeamMemberDraft {
                    resident_id,
                    expected_configuration_id: current.id,
                },
                TeamMemberDraft {
                    resident_id,
                    expected_configuration_id: current.id,
                },
            ],
        }),
        Err(TeamError::InvalidMembers)
    ));

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM teams", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'team.formed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn formation_proof_stays_fixed_while_current_member_eligibility_changes() {
    let store = TestStore::new("current-eligibility");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_id = admit_residents(&accounts, store.workspace_id)[0];
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    let active = lifecycle.activate(resident_id, None).unwrap();
    let (_models, activation_id) = activate_model(&store, &accounts);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let formation_configuration = configurations
        .configure(
            resident_id,
            None,
            activation_id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    let teams = TeamService::new(Arc::clone(&accounts), store.workspace_id);
    let formed = teams
        .form(TeamDraft {
            project_id,
            name: "Durable Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id,
                expected_configuration_id: formation_configuration.id,
            }],
        })
        .unwrap();

    let suspended = lifecycle
        .suspend(resident_id, active.latest_decision_id)
        .unwrap();
    let unavailable = teams.get(formed.id).unwrap();
    assert_eq!(
        unavailable.members[0].formation_configuration_id,
        formation_configuration.id
    );
    assert!(!unavailable.members[0].eligible_now);
    assert_eq!(
        unavailable.members[0].ineligibility_reasons,
        [TeamIneligibilityReason::ResidentSuspended]
    );

    lifecycle
        .reactivate(resident_id, suspended.latest_decision_id)
        .unwrap();
    let cleared = configurations
        .clear(resident_id, formation_configuration.id)
        .unwrap();
    let unavailable = teams.get(formed.id).unwrap();
    assert_eq!(
        unavailable.members[0].ineligibility_reasons,
        [TeamIneligibilityReason::ConfigurationCleared]
    );

    let replacement = configurations
        .configure(
            resident_id,
            Some(cleared.id),
            activation_id,
            vec![BeaconCapability::ArtifactProduce],
        )
        .unwrap();
    let restored = teams.get(formed.id).unwrap();
    assert_eq!(
        restored.members[0].formation_configuration_id,
        formation_configuration.id
    );
    assert_eq!(restored.members[0].current_configuration_id, replacement.id);
    assert!(restored.members[0].eligible_now);
    assert!(restored.members[0].ineligibility_reasons.is_empty());
}

#[test]
fn locked_sessions_and_tampered_formation_events_fail_closed() {
    let store = TestStore::new("tamper");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_ids = admit_residents(&accounts, store.workspace_id);
    let resident_id = resident_ids[0];
    let later_member_id = resident_ids[1];
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    lifecycle.activate(resident_id, None).unwrap();
    lifecycle.activate(later_member_id, None).unwrap();
    let (_models, activation_id) = activate_model(&store, &accounts);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let configuration = configurations
        .configure(
            resident_id,
            None,
            activation_id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .unwrap();
    let later_member_configuration = configurations
        .configure(
            later_member_id,
            None,
            activation_id,
            vec![BeaconCapability::ArtifactProduce],
        )
        .unwrap();
    let teams = TeamService::new(Arc::clone(&accounts), store.workspace_id);
    let formed = teams
        .form(TeamDraft {
            project_id,
            name: "Protected Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id,
                expected_configuration_id: configuration.id,
            }],
        })
        .unwrap();
    accounts.lock().unwrap();
    assert!(matches!(teams.get(formed.id), Err(TeamError::Account(_))));
    assert!(matches!(teams.list(project_id), Err(TeamError::Account(_))));
    drop(teams);
    drop(accounts);

    let connection = Connection::open(&store.path).unwrap();
    assert!(
        connection
            .execute(
                "UPDATE teams SET name = 'tampered' WHERE id = ?1",
                [formed.id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO team_formation_members (
                     workspace_id, team_id, position, resident_id, configuration_id
                 ) VALUES (?1, ?2, 1, ?3, ?4)",
                rusqlite::params![
                    store.workspace_id.to_string(),
                    formed.id.to_string(),
                    later_member_id.to_string(),
                    later_member_configuration.id.to_string(),
                ],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE events SET payload_json = '{}' WHERE kind = 'team.formed'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::TeamEventMismatch(_))
    ));
}

#[test]
fn concurrent_accepted_formations_are_distinct_immutable_teams() {
    let store = TestStore::new("concurrent");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_id = admit_residents(&accounts, store.workspace_id)[0];
    ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id)
        .activate(resident_id, None)
        .unwrap();
    let (_models, activation_id) = activate_model(&store, &accounts);
    let configuration =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id)
            .configure(
                resident_id,
                None,
                activation_id,
                vec![BeaconCapability::WorkCreateMission],
            )
            .unwrap();
    let teams = Arc::new(TeamService::new(accounts, store.workspace_id));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let teams = Arc::clone(&teams);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                teams.form(TeamDraft {
                    project_id,
                    name: "Repeated label".to_owned(),
                    members: vec![TeamMemberDraft {
                        resident_id,
                        expected_configuration_id: configuration.id,
                    }],
                })
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let formed = workers
        .into_iter()
        .map(|worker| worker.join().unwrap().unwrap())
        .collect::<Vec<_>>();

    assert_ne!(formed[0].id, formed[1].id);
    assert_eq!(teams.list(project_id).unwrap().len(), 2);
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'team.formed'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}
