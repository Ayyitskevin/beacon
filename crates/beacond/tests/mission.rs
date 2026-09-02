use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread::{self, JoinHandle};

use beacond::{
    AutonomyProfile, BeaconCapability, LocalModelActivationService, LocalModelEndpoint,
    MissionAdmissionActor, MissionDraft, MissionError, MissionLifecycle, MissionMemberDraft,
    MissionService, OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft,
    ProjectDraft, ProjectService, ProjectView, ProviderFeature, ResidentConfiguration,
    ResidentConfigurationService, ResidentLifecycleService, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    TeamDraft, TeamMemberDraft, TeamService, TeamView,
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
            "beacond-mission-{label}-{}.sqlite3",
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

struct PreparedTeam {
    project: ProjectView,
    team: TeamView,
    configurations: Vec<ResidentConfiguration>,
}

fn prepare_team(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
    member_count: usize,
) -> PreparedTeam {
    let project = ProjectService::new(Arc::clone(accounts), store.workspace_id)
        .create(ProjectDraft {
            name: "Beacon Alpha".to_owned(),
            charter: ProjectCharterDraft {
                purpose: "Prove the local agent office.".to_owned(),
                desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        })
        .unwrap();
    let roles = [
        StarterRole::Lead,
        StarterRole::Builder,
        StarterRole::Reviewer,
    ];
    let residents = StarterOrganizationService::new(Arc::clone(accounts), store.workspace_id)
        .accept(
            1,
            roles[..member_count]
                .iter()
                .copied()
                .map(|role| StarterResidentDraft {
                    role,
                    display_name: None,
                })
                .collect(),
        )
        .unwrap()
        .residents;
    let lifecycle = ResidentLifecycleService::new(Arc::clone(accounts), store.workspace_id);
    for resident in &residents {
        lifecycle.activate(resident.id, None).unwrap();
    }
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
    let configurations = residents
        .iter()
        .map(|resident| {
            ResidentConfigurationService::new(Arc::clone(accounts), store.workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    vec![BeaconCapability::WorkCreateMission],
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let team = TeamService::new(Arc::clone(accounts), store.workspace_id)
        .form(TeamDraft {
            project_id: project.id,
            name: "Mission Team".to_owned(),
            members: configurations
                .iter()
                .map(|configuration| TeamMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                })
                .collect(),
        })
        .unwrap();
    PreparedTeam {
        project,
        team,
        configurations,
    }
}

fn mission_draft(prepared: &PreparedTeam) -> MissionDraft {
    MissionDraft {
        project_id: prepared.project.id,
        expected_charter_revision: prepared.project.charter.revision,
        team_id: prepared.team.id,
        desired_outcome: "  Ship one reviewed Beacon feature.\nPreserve its evidence.  ".to_owned(),
        success_conditions: vec!["Relevant gates pass".to_owned()],
        constraints: vec!["Keep the change reversible".to_owned()],
        prohibitions: vec!["Do not deploy".to_owned()],
        members: prepared
            .configurations
            .iter()
            .map(|configuration| MissionMemberDraft {
                resident_id: configuration.resident_id,
                expected_configuration_id: configuration.id,
            })
            .collect(),
    }
}

#[test]
fn operator_admits_a_finite_mission_from_exact_current_team_evidence() {
    let store = TestStore::new("admit");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let configuration = &prepared.configurations[0];

    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();

    assert_eq!(admitted.project_id, prepared.project.id);
    assert_eq!(admitted.charter_revision, prepared.project.charter.revision);
    assert_eq!(admitted.team_id, prepared.team.id);
    assert_eq!(
        admitted.desired_outcome,
        "Ship one reviewed Beacon feature.\nPreserve its evidence."
    );
    assert_eq!(admitted.success_conditions, ["Relevant gates pass"]);
    assert_eq!(admitted.constraints, ["Keep the change reversible"]);
    assert_eq!(admitted.prohibitions, ["Do not deploy"]);
    assert_eq!(admitted.lifecycle, MissionLifecycle::Admitted);
    assert_eq!(admitted.admitted_by, MissionAdmissionActor::Operator);
    assert_eq!(admitted.members.len(), 1);
    assert_eq!(admitted.members[0].resident_id, configuration.resident_id);
    assert_eq!(
        admitted.members[0].admission_configuration_id,
        configuration.id
    );
    assert_eq!(missions.get(admitted.id).unwrap(), admitted);
    let listed = missions.list(prepared.project.id).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], admitted);

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'mission.admitted'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    drop(connection);
    drop(missions);
    drop(accounts);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.schema_version, 23);
}

#[test]
fn schema_rejects_a_mission_member_outside_the_owning_team() {
    let store = TestStore::new("team-member-foreign-key");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 2);
    let owning_team = TeamService::new(Arc::clone(&accounts), store.workspace_id)
        .form(TeamDraft {
            project_id: prepared.project.id,
            name: "Single-member Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id: prepared.configurations[0].resident_id,
                expected_configuration_id: prepared.configurations[0].id,
            }],
        })
        .unwrap();
    let outside_member = &prepared.configurations[1];
    let mission_id = Uuid::new_v4();
    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO missions (
                 id, workspace_id, project_id, charter_revision, team_id, desired_outcome,
                 success_conditions_json, constraints_json, prohibitions_json, lifecycle,
                 admitted_by, admitted_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'Test the Team boundary',
                       '[\"Boundary holds\"]', '[]', '[]', 'admitted', 'operator', 1)",
            rusqlite::params![
                mission_id.to_string(),
                store.workspace_id.to_string(),
                prepared.project.id.to_string(),
                i64::from(prepared.project.charter.revision),
                owning_team.id.to_string(),
            ],
        )
        .unwrap();

    assert!(
        transaction
            .execute(
                "INSERT INTO mission_admission_members (
                     workspace_id, mission_id, team_id, position, resident_id, configuration_id
                 ) VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                rusqlite::params![
                    store.workspace_id.to_string(),
                    mission_id.to_string(),
                    owning_team.id.to_string(),
                    outside_member.resident_id.to_string(),
                    outside_member.id.to_string(),
                ],
            )
            .is_err()
    );
}

