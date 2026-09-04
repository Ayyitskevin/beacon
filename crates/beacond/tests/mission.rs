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
    assert_eq!(report.diagnostics.schema_version, 28);
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
    assert_eq!(report.diagnostics.schema_version, 28);
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

// ---------------------------------------------------------------------------------------------
// ADR 0041 slice 1 — condition identity.
//
// A Mission's success conditions had no identity of any kind: a JSON array of strings, stable by
// accident of three unrelated mechanisms and pointable-at by no foreign key. These six tests are the
// slice's whole acceptance, one per bullet of the design's list.
// ---------------------------------------------------------------------------------------------

/// Every condition row a store holds for one Mission, in ordinal order.
fn condition_rows(path: &PathBuf, mission_id: Uuid) -> Vec<(i64, String)> {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT ordinal, text FROM mission_success_conditions
             WHERE mission_id = ?1 ORDER BY ordinal",
        )
        .unwrap();
    statement
        .query_map([mission_id.to_string()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn admit_with_conditions(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
    conditions: Vec<String>,
) -> beacond::MissionView {
    let prepared = prepare_team(store, accounts, 1);
    let mut draft = mission_draft(&prepared);
    draft.success_conditions = conditions;
    MissionService::new(Arc::clone(accounts), store.workspace_id)
        .admit(draft)
        .unwrap()
}

#[test]
fn every_declared_condition_gets_a_row_in_declaration_order() {
    // Bullet 1. Declaration order, not sorted order: the ordinal IS the identity, so a row set that
    // is correct as a *set* and wrong as a *sequence* is wrong.
    let store = TestStore::new("conditions-order");
    let accounts = enrolled_accounts(&store);
    let declared = vec![
        "Zebra crossing is painted".to_owned(),
        "Alpha gate passes".to_owned(),
        "Middle review is recorded".to_owned(),
    ];
    let mission = admit_with_conditions(&store, &accounts, declared.clone());
    assert_eq!(mission.success_conditions, declared);

    assert_eq!(
        condition_rows(&store.path, mission.id),
        declared
            .iter()
            .enumerate()
            .map(|(ordinal, text)| (i64::try_from(ordinal).unwrap(), text.clone()))
            .collect::<Vec<_>>(),
        "the rows are not the Mission's declaration order"
    );
}

#[test]
fn twenty_conditions_give_twenty_rows_and_one_gives_one() {
    // Bullet 2. Both ends of the module's own bound, because an off-by-one at either end is a
    // different defect from an off-by-one in the middle.
    // Two stores: one store enrols one Team, and a second `prepare_team` in the same store is
    // refused as `DecisionAlreadyRecorded`.
    let many_store = TestStore::new("conditions-twenty");
    let many_accounts = enrolled_accounts(&many_store);
    let twenty: Vec<String> = (0..20).map(|index| format!("Condition {index}")).collect();
    let many = admit_with_conditions(&many_store, &many_accounts, twenty.clone());
    assert_eq!(
        condition_rows(&many_store.path, many.id),
        twenty
            .iter()
            .enumerate()
            .map(|(ordinal, text)| (i64::try_from(ordinal).unwrap(), text.clone()))
            .collect::<Vec<_>>()
    );

    let one_store = TestStore::new("conditions-one");
    let one_accounts = enrolled_accounts(&one_store);
    let one = admit_with_conditions(&one_store, &one_accounts, vec!["Only this".to_owned()]);
    assert_eq!(
        condition_rows(&one_store.path, one.id),
        vec![(0, "Only this".to_owned())]
    );
}

#[test]
fn condition_rows_refuse_update_and_refuse_delete() {
    // Bullet 3. The ordinal is an identity only if it cannot move, and the text is evidence only if
    // it cannot be rewritten. Both refusals are the store's, not a Command's.
    let store = TestStore::new("conditions-immutable");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Immutable".to_owned()]);

    let connection = Connection::open(&store.path).unwrap();
    let update = connection.execute(
        "UPDATE mission_success_conditions SET text = 'rewritten' WHERE mission_id = ?1",
        [mission.id.to_string()],
    );
    assert!(update.is_err(), "a condition row accepted an UPDATE");
    let delete = connection.execute(
        "DELETE FROM mission_success_conditions WHERE mission_id = ?1",
        [mission.id.to_string()],
    );
    assert!(delete.is_err(), "a condition row accepted a DELETE");
    drop(connection);

    assert_eq!(
        condition_rows(&store.path, mission.id),
        vec![(0, "Immutable".to_owned())]
    );
}

#[test]
fn a_condition_row_whose_text_disagrees_with_the_mission_fails_startup() {
    // Bullet 4, and the scenario ONLY the per-ordinal text check can see: the count is still right.
    // Written through a fresh table because the row itself refuses UPDATE -- which is the point:
    // getting a disagreeing row into this store takes deliberate tampering, and reconciliation is
    // what notices it anyway.
    let store = TestStore::new("conditions-text-drift");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Declared exactly".to_owned()]);

    let connection = Connection::open(&store.path).unwrap();
    // The row refuses UPDATE, so the trigger comes off and goes back on **byte-identically** -- its
    // own stored text, replayed. `verify_schema` compares the store's schema against the migrations,
    // so a re-creation that differs by one space refuses startup as `SchemaMismatch` and proves
    // nothing about this check. (It did, first try.)
    let trigger_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'trigger' AND name = 'mission_success_conditions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_success_conditions_no_update;
             UPDATE mission_success_conditions SET text = 'something else entirely';",
        )
        .unwrap();
    connection.execute_batch(&trigger_sql).unwrap();
    let count: i64 = connection
        .query_row(
            "SELECT count(*) FROM mission_success_conditions WHERE mission_id = ?1",
            [mission.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "the count must still agree, or this proves nothing"
    );
    drop(connection);

    let outcome = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        matches!(outcome, Err(beacond::SupervisorError::MissionStateMismatch)),
        "startup accepted a condition row that disagrees with its Mission: {outcome:?}"
    );
}

#[test]
fn a_condition_row_the_mission_never_declared_fails_startup() {
    // Bullet 5, and the scenario ONLY the count check can see: every declared ordinal still holds
    // exactly the declared text, so a walk of the Mission's array finds nothing wrong. The extra row
    // sits past the end of it.
    let store = TestStore::new("conditions-extra-row");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Declared exactly".to_owned()]);

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "INSERT INTO mission_success_conditions (workspace_id, mission_id, ordinal, text)
             VALUES (?1, ?2, 1, 'a condition nobody declared')",
            [store.workspace_id.to_string(), mission.id.to_string()],
        )
        .unwrap();
    let declared: String = connection
        .query_row(
            "SELECT text FROM mission_success_conditions
             WHERE mission_id = ?1 AND ordinal = 0",
            [mission.id.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        declared, "Declared exactly",
        "the declared ordinal must still be right, or this proves nothing"
    );
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::MissionStateMismatch)
    ));
}

