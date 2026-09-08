use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread::{self, JoinHandle};

#[cfg(debug_assertions)]
use beacond::set_after_team_enumeration_hook;
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
    assert_eq!(report.diagnostics.schema_version, 36);
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

/// R28 tier 2 pass 8 of 14: a Team the log does not account for is refused at startup, and the
/// refusal names WHICH Team.
#[test]
fn startup_refuses_an_unaccounted_team_and_names_the_row() {
    // R44: BOTH ENDS. The plant is the failing row, so its id places the failure: all `f`
    // sorts it LAST and all `0` sorts it FIRST -- the extremes of a UUID's text, so the
    // placement holds against any id the fixture mints. Tampering one end only made
    // "names the failing row" and "names that end" the same assertion.
    for (end, fill) in [("last", "f"), ("first", "0")] {
        let store = TestStore::new(&format!("r28t2-team-startup-{end}"));
        let accounts = enrolled_accounts(&store);
        let project_id = create_project(&accounts, store.workspace_id);
        let resident_ids = admit_residents(&accounts, store.workspace_id);
        let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
        for resident_id in &resident_ids {
            lifecycle.activate(*resident_id, None).unwrap();
        }

        let (_models, activation_id) = activate_model(&store, &accounts);

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
                name: "Alpha Team".to_owned(),
                members,
            })
            .expect("the fixture forms a Team");

        // R39.3 (kimi's HOLD): TWO rows, and the tampered one sorts LAST under this pass's own
        // `ORDER BY workspace_id, id`. With a one-Team fixture the honest row and the failing row are
        // the same row, so a mutant naming `rows[0]` is indistinguishable from naming the failing row
        // -- which is exactly what kimi measured surviving. The planted id is all `f`, so it sorts
        // last by construction, and the honest Team stays in the table as row 0.
        let connection = Connection::open(&store.path).unwrap();
        let columns: Vec<String> = connection
            .prepare("SELECT name FROM pragma_table_info('teams')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let planted = format!(
            "{fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}-\
         {fill}{fill}{fill}{fill}-{fill}{fill}{fill}{fill}-{fill}{fill}{fill}{fill}-\
         {fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}{fill}"
        )
        .replace([' ', '\n'], "");
        let names = columns.join(", ");
        let values = columns
            .iter()
            .map(|column| match column.as_str() {
                "id" => "?1".to_owned(),
                "name" => "'Planted Team'".to_owned(),
                other => other.to_owned(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let planted_rows = connection
            .execute(
                &format!("INSERT INTO teams ({names}) SELECT {values} FROM teams WHERE id = ?2"),
                rusqlite::params![planted, formed.id.to_string()],
            )
            .expect("the planted row inserts");
        assert_eq!(
            planted_rows, 1,
            "the plant inserted the wrong number of rows"
        );
        // R44.6: CALL the pass's own enumeration rather than copying its query.
        let honest: String = beacond::enumerate_teams(&connection)
            .expect("the pass's enumeration reads")
            .first()
            .expect("the fixture formed a Team")
            .1
            .clone();
        if end == "last" {
            assert_ne!(honest, planted, "{end}: the plant must not sort first");
        } else {
            assert_eq!(honest, planted, "{end}: the plant must sort first");
        }
        drop(connection);

        let refusal = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .expect_err("a store holding a Team with no formation Event reopened Ready");
        assert!(
            matches!(
                &refusal,
                beacond::SupervisorError::TeamStateMismatch { team_id, .. }
                    if team_id.to_string() == planted
            ) || matches!(&refusal, beacond::SupervisorError::TeamTotalsMismatch(_)),
            "refused for the wrong reason, or naming the wrong row: {refusal:?}"
        );

        // R28.4: the line is the key plus the fault's NAME (kimi's P3-2 form).
        let line = refusal.to_string();
        let named = line.rsplit(": ").next().unwrap().to_owned();
        // kimi's P2: PINNED, not just shaped. A name-shaped tail is satisfied by ANY arm,
        // so without this a pass carrying a fixed-but-wrong fault survives -- the M9 class
        // measured surviving on the pass-7 bar.
        assert_eq!(
            named, "InvalidStore",
            "the refusal carries the wrong fault: {line}"
        );
        assert!(
            !named.contains(' ') && !named.contains('{'),
            "the tail of the line is a rendering, not a name: {line}"
        );
        assert!(
            format!("{refusal:?}").contains(&named),
            "the named fault is not reachable through Debug: {refusal:?}"
        );
    }
}

/// R45.5: every startup pass's enumeration and Totals read belong to one snapshot. A public write
/// committed between those two Team reads leaves an honest before-state and honest after-state;
/// startup must not synthesize a mismatch from one half of each.
#[cfg(debug_assertions)]
#[test]
fn sol1_team_enumeration_and_totals_share_one_transaction() {
    struct ClearHook;
    impl Drop for ClearHook {
        fn drop(&mut self) {
            set_after_team_enumeration_hook(None);
        }
    }

    let store = TestStore::new("sol1-totals-snapshot");
    let accounts = enrolled_accounts(&store);
    let project_id = create_project(&accounts, store.workspace_id);
    let resident_ids = admit_residents(&accounts, store.workspace_id);
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    for resident_id in &resident_ids {
        lifecycle.activate(*resident_id, None).unwrap();
    }
    let (_models, activation_id) = activate_model(&store, &accounts);
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let members: Vec<_> = resident_ids
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
    teams
        .form(TeamDraft {
            project_id,
            name: "Alpha Team".to_owned(),
            members: members.clone(),
        })
        .unwrap();

    let state = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let target = store.path.clone();
    let hook_state = Arc::clone(&state);
    let _clear_hook = ClearHook;
    set_after_team_enumeration_hook(Some(Arc::new(move |path| {
        if path != target {
            return;
        }
        let (lock, changed) = &*hook_state;
        let mut state = lock.lock().unwrap();
        state.0 = true;
        changed.notify_all();
        while !state.1 {
            state = changed.wait(state).unwrap();
        }
    })));

    let store_path = store.path.clone();
    let bootstrap = thread::spawn(move || {
        Supervisor::bootstrap(StartupConfig {
            store_path,
            initial_workspace_name: "Ignored".to_owned(),
        })
    });
    let (lock, changed) = &*state;
    let mut guard = lock.lock().unwrap();
    while !guard.0 {
        let (next, timeout) = changed
            .wait_timeout(guard, std::time::Duration::from_secs(10))
            .unwrap();
        assert!(
            !timeout.timed_out(),
            "R45.5.TEAM_ENUMERATION_REACHED: startup never reached the Team barrier"
        );
        guard = next;
    }
    drop(guard);

    let concurrent_write = teams.form(TeamDraft {
        project_id,
        name: "Beta Team".to_owned(),
        members,
    });
    let mut guard = lock.lock().unwrap();
    guard.1 = true;
    changed.notify_all();
    drop(guard);
    concurrent_write.expect("R45.5.WAL_WRITER_COMMITS: the honest concurrent Team must commit");

    bootstrap
        .join()
        .unwrap()
        .expect("R45.5.ONE_READ_SNAPSHOT: startup mixed Team enumeration and Totals snapshots");
    set_after_team_enumeration_hook(None);
    assert_eq!(
        teams.list(project_id).unwrap().len(),
        2,
        "R45.5.HONEST_AFTER_STATE: both committed Teams remain readable"
    );
}