#[test]
fn mission_admission_refuses_invalid_stale_or_ineligible_inputs_without_writes() {
    let store = TestStore::new("refusals");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 2);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);

    let mut invalid_outcome = mission_draft(&prepared);
    invalid_outcome.desired_outcome = "   ".to_owned();
    assert!(matches!(
        missions.admit(invalid_outcome),
        Err(MissionError::InvalidDesiredOutcome)
    ));
    let mut control_outcome = mission_draft(&prepared);
    control_outcome.desired_outcome = "unsafe\0outcome".to_owned();
    assert!(matches!(
        missions.admit(control_outcome),
        Err(MissionError::InvalidDesiredOutcome)
    ));
    let mut invalid_success = mission_draft(&prepared);
    invalid_success.success_conditions.clear();
    assert!(matches!(
        missions.admit(invalid_success),
        Err(MissionError::InvalidSuccessConditions)
    ));
    let mut duplicate_success = mission_draft(&prepared);
    duplicate_success.success_conditions = vec!["Same".to_owned(), " Same ".to_owned()];
    assert!(matches!(
        missions.admit(duplicate_success),
        Err(MissionError::DuplicateMissionEntry)
    ));
    let mut missing_members = mission_draft(&prepared);
    missing_members.members.clear();
    assert!(matches!(
        missions.admit(missing_members),
        Err(MissionError::InvalidMembers)
    ));
    let mut reordered = mission_draft(&prepared);
    reordered.members.reverse();
    assert!(matches!(
        missions.admit(reordered),
        Err(MissionError::StaleTeamEligibility)
    ));
    let mut stale_configuration = mission_draft(&prepared);
    stale_configuration.members[0].expected_configuration_id = Uuid::new_v4();
    assert!(matches!(
        missions.admit(stale_configuration),
        Err(MissionError::StaleTeamEligibility)
    ));
    let mut wrong_team = mission_draft(&prepared);
    wrong_team.team_id = Uuid::new_v4();
    assert!(matches!(
        missions.admit(wrong_team),
        Err(MissionError::TeamNotFound)
    ));

    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    let first_resident = lifecycle
        .list()
        .unwrap()
        .into_iter()
        .find(|resident| resident.id == prepared.configurations[0].resident_id)
        .unwrap();
    let suspended = lifecycle
        .suspend(first_resident.id, first_resident.latest_decision_id)
        .unwrap();
    assert!(matches!(
        missions.admit(mission_draft(&prepared)),
        Err(MissionError::ResidentIneligible(id)) if id == first_resident.id
    ));
    lifecycle
        .reactivate(first_resident.id, suspended.latest_decision_id)
        .unwrap();

    ProjectService::new(Arc::clone(&accounts), store.workspace_id)
        .revise_charter(
            prepared.project.id,
            prepared.project.charter.revision,
            ProjectCharterDraft {
                purpose: "Prove the local agent office safely.".to_owned(),
                desired_outcomes: prepared.project.charter.desired_outcomes.clone(),
                constraints: prepared.project.charter.constraints.clone(),
                prohibitions: prepared.project.charter.prohibitions.clone(),
                autonomy_profile: prepared.project.charter.autonomy_profile,
            },
        )
        .unwrap();
    assert!(matches!(
        missions.admit(mission_draft(&prepared)),
        Err(MissionError::StaleCharterRevision {
            expected: 1,
            current: 2
        })
    ));

    accounts.lock().unwrap();
    let mut malformed_while_locked = mission_draft(&prepared);
    malformed_while_locked.members.clear();
    assert!(matches!(
        missions.admit(malformed_while_locked),
        Err(MissionError::Account(_))
    ));
    assert!(matches!(
        missions.list(prepared.project.id),
        Err(MissionError::Account(_))
    ));
    assert!(matches!(
        missions.get(Uuid::new_v4()),
        Err(MissionError::Account(_))
    ));

    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM missions", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'mission.admitted'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn admission_evidence_stays_fixed_when_resident_configuration_changes() {
    let store = TestStore::new("fixed-evidence");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();
    let original_configuration = &prepared.configurations[0];
    let activation_id = original_configuration
        .binding
        .as_ref()
        .unwrap()
        .activation_id;
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);

    let cleared = configurations
        .clear(
            original_configuration.resident_id,
            original_configuration.id,
        )
        .unwrap();
    let replacement = configurations
        .configure(
            original_configuration.resident_id,
            Some(cleared.id),
            activation_id,
            vec![BeaconCapability::ArtifactProduce],
        )
        .unwrap();
    assert_ne!(replacement.id, original_configuration.id);

    let reloaded = missions.get(admitted.id).unwrap();
    assert_eq!(reloaded, admitted);
    assert_eq!(
        reloaded.members[0].admission_configuration_id,
        original_configuration.id
    );
}