#[test]
fn the_condition_migration_adds_triggers_and_rewrites_none() {
    // Bullet 6. Every migration in this series that touched the Event-kind vocabulary had to drop
    // and re-create every trigger referencing `events`, and one forgotten trigger is a guard that
    // silently stops existing. This migration is additive and adds no Event, so the whole inventory
    // is pinned: any future migration that drops, renames or rewrites a trigger turns this red.
    let store = TestStore::new("conditions-triggers");
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger' ORDER BY name")
        .unwrap();
    let triggers = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        triggers.contains(&"mission_success_conditions_no_update".to_owned())
            && triggers.contains(&"mission_success_conditions_no_delete".to_owned()),
        "the new triggers are missing: {triggers:?}"
    );
    assert!(
        triggers.contains(&"team_formation_members_no_insert_after_formation".to_owned())
            && triggers.contains(&"mission_admission_members_no_insert_after_admission".to_owned()),
        "an Event-keyed roster guard stopped existing: {triggers:?}"
    );

    // ...and the migration's own text is additive: it names no trigger of anyone else's, renames no
    // table, and does not touch `events`. Asserted against the shipped file, so a later edit to it
    // is what fails rather than a store that happens to look right.
    let migration = include_str!("../migrations/0024_mission_success_conditions.sql");
    for forbidden in ["DROP TRIGGER", "ALTER TABLE", "DROP TABLE", "UPDATE events"] {
        assert!(
            !migration.contains(forbidden),
            "the migration is not additive: it contains {forbidden}"
        );
    }
}

/// Migration 0028 REBUILT `evaluations` and its three related tables to generalize the subject, so
/// a rollback to 23 has to put 0023's shapes back -- otherwise the store keeps a v28 `evaluations`
/// whose triggers reference `artifacts`, which the rollback above has just dropped.
///
/// The definitions are sliced out of 0023's own text, like `restore_pre_0025_events` above: a
/// rollback that retyped them would be testing the test. This store has no Evaluations, so the
/// tables come back empty.
fn restore_pre_0028_evaluations(connection: &Connection) {
    let source = include_str!("../migrations/0023_evaluations.sql");
    let from = source
        .find("CREATE TABLE evaluations (")
        .expect("0023 defines the evaluations table");
    let to = source
        .find("UPDATE schema_meta")
        .expect("0023 stamps its version last");
    connection
        .execute_batch(
            "DROP TRIGGER evaluations_evaluator_is_not_the_artifact_author;
             DROP TRIGGER evaluations_artifact_subject_carries_its_mission;
             DROP TRIGGER evaluations_no_self_evaluation;
             DROP TRIGGER evaluations_no_update;
             DROP TRIGGER evaluations_no_delete;
             DROP TRIGGER evaluation_supersessions_no_update;
             DROP TRIGGER evaluation_supersessions_no_delete;
             DROP TRIGGER evaluation_findings_no_update;
             DROP TRIGGER evaluation_findings_no_delete;
             DROP TRIGGER evaluation_cited_runs_no_update;
             DROP TRIGGER evaluation_cited_runs_no_delete;
             DROP TABLE evaluation_supersessions;
             DROP TABLE evaluation_cited_runs;
             DROP TABLE evaluation_findings;
             DROP TABLE evaluations;",
        )
        .unwrap();
    connection.execute_batch(&source[from..to]).unwrap();
}

/// Migrations 0025 and 0026 each rebuild the closed Event-kind table -- 0025 to add two kinds, 0026
/// to add `mission.completed`. A rollback that drops only their tables leaves the rebuilt table
/// behind, and the store then matches no schema version. Restoring 0023's definition takes both
/// rebuilds off at once, which is what a rollback to 23 needs.
///
/// The definition and the two Event-keyed roster triggers are read out of 0023's own text rather
/// than retyped: a rollback that retyped them would be testing the test.
fn restore_pre_0025_events(connection: &Connection) {
    let source = include_str!("../migrations/0023_evaluations.sql");
    let block = |start: &str| -> String {
        let from = source.find(start).expect("0023 defines it");
        let end = source[from..].find("\n\n").expect("the block ends");
        source[from..from + end].to_owned()
    };
    let events = block("CREATE TABLE events (");
    let team_guard = block("CREATE TRIGGER team_formation_members_no_insert_after_formation");
    let mission_guard = block("CREATE TRIGGER mission_admission_members_no_insert_after_admission");
    connection
        .execute_batch(
            "DROP TRIGGER team_formation_members_no_insert_after_formation;
             DROP TRIGGER mission_admission_members_no_insert_after_admission;
             ALTER TABLE events RENAME TO events_rollback;",
        )
        .unwrap();
    connection.execute_batch(&events).unwrap();
    connection
        .execute_batch(
            "INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
             SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
             FROM events_rollback ORDER BY sequence;
             DROP TABLE events_rollback;",
        )
        .unwrap();
    connection.execute_batch(&team_guard).unwrap();
    connection.execute_batch(&mission_guard).unwrap();
}