#[test]
fn mission_state_is_immutable_and_tampered_events_fail_startup() {
    let store = TestStore::new("tamper");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();
    drop(missions);
    drop(accounts);

    let connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE missions SET desired_outcome = 'tampered' WHERE id = ?1",
                [admitted.id.to_string()],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO mission_admission_members (
                     workspace_id, mission_id, team_id, position, resident_id, configuration_id
                 ) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                rusqlite::params![
                    store.workspace_id.to_string(),
                    admitted.id.to_string(),
                    admitted.team_id.to_string(),
                    admitted.members[0].resident_id.to_string(),
                    admitted.members[0].admission_configuration_id.to_string(),
                ],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE events SET payload_json = '{}' WHERE kind = 'mission.admitted'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::MissionEventMismatch(_))
    ));
}

#[test]
fn schema_ten_migrates_without_rewriting_events_or_the_team_roster_guard() {
    let path = std::env::temp_dir().join(format!(
        "beacond-mission-v10-migration-{}.sqlite3",
        Uuid::new_v4()
    ));
    let workspace_id = Uuid::new_v4();
    let store = TestStore {
        path: path.clone(),
        workspace_id,
    };
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    for migration in [
        include_str!("../migrations/0001_initial.sql"),
        include_str!("../migrations/0002_operator_account.sql"),
        include_str!("../migrations/0003_session_policy.sql"),
        include_str!("../migrations/0004_starter_organization.sql"),
        include_str!("../migrations/0005_projects.sql"),
        include_str!("../migrations/0006_project_charter_revisions.sql"),
        include_str!("../migrations/0007_local_model_activations.sql"),
        include_str!("../migrations/0008_resident_lifecycle.sql"),
        include_str!("../migrations/0009_resident_configurations.sql"),
        include_str!("../migrations/0010_teams.sql"),
    ] {
        connection.execute_batch(migration).unwrap();
    }
    connection
        .execute(
            "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 7)",
            [workspace_id.to_string()],
        )
        .unwrap();
    let payload = json!({ "workspace_id": workspace_id, "name": "Personal" }).to_string();
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'workspace.created', ?2, 7)",
            rusqlite::params![workspace_id.to_string(), payload],
        )
        .unwrap();
    let before = connection
        .query_row(
            "SELECT sequence, kind, payload_json, occurred_at_ms FROM events",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    drop(connection);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .unwrap();
    assert_eq!(report.diagnostics.schema_version, 23);
    let connection = Connection::open(&path).unwrap();
    let after = connection
        .query_row(
            "SELECT sequence, kind, payload_json, occurred_at_ms FROM events",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(after, before);
    let guard_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger'
               AND name = 'team_formation_members_no_insert_after_formation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(guard_sql.contains("FROM events"));
    assert!(!guard_sql.contains("events_v10"));
    drop(connection);
    drop(store);
}

#[test]
fn concurrent_accepted_admissions_create_distinct_immutable_missions() {
    let store = TestStore::new("concurrent");
    let accounts = enrolled_accounts(&store);
    let prepared = Arc::new(prepare_team(&store, &accounts, 1));
    let missions = Arc::new(MissionService::new(accounts, store.workspace_id));
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|_| {
            let missions = Arc::clone(&missions);
            let prepared = Arc::clone(&prepared);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                missions.admit(mission_draft(&prepared))
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let admitted = workers
        .into_iter()
        .map(|worker| worker.join().unwrap().unwrap())
        .collect::<Vec<_>>();

    assert_ne!(admitted[0].id, admitted[1].id);
    assert_eq!(missions.list(prepared.project.id).unwrap().len(), 2);
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'mission.admitted'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        2
    );
}