#[test]
fn a_store_at_the_previous_schema_version_migrates_and_keeps_its_condition_rows() {
    // Bullet 1's other half, and the only test that exercises the BACKFILL rather than the write
    // path. Every other slice-1 test admits its Mission after the migration, so the rows it checks
    // came from `insert_admission`; this one takes a store that already has a Mission, drops the
    // table, rolls the version back, and lets the migration rebuild it from the Mission's own JSON.
    //
    // It found two defects the other six could not: the missing `20 =>` ladder arm (every store on
    // main is at 20, and without the arm they all refuse startup), and that the backfill's order is
    // load-bearing and untested.
    let store = TestStore::new("conditions-backfill");
    let accounts = enrolled_accounts(&store);
    let declared = vec![
        "Zebra crossing is painted".to_owned(),
        "Alpha gate passes".to_owned(),
        "Middle review is recorded".to_owned(),
    ];
    let mission = admit_with_conditions(&store, &accounts, declared.clone());

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            // Newest migration first. 0027's Artifact store is newest (the relation first, since
            // it keys into `artifacts`). 0026 and 0025 BOTH rebuilt the closed `events` table --
            // 0026 to add `mission.completed`, 0025 to add two kinds -- so dropping their tables
            // is not enough: the rebuilt definition has to come off too or the store no longer
            // matches schema 23. 0027 rebuilt `events` too; `restore_pre_0025_events` below
            // unwinds every rebuild at once, back to 0023's definition.
            "DROP TRIGGER artifact_supersessions_no_update;
             DROP TRIGGER artifact_supersessions_no_delete;
             DROP TABLE artifact_supersessions;
             DROP TRIGGER artifacts_no_update;
             DROP TRIGGER artifacts_no_delete;
             DROP TABLE artifacts;
             DROP TRIGGER mission_completion_conditions_no_update;
             DROP TRIGGER mission_completion_conditions_no_delete;
             DROP TABLE mission_completion_conditions;
             DROP TRIGGER mission_completions_no_update;
             DROP TRIGGER mission_completions_no_delete;
             DROP TABLE mission_completions;
             DROP TRIGGER mission_impact_classifications_no_update;
             DROP TRIGGER mission_impact_classifications_no_delete;
             DROP TABLE mission_impact_classifications;
             DROP TRIGGER measured_spend_no_update;
             DROP TRIGGER measured_spend_no_delete;
             DROP TABLE measured_spend;
             DROP TABLE mission_success_conditions;
             UPDATE schema_meta SET version = 23 WHERE singleton = 1;",
        )
        .unwrap();
    restore_pre_0028_evaluations(&connection);
    restore_pre_0025_events(&connection);
    drop(connection);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("a store at the previous schema version must migrate");
    assert_eq!(report.diagnostics.schema_version, 28);

    assert_eq!(
        condition_rows(&store.path, mission.id),
        declared
            .iter()
            .enumerate()
            .map(|(ordinal, text)| (i64::try_from(ordinal).unwrap(), text.clone()))
            .collect::<Vec<_>>(),
        "the backfill did not reproduce the Mission's declaration order"
    );
}

// ---------------------------------------------------------------------------------------------
// Migration 0027 — the completion store, its Event, and the verified reader.
//
// One test per bullet of `A-opus-mission-completion-design.md` §12 slice 3.
// ---------------------------------------------------------------------------------------------

fn completion_for(
    store: &TestStore,
    mission: &beacond::MissionView,
    boundary: u64,
    outcomes: Vec<beacond::ConditionOutcome>,
) -> beacond::MissionCompletionV1 {
    let member = &mission.members[0];
    beacond::MissionCompletionV1 {
        workspace_id: store.workspace_id,
        project_id: mission.project_id,
        mission_id: mission.id,
        completion_id: Uuid::new_v4(),
        actor: beacond::CompletionActor::Operator,
        accountable_resident_id: member.resident_id,
        accountable_resident_configuration_id: member.admission_configuration_id,
        boundary_event_seq: boundary,
        disposition: beacond::CompletionDisposition::Concluded,
        declared_conditions_sha256: beacond::declared_conditions_digest(
            &mission.success_conditions,
        ),
        condition_results: outcomes
            .into_iter()
            .enumerate()
            .map(|(ordinal, outcome)| beacond::ConditionResultRecord {
                ordinal: u32::try_from(ordinal).unwrap(),
                outcome,
                support: vec![],
                note: None,
            })
            .collect(),
        delivered_artifacts: vec![],
        unfinished_work: vec![],
        uncertainty: vec![],
        spend: beacond::SpendReference::NotMeasured("no spend record exists".to_owned()),
        supersedes: beacond::CompletionSupersedes::None,
        summary: "One reviewed feature shipped.".to_owned(),
        completed_at_ms: 9_000,
    }
}

/// The Mission's admission Event sequence -- the floor a completion's boundary must be at or above.
fn admitted_sequence(store: &TestStore, mission_id: Uuid) -> u64 {
    Connection::open(&store.path)
        .unwrap()
        .query_row(
            "SELECT min(sequence) FROM events WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?1",
            [mission_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map(|sequence| u64::try_from(sequence).unwrap())
        .unwrap()
}

#[test]
fn a_completion_naming_every_declared_condition_exactly_once_is_accepted() {
    // Bullet 1, and the positive control every refusal below is measured against.
    let store = TestStore::new("completion-accepted");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(
        &store,
        &accounts,
        vec!["Gates pass".to_owned(), "Evidence filed".to_owned()],
    );
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![
            beacond::ConditionOutcome::Met,
            beacond::ConditionOutcome::AssessmentInconclusive,
        ],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).expect("it is accepted");
    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest).unwrap(),
        record
    );
    drop(connection);

    // ...and it survives a restart, which is the only proof the store holds what it claims.
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("startup verifies the completion");
}

#[test]
fn a_gap_a_duplicate_and_an_ordinal_past_the_end_are_three_distinguishable_refusals() {
    // Bullet 2 and bullet 3. A duplicate and an ordinal past the end are refused by the STORE -- the
    // primary key and the foreign key to `mission_success_conditions` -- and a gap by the reader's
    // count. Three shapes, three mechanisms, and the errors tell them apart.
    let store = TestStore::new("completion-ordinals");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(
        &store,
        &accounts,
        vec!["Gates pass".to_owned(), "Evidence filed".to_owned()],
    );
    let boundary = admitted_sequence(&store, mission.id);

    // A gap: one result for two declared conditions. The record encodes -- ordinals are dense from
    // zero within it -- and the reader refuses, because only the Mission knows there are two. The
    // WRITE refuses now too: `record_completion` reads the record back through that reader inside
    // its transaction, so a gap never reaches the store. (It used to be storable and then
    // unreadable, which told a caller the write had succeeded.)
    let short = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );
    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert_eq!(
        beacond::record_completion(&mut connection, &short),
        Err(beacond::CompletionReadRefusal::ConditionsNotExactlyOnce {
            declared: 2,
            stored: 1
        })
    );

    // The positive control the other two shapes attach to: a record that names both conditions.
    let whole = completion_for(
        &store,
        &mission,
        boundary,
        vec![
            beacond::ConditionOutcome::Met,
            beacond::ConditionOutcome::NotMet,
        ],
    );
    let digest = beacond::record_completion(&mut connection, &whole).expect("it is accepted");

    // A duplicate ordinal: refused by the primary key, before any reader sees it.
    let duplicate = connection.execute(
        "INSERT INTO mission_completion_conditions
             (workspace_id, completion_digest, mission_id, condition_ordinal, outcome)
         VALUES (?1, ?2, ?3, 0, 'met')",
        rusqlite::params![
            store.workspace_id.to_string(),
            digest,
            mission.id.to_string()
        ],
    );
    assert!(duplicate.is_err(), "a duplicate ordinal was stored");

    // An ordinal past the end of the Mission's declaration: refused by the foreign key to
    // `mission_success_conditions`, which is why that table exists.
    let past_the_end = connection.execute(
        "INSERT INTO mission_completion_conditions
             (workspace_id, completion_digest, mission_id, condition_ordinal, outcome)
         VALUES (?1, ?2, ?3, 7, 'met')",
        rusqlite::params![
            store.workspace_id.to_string(),
            digest,
            mission.id.to_string()
        ],
    );
    assert!(
        past_the_end.is_err(),
        "an ordinal the Mission never declared was stored"
    );
}

#[test]
fn a_completion_about_a_tampered_mission_is_refused_even_with_a_complete_ordinal_set() {
    // Bullet 5, and the scenario the exactly-once check cannot see: the ordinal set is complete and
    // the CONDITIONS moved. Two checks, two scenarios, neither masking the other.
    let store = TestStore::new("completion-tampered");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();
    assert!(beacond::verified_completion_within(&connection, store.workspace_id, &digest).is_ok());

    // Rewrite the Mission's condition text under the completion. The trigger comes off and goes back
    // on from its own stored SQL, so only the row changes.
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'mission_success_conditions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_success_conditions_no_update;
             UPDATE mission_success_conditions SET text = 'something else entirely';",
        )
        .unwrap();
    connection.execute_batch(&trigger).unwrap();

    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest),
        Err(beacond::CompletionReadRefusal::DeclaredConditionsMoved),
        "the ordinal set is still complete, so only the digest can see this"
    );
}

#[test]
fn a_boundary_before_the_missions_admission_is_refused() {
    // Bullet 6, first scenario: a completion cannot claim a cut before its own Mission existed.
    let store = TestStore::new("completion-boundary-low");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let record = completion_for(&store, &mission, 0, vec![beacond::ConditionOutcome::Met]);

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert_eq!(
        beacond::record_completion(&mut connection, &record),
        Err(beacond::CompletionReadRefusal::BoundaryOutOfRange)
    );
}

#[test]
fn a_boundary_at_or_after_the_completions_own_event_is_refused() {
    // Bullet 6, second scenario, and a different mechanism from the first: the record's own Event
    // must land strictly after the cut it reports at. A caller cannot satisfy this by choosing a
    // number, because the sequence is the store's to assign -- so the test picks one past it.
    let store = TestStore::new("completion-boundary-high");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let record = completion_for(
        &store,
        &mission,
        u64::from(u32::MAX),
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert_eq!(
        beacond::record_completion(&mut connection, &record),
        Err(beacond::CompletionReadRefusal::BoundaryOutOfRange)
    );
}

#[test]
fn a_cross_workspace_read_is_a_refusal_and_not_an_empty_result() {
    // Bullet 7. "Not yours" and "not there" are different answers, and a scoped query that returns
    // nothing tells a caller the wrong thing about their own store.
    let store = TestStore::new("completion-cross-workspace");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();

    assert_eq!(
        beacond::verified_completion_within(&connection, Uuid::new_v4(), &digest),
        Err(beacond::CompletionReadRefusal::CrossWorkspace)
    );
    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &"0".repeat(64)),
        Err(beacond::CompletionReadRefusal::Absent),
        "and a digest nobody stored is Absent, which is the other answer"
    );
}

#[test]
fn an_accountable_resident_not_on_the_admission_roster_is_refused_by_the_store() {
    // Bullet 8. Accountability is structural, not asserted: the composite foreign key requires the
    // Resident to be an admitted member of THIS Mission under the exact configuration named.
    let store = TestStore::new("completion-roster");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let mut record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );
    record.accountable_resident_id = Uuid::new_v4();

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert!(
        beacond::record_completion(&mut connection, &record).is_err(),
        "a completion named a Resident this Mission never admitted"
    );
}

#[test]
fn a_delivered_artifact_is_refused_while_no_artifact_entity_exists() {
    // Bullet 9. The reference's SHAPE is settled (R16: the Artifact record's digest); its referent
    // is not. Storing a digest nothing can verify would be exactly the dangling claim the record's
    // vocabulary refuses to have a literal for, so the refusal names what is missing.
    let store = TestStore::new("completion-artifact");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let mut record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );
    record.delivered_artifacts = vec![[0x22; 32]];

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    assert_eq!(
        beacond::record_completion(&mut connection, &record),
        Err(beacond::CompletionReadRefusal::NoArtifactEntity)
    );
}

#[test]
fn a_row_that_disagrees_with_its_own_bytes_fails_the_reader_and_fails_startup() {
    // Bullet 4. The indexed columns are a projection of the bytes, and a projection that can drift
    // from its source is a second place to be wrong about what a completion says.
    let store = TestStore::new("completion-column-drift");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();

    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'mission_completions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_completions_no_update;
             UPDATE mission_completions SET disposition = 'abandoned';",
        )
        .unwrap();
    connection.execute_batch(&trigger).unwrap();

    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest),
        Err(beacond::CompletionReadRefusal::ColumnDisagrees(
            "disposition"
        ))
    );
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::MissionCompletionStateMismatch)
    ));
}

#[test]
fn an_orphan_completion_event_fails_startup() {
    // Bullet 11. Exactly one `mission.completed` Event per completion: an Event naming no record is
    // as much a lie about the ledger as a record with no Event.
    let store = TestStore::new("completion-orphan-event");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();
    let payload = json!({
        "workspaceId": store.workspace_id,
        "missionId": mission.id,
        "completionId": Uuid::new_v4(),
        "digestValue": "0".repeat(64),
        "canonicalBytesBase64": "",
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'mission.completed', ?2, 9001)",
            rusqlite::params![store.workspace_id.to_string(), payload],
        )
        .unwrap();
    assert!(!digest.is_empty());
    drop(connection);

    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "an orphan completion event passed startup"
    );
}

#[test]
fn a_row_whose_bytes_no_longer_hash_to_its_digest_is_refused() {
    // Bullet 4's other half, and the scenario only the digest recomputation can see: every indexed
    // column still agrees with the bytes, and the BYTES are not the ones the digest names. The
    // mutation table found this untested -- removing the recomputation left every test green,
    // because the column comparison catches a different tampering.
    let store = TestStore::new("completion-digest-drift");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![beacond::ConditionOutcome::Met],
    );

    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();

    // A second record that differs only in its summary: legal bytes, every column of the stored row
    // still true of them, and a digest that names the first record's bytes rather than these.
    let mut other = record.clone();
    other.summary = "A different summary entirely.".to_owned();
    let other_bytes = other.canonical_bytes().unwrap();

    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'mission_completions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER mission_completions_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE mission_completions SET canonical_bytes = ?1 WHERE digest_value = ?2",
            rusqlite::params![other_bytes, digest],
        )
        .unwrap();
    connection.execute_batch(&trigger).unwrap();

    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest),
        Err(beacond::CompletionReadRefusal::BytesDisagree),
        "identity is a computation over the bytes, not the column beside them"
    );
}
