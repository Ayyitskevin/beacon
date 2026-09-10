use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::PermissionsExt;
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
    assert_eq!(report.diagnostics.schema_version, 36);
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
            // R58: this fixture builds a v10-shaped `events`, which has no chain columns. The
            // chain's vocabulary belongs only where the chain's schema is.
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
    assert_eq!(report.diagnostics.schema_version, 36);
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
        matches!(
            outcome,
            Err(beacond::SupervisorError::MissionStateMismatch { .. })
        ),
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
        Err(beacond::SupervisorError::MissionStateMismatch { .. })
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
/// R46: `receipts` back to its 0022 shape.
///
/// 0033 added the final SQL outcome variant and 0034 removed that membership CHECK; both rebuild
/// `receipts` and rewrite `evaluation_cited_runs` with it (a foreign key follows a rename). Any
/// fixture that rolls a store below 33 has to unwind both -- before R46 `receipts` had been untouched
/// since 0022, so no rollback named it. The CREATE and its two triggers only: 0022's own carry-over
/// INSERT from `receipts_v21` belongs to that rebuild.
fn restore_pre_0033_receipts(connection: &Connection) {
    let source = include_str!("../migrations/0022_candidates.sql");
    let block = &source[source
        .find("CREATE TABLE receipts (")
        .expect("0022 defines the receipts table")
        ..source
            .find("CREATE TABLE dispatch_preparations (")
            .expect("0022 defines the table after receipts")];
    let create = &block[..block.find(") STRICT;").expect("the table ends") + ") STRICT;".len()];
    let triggers = &block[block
        .find("CREATE TRIGGER receipts_no_update")
        .expect("0022 makes receipts immutable")..];
    connection
        .execute_batch(
            "DROP TRIGGER receipts_no_update;
             DROP TRIGGER receipts_no_delete;
             DROP TRIGGER evaluation_cited_runs_no_update;
             DROP TRIGGER evaluation_cited_runs_no_delete;
             ALTER TABLE evaluation_cited_runs RENAME TO evaluation_cited_runs_r46;
             ALTER TABLE receipts RENAME TO receipts_r46;",
        )
        .expect("the R46 shapes come off");
    connection.execute_batch(create).expect("0022's receipts");
    connection.execute_batch(triggers).expect("its triggers");
    let cited = include_str!("../migrations/0028_evaluation_subjects.sql");
    let cited_block = &cited[cited
        .find("CREATE TABLE evaluation_cited_runs (")
        .expect("0028 defines the cited-runs table")..];
    let cited_create =
        &cited_block[..cited_block.find(") STRICT;").expect("it ends") + ") STRICT;".len()];
    connection
        .execute_batch(
            "INSERT INTO receipts SELECT * FROM receipts_r46;
             DROP TABLE receipts_r46;",
        )
        .expect("the rows carry across");
    connection
        .execute_batch(cited_create)
        .expect("0028's child");
    // SLICED, not retyped: SQLite keeps the literal CREATE text, so a retyped trigger differs by
    // its indentation alone and the schema signature refuses. Measured -- the first version of
    // this helper wrote them out and the store would not match a real store at that version.
    // Exactly the two triggers: from the first to the END of the second, because 0028 defines
    // other objects between here and the end of its text.
    let triggers_from = cited_block
        .find("CREATE TRIGGER evaluation_cited_runs_no_update")
        .expect("0028 makes the cited runs immutable");
    let delete_at = cited_block
        .find("CREATE TRIGGER evaluation_cited_runs_no_delete")
        .expect("0028 defines both cited-run triggers");
    let triggers_to = delete_at
        + cited_block[delete_at..]
            .find("END;")
            .expect("the trigger body ends")
        + "END;".len();
    let cited_triggers = &cited_block[triggers_from..triggers_to];
    connection
        .execute_batch(
            "INSERT INTO evaluation_cited_runs SELECT * FROM evaluation_cited_runs_r46;
             DROP TABLE evaluation_cited_runs_r46;",
        )
        .expect("the child's rows carry across");
    connection
        .execute_batch(cited_triggers)
        .expect("the child's rules carry across");
}

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
    // R46 first, while the schema is still whole: renaming a table validates every trigger,
    // and the teardown below drops `artifacts`, which an `evaluations` trigger names.
    restore_pre_0033_receipts(&connection);
    restore_pre_0035_spend(&connection);
    connection
        .execute_batch(
            // Newest migration first. 0029's Outcome manifest store is newest: it keys into
            // `artifacts`, `mission_completions` and `missions`, so it comes off before any of
            // them. 0027's Artifact store is next (the relation first, since it keys into
            // `artifacts`). 0026 and 0025 BOTH rebuilt the closed `events` table --
            // 0026 to add `mission.completed`, 0025 to add two kinds -- so dropping their tables
            // is not enough: the rebuilt definition has to come off too or the store no longer
            // matches schema 23. 0027 rebuilt `events` too; `restore_pre_0025_events` below
            // unwinds every rebuild at once, back to 0023's definition.
            // R58: 0036's chain objects come off first, newest migration first.
            "DROP TRIGGER event_chain_state_no_delete;
             DROP TABLE event_chain_state;
             DROP TRIGGER outcome_publications_no_update;
             DROP TRIGGER outcome_publications_no_delete;
             DROP INDEX outcome_publications_by_mission;
             DROP TABLE outcome_publications;
             DROP TRIGGER outcome_manifests_no_update;
             DROP TRIGGER outcome_manifests_no_delete;
             DROP INDEX outcome_manifests_by_mission;
             DROP TABLE outcome_manifests;
             DROP TRIGGER artifact_supersessions_no_update;
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
    assert_eq!(report.diagnostics.schema_version, 36);

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
fn verified_completion_refuses_a_condition_outcome_projection_that_disagrees_with_its_bytes() {
    let store = TestStore::new("completion-outcome-projection-tamper");
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

    connection
        .execute_batch("DROP TRIGGER mission_completion_conditions_no_update")
        .unwrap();
    connection
        .execute(
            "UPDATE mission_completion_conditions SET outcome = 'not_met'
             WHERE workspace_id = ?1 AND completion_digest = ?2 AND condition_ordinal = 0",
            rusqlite::params![store.workspace_id.to_string(), digest],
        )
        .unwrap();

    let reread = beacond::verified_completion_within(
        &connection,
        store.workspace_id,
        &record.digest().unwrap().value,
    );
    assert!(
        reread.is_err(),
        "verified reader returned a completion whose condition outcome projection contradicts its canonical bytes: {reread:?}"
    );
}

#[test]
fn a_completion_whose_mission_has_no_admission_event_is_refused_by_name() {
    // R21.4 (fable2's O-P0b question 4). The reader's boundary check takes `min(sequence)` over the
    // Mission's `mission.admitted` Events, and `min` over no rows is NULL -- which surfaced as a raw
    // `Store("Invalid column type Null ...")`. A Mission with no admission Event is a corrupt store
    // state, and a corrupt state is refused by NAME, never as a driver error a caller has to parse.
    let store = TestStore::new("completion-mission-unadmitted");
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

    // The admission Event, removed behind the store. `events` has no immutability trigger, so no
    // trigger has to come down to arrange it -- which is part of why the state is reachable.
    let removed = connection
        .execute(
            "DELETE FROM events WHERE kind = 'mission.admitted'
             AND json_extract(payload_json, '$.mission.id') = ?1",
            rusqlite::params![mission.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        removed, 1,
        "the arrangement did not remove the admission Event"
    );

    let reread = beacond::verified_completion_within(&connection, store.workspace_id, &digest);
    assert!(
        matches!(
            reread,
            Err(beacond::CompletionReadRefusal::MissionNotAdmitted)
        ),
        "a Mission with no admission Event was not refused by name: {reread:?}"
    );
}

#[test]
fn a_completion_cannot_use_an_unreadable_mission_admission_event_as_its_boundary() {
    let store = TestStore::new("completion-unreadable-admission");
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

    let changed = connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(payload_json, '$.mission.projectId', ?1)
             WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?2",
            rusqlite::params![Uuid::new_v4().to_string(), mission.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        changed, 1,
        "the arrangement did not corrupt the admission Event"
    );
    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(beacond::SupervisorError::MissionEventMismatch(_))
    ));

    let reread = beacond::verified_completion_within(&connection, store.workspace_id, &digest);
    assert!(
        reread.is_err(),
        "the completion reader accepted a boundary from an admission Event its owning reader refuses"
    );
}

/// The three states, by name. sol1's probe above proves the unreadable Event is refused; this
/// proves it is refused as UNREADABLE -- `MissionAdmissionUnreadable` carrying the Event's own
/// sequence -- and not as the absence R21.4 named. Absent, unreadable and verified are three
/// answers about the same Mission, and a reader that collapsed the middle one into either
/// neighbour would tell a caller the wrong thing about their store.
#[test]
fn an_unreadable_admission_event_is_refused_as_unreadable_not_as_absent() {
    let store = TestStore::new("completion-admission-three-states");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    let admitted = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        admitted,
        vec![beacond::ConditionOutcome::Met],
    );
    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let digest = beacond::record_completion(&mut connection, &record).unwrap();

    // VERIFIED: the honest store reads.
    beacond::verified_completion_within(&connection, store.workspace_id, &digest)
        .expect("an admission Event the Mission owner verifies positions the boundary");

    // UNREADABLE: the same Event with one field of its bytes rewritten.
    connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(payload_json, '$.mission.projectId', ?1)
             WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?2",
            rusqlite::params![Uuid::new_v4().to_string(), mission.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest)
            .expect_err("the Mission owner refuses the Event, so the boundary is unpositioned"),
        beacond::CompletionReadRefusal::MissionAdmissionUnreadable {
            sequence: i64::try_from(admitted).unwrap(),
        }
    );

    // ABSENT: no Event at all is still the other name.
    connection
        .execute(
            "DELETE FROM events WHERE kind = 'mission.admitted'
             AND json_extract(payload_json, '$.mission.id') = ?1",
            rusqlite::params![mission.id.to_string()],
        )
        .unwrap();
    assert_eq!(
        beacond::verified_completion_within(&connection, store.workspace_id, &digest)
            .expect_err("no admission Event"),
        beacond::CompletionReadRefusal::MissionNotAdmitted
    );
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
        Err(beacond::SupervisorError::MissionCompletionStateMismatch { .. })
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
            "INSERT INTO events (
                 workspace_id, kind, payload_json, occurred_at_ms,
                 predecessor_kind, predecessor_digest, link_digest
             )
             VALUES (?1, 'mission.completed', ?2, 9001, 'unchained', NULL, NULL)",
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

/// The pre-0035 spend shapes, for a test that stamps a version below 35.
///
/// 0035 widened `measured_spend`'s canonicalizer CHECK and added `measured_spend_lines`. A store
/// rolled back below 35 must carry the shapes that version actually had, or its schema signature
/// will not match one built from the ladder. Taken from 0025's own text: a rollback that retyped
/// the DDL would be testing the test.
fn restore_pre_0035_spend(connection: &Connection) {
    let source = include_str!("../migrations/0025_spend_and_impact.sql");
    let block = &source[source
        .find("CREATE TABLE measured_spend (")
        .expect("0025 defines the spend table")
        ..source
            .find("-- One Mission impact classification.")
            .expect("0025 defines the table after it")];
    connection
        .execute_batch(
            "CREATE TEMP TABLE spend_pre_0035 AS SELECT * FROM measured_spend;
             DROP TABLE measured_spend_lines;
             DROP TRIGGER measured_spend_no_update;
             DROP TRIGGER measured_spend_no_delete;
             DROP TABLE measured_spend;",
        )
        .expect("the 0035 shapes come off");
    connection.execute_batch(block).expect("0025's spend table");
    connection
        .execute_batch(
            "INSERT INTO measured_spend SELECT * FROM spend_pre_0035;
             DROP TABLE spend_pre_0035;",
        )
        .expect("the rows come back");
}

#[test]
fn public_mission_read_refuses_constraints_changed_after_admission() {
    let store = TestStore::new("public-read-constraints");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();
    assert_eq!(missions.get(admitted.id).unwrap(), admitted);

    // Deliberate corruption of a private, owner-minted fixture after the live service opened.
    // Restore the trigger from its exact stored SQL; only the admission data changes.
    let connection = Connection::open(&store.path).unwrap();
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'missions_no_update'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    connection
        .execute_batch("DROP TRIGGER missions_no_update")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE missions SET constraints_json = '[]' WHERE id = ?1",
                [admitted.id.to_string()],
            )
            .unwrap(),
        1,
        "B11.GET.CONSTRAINTS_FIXTURE_CHANGED"
    );
    connection.execute_batch(&trigger).unwrap();

    let refusal = missions
        .get(admitted.id)
        .expect_err("B11.GET.CHANGED_CONSTRAINTS_REFUSED");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B11.GET.CHANGED_CONSTRAINTS_REFUSED: {refusal:?}"
    );
}

fn public_read_fixture(label: &str) -> (TestStore, MissionService, beacond::MissionView) {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 2);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();
    assert_eq!(missions.get(admitted.id).unwrap(), admitted);
    (store, missions, admitted)
}

#[test]
fn public_mission_read_refuses_a_missing_admission_event() {
    let (store, missions, admitted) = public_read_fixture("public-ownerless");
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM events WHERE kind = 'mission.admitted'
         AND json_extract(payload_json, '$.mission.id') = ?1",
                [admitted.id.to_string()],
            )
            .unwrap(),
        1,
        "B11.GET.OWNER_EVENT_REMOVED"
    );
    let refusal = missions
        .get(admitted.id)
        .expect_err("B11.GET.OWNER_EVENT_REQUIRED");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B11.GET.OWNER_EVENT_REQUIRED: {refusal:?}"
    );
}

#[test]
fn public_mission_read_refuses_two_otherwise_valid_admission_events() {
    let (store, missions, admitted) = public_read_fixture("public-two-owners");
    let connection = Connection::open(&store.path).unwrap();
    // Retag a different real Event, keeping both admission payloads AND timestamps identical.
    // The cardinality is wrong; neither candidate hides behind a different timestamp refusal.
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET kind = 'mission.admitted',
             payload_json = (SELECT payload_json FROM events WHERE kind = 'mission.admitted'
                 AND json_extract(payload_json, '$.mission.id') = ?1),
             occurred_at_ms = (SELECT occurred_at_ms FROM events WHERE kind = 'mission.admitted'
                 AND json_extract(payload_json, '$.mission.id') = ?1)
         WHERE sequence = (SELECT min(sequence) FROM events)",
                [admitted.id.to_string()],
            )
            .unwrap(),
        1,
        "B11.GET.SECOND_OWNER_EVENT_PLANTED"
    );
    let (count, payloads, times): (i64, i64, i64) = connection
        .query_row(
            "SELECT count(*), count(DISTINCT payload_json), count(DISTINCT occurred_at_ms)
         FROM events WHERE kind = 'mission.admitted'
           AND json_extract(payload_json, '$.mission.id') = ?1",
            [admitted.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (count, payloads, times),
        (2, 1, 1),
        "B11.GET.TWO_IDENTICAL_OWNER_CANDIDATES"
    );
    let refusal = missions
        .get(admitted.id)
        .expect_err("B11.GET.EXACTLY_ONE_OWNER_EVENT");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B11.GET.EXACTLY_ONE_OWNER_EVENT: {refusal:?}"
    );
}

#[test]
fn public_mission_read_refuses_a_changed_second_roster_member() {
    let (store, missions, admitted) = public_read_fixture("public-roster");
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(admitted.members.len(), 2, "B11.GET.TWO_ROSTER_MEMBERS");
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET payload_json = json_set(payload_json,
             '$.mission.members[1].admissionConfigurationId', ?2)
         WHERE kind = 'mission.admitted'
           AND json_extract(payload_json, '$.mission.id') = ?1",
                [admitted.id.to_string(), Uuid::new_v4().to_string()],
            )
            .unwrap(),
        1,
        "B11.GET.SECOND_MEMBER_CHANGED"
    );
    let refusal = missions
        .get(admitted.id)
        .expect_err("B11.GET.WHOLE_ROSTER_REQUIRED");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B11.GET.WHOLE_ROSTER_REQUIRED: {refusal:?}"
    );
}

#[test]
fn public_mission_read_refuses_an_unreadable_admission_payload() {
    let (store, missions, admitted) = public_read_fixture("public-unreadable");
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(connection.execute(
        "UPDATE events SET payload_json = json_remove(payload_json, '$.mission.constraints')
         WHERE kind = 'mission.admitted'
           AND json_extract(payload_json, '$.mission.id') = ?1",
        [admitted.id.to_string()],
    ).unwrap(), 1, "B11.GET.OWNER_PAYLOAD_UNREADABLE");
    let refusal = missions
        .get(admitted.id)
        .expect_err("B11.GET.UNREADABLE_IS_NOT_ABSENT");
    assert!(
        matches!(refusal, MissionError::Json(_)),
        "B11.GET.UNREADABLE_IS_NOT_ABSENT: {refusal:?}"
    );
}

#[test]
fn public_mission_read_keeps_absent_and_foreign_workspace_indistinguishable() {
    let store = TestStore::new("public-scope");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();
    let missing = missions.get(Uuid::new_v4()).unwrap_err();
    // Use the same connection and real Mission row, with a different Workspace scope.
    let foreign = MissionService::new(accounts, Uuid::new_v4())
        .get(admitted.id)
        .unwrap_err();
    assert!(
        matches!(missing, MissionError::MissionNotFound),
        "B11.GET.ABSENT_NAME"
    );
    assert!(
        matches!(foreign, MissionError::MissionNotFound),
        "B11.GET.FOREIGN_NAME"
    );
    assert_eq!(
        missing.to_string().as_bytes(),
        foreign.to_string().as_bytes(),
        "B11.GET.SCOPE_REFUSALS_IDENTICAL"
    );
}

// ---------------------------------------------------------------------------------------------
// B12 — the Project Mission listing is owner-verified, in one snapshot
// ---------------------------------------------------------------------------------------------

const B12_FIRST_OUTCOME: &str = "Ship the first reviewed Beacon feature.";
const B12_SECOND_OUTCOME: &str = "Ship the second reviewed Beacon feature.";

/// One Project with two admitted Missions, and the admit returns that produced them.
///
/// The admit returns are a different producer than `list`, so the expected ORDER may be derived
/// from them; the expected CONTENT is held in the constants above.
fn project_with_two_missions(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
) -> (Uuid, Vec<beacond::MissionView>) {
    let prepared = prepare_team(store, accounts, 1);
    let missions = MissionService::new(Arc::clone(accounts), store.workspace_id);
    let mut first = mission_draft(&prepared);
    first.desired_outcome = B12_FIRST_OUTCOME.to_owned();
    let mut second = mission_draft(&prepared);
    second.desired_outcome = B12_SECOND_OUTCOME.to_owned();
    let admitted = vec![
        missions.admit(first).unwrap(),
        missions.admit(second).unwrap(),
    ];
    let mut expected = admitted.clone();
    expected.sort_by(|left, right| {
        (left.admitted_at_ms, left.id).cmp(&(right.admitted_at_ms, right.id))
    });
    (prepared.project.id, expected)
}

/// INTENTIONAL CORRUPTION, after a real owner mint: the Event now says something the row does not.
fn corrupt_admission_event(path: &PathBuf, mission_id: Uuid) {
    let connection = Connection::open(path).unwrap();
    let changed = connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(payload_json, '$.mission.desiredOutcome', ?2)
             WHERE kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?1",
            rusqlite::params![mission_id.to_string(), "an outcome nobody admitted"],
        )
        .unwrap();
    assert_eq!(changed, 1, "exactly one admission Event was rewritten");
}

#[test]
fn a_listed_mission_whose_event_disagrees_refuses_the_whole_list() {
    let store = TestStore::new("b12-first-broken");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);
    corrupt_admission_event(&store.path, expected[0].id);

    let refusal = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect_err("B12.LIST.EVERY_ROW_IS_OWNER_VERIFIED");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B12.LIST.EVERY_ROW_IS_OWNER_VERIFIED: {refusal:?}"
    );
}

#[test]
fn a_broken_last_row_refuses_the_whole_list_rather_than_returning_a_good_prefix() {
    let store = TestStore::new("b12-last-broken");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);
    corrupt_admission_event(&store.path, expected[1].id);

    let refusal = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect_err("B12.LIST.NO_GOOD_PREFIX");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B12.LIST.NO_GOOD_PREFIX: {refusal:?}"
    );
}

#[test]
fn a_healthy_project_lists_every_admission_in_admitted_order() {
    let store = TestStore::new("b12-healthy");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);

    let listed = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect("a healthy Project lists");

    assert_eq!(listed.len(), 2, "B12.LIST.EVERY_ADMISSION_IS_RETURNED");
    // Content against held literals; order against the admit returns, a different producer.
    let outcomes: Vec<&str> = listed
        .iter()
        .map(|mission| mission.desired_outcome.as_str())
        .collect();
    let mut held = [B12_FIRST_OUTCOME, B12_SECOND_OUTCOME];
    held.sort();
    let mut observed = outcomes.clone();
    observed.sort();
    assert_eq!(observed, held.to_vec(), "both admitted outcomes are listed");
    assert_eq!(
        listed.iter().map(|m| m.id).collect::<Vec<_>>(),
        expected.iter().map(|m| m.id).collect::<Vec<_>>(),
        "B12.LIST.ORDER_IS_ADMITTED_AT_THEN_ID"
    );
    assert_eq!(listed, expected, "the full views must survive verification");
}

#[test]
fn a_listed_mission_with_no_admission_event_refuses_the_list() {
    let store = TestStore::new("b12-no-event");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);
    // INTENTIONAL CORRUPTION, after a real owner mint.
    let connection = Connection::open(&store.path).unwrap();
    let removed = connection
        .execute(
            "DELETE FROM events WHERE kind = 'mission.admitted'
             AND json_extract(payload_json, '$.mission.id') = ?1",
            rusqlite::params![expected[1].id.to_string()],
        )
        .unwrap();
    assert_eq!(removed, 1);

    let refusal = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect_err("B12.LIST.UNREADABLE_IS_NOT_ABSENT");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B12.LIST.UNREADABLE_IS_NOT_ABSENT: {refusal:?}"
    );
}

#[test]
fn a_listed_mission_with_two_admission_events_refuses_the_list() {
    let store = TestStore::new("b12-two-events");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);
    // INTENTIONAL CORRUPTION, after a real owner mint: retag the store's first Event so a second
    // row claims this admission, payload and timestamp together so only the count is wrong.
    let connection = Connection::open(&store.path).unwrap();
    let changed = connection
        .execute(
            "UPDATE events
             SET kind = 'mission.admitted',
                 payload_json = (
                     SELECT payload_json FROM events
                     WHERE kind = 'mission.admitted'
                       AND json_extract(payload_json, '$.mission.id') = ?1),
                 occurred_at_ms = (
                     SELECT occurred_at_ms FROM events
                     WHERE kind = 'mission.admitted'
                       AND json_extract(payload_json, '$.mission.id') = ?1)
             WHERE sequence = (SELECT min(sequence) FROM events)",
            rusqlite::params![expected[0].id.to_string()],
        )
        .unwrap();
    assert_eq!(changed, 1);

    let refusal = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect_err("B12.LIST.EXACTLY_ONE_EVENT_PER_LISTED_MISSION");
    assert!(
        matches!(refusal, MissionError::InvalidStore),
        "B12.LIST.EXACTLY_ONE_EVENT_PER_LISTED_MISSION: {refusal:?}"
    );
}

#[test]
fn a_project_with_no_admitted_missions_lists_empty_rather_than_refusing() {
    let store = TestStore::new("b12-empty");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);

    let listed = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(prepared.project.id)
        .expect("an empty Project is not a broken one");
    assert!(listed.is_empty(), "B12.LIST.EMPTY_IS_NOT_A_REFUSAL");
}

#[test]
fn an_absent_project_and_another_workspaces_project_are_both_project_not_found() {
    // ONE store, one connection, one REAL Project row -- read once under its own Workspace and once
    // under a foreign one. A second database would only prove that an id absent from an empty store
    // is absent, which is a different claim (root's B12 fold finding).
    let store = TestStore::new("b12-scope");
    let accounts = enrolled_accounts(&store);
    let (project_id, _expected) = project_with_two_missions(&store, &accounts);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    assert_eq!(
        missions.list(project_id).unwrap().len(),
        2,
        "the held Project id really exists in this store"
    );

    let absent = missions
        .list(Uuid::new_v4())
        .expect_err("B12.LIST.ABSENT_NAME");
    let foreign = MissionService::new(accounts, Uuid::new_v4())
        .list(project_id)
        .expect_err("B12.LIST.FOREIGN_NAME");

    assert!(
        matches!(absent, MissionError::ProjectNotFound),
        "B12.LIST.ABSENT_NAME: {absent:?}"
    );
    assert!(
        matches!(foreign, MissionError::ProjectNotFound),
        "B12.LIST.FOREIGN_NAME: {foreign:?}"
    );
    assert_eq!(
        absent.to_string().as_bytes(),
        foreign.to_string().as_bytes(),
        "B12.LIST.SCOPE_REFUSALS_IDENTICAL"
    );
}

#[test]
fn the_listing_keeps_the_admitted_configuration_after_the_resident_is_reconfigured() {
    let store = TestStore::new("b12-historical");
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, 1);
    let missions = MissionService::new(Arc::clone(&accounts), store.workspace_id);
    let admitted = missions.admit(mission_draft(&prepared)).unwrap();

    let original = &prepared.configurations[0];
    let activation_id = original.binding.as_ref().unwrap().activation_id;
    let configurations =
        ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id);
    let cleared = configurations
        .clear(original.resident_id, original.id)
        .unwrap();
    let replacement = configurations
        .configure(
            original.resident_id,
            Some(cleared.id),
            activation_id,
            vec![BeaconCapability::ArtifactProduce],
        )
        .unwrap();
    assert_ne!(replacement.id, original.id);

    let listed = missions
        .list(prepared.project.id)
        .expect("the Project lists");
    assert_eq!(listed, vec![admitted]);
    assert_eq!(
        listed[0].members[0].admission_configuration_id, original.id,
        "B12.LIST.HISTORY_IS_NOT_REWRITTEN_BY_CURRENT_STATE"
    );
}

#[test]
fn a_listed_mission_with_an_unreadable_admission_payload_refuses_as_json_not_absent() {
    let store = TestStore::new("b12-unreadable-payload");
    let accounts = enrolled_accounts(&store);
    let (project_id, expected) = project_with_two_missions(&store, &accounts);
    // INTENTIONAL CORRUPTION, after a real owner mint: still valid JSON, no longer a Mission.
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET payload_json = json_remove(payload_json, '$.mission.constraints')
                 WHERE kind = 'mission.admitted'
                   AND json_extract(payload_json, '$.mission.id') = ?1",
                [expected[1].id.to_string()],
            )
            .unwrap(),
        1,
        "B12.LIST.OWNER_PAYLOAD_UNREADABLE"
    );

    let refusal = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .list(project_id)
        .expect_err("B12.LIST.UNREADABLE_PAYLOAD_IS_NOT_ABSENT");
    assert!(
        matches!(refusal, MissionError::Json(_)),
        "B12.LIST.UNREADABLE_PAYLOAD_IS_NOT_ABSENT: {refusal:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// C3 — the read-only Mission completion summary, through its public owner
//
// The DTO literal below is written from `C3-interface.md`, not derived from the reader.
// ---------------------------------------------------------------------------------------------

/// One Mission with two declared conditions, admitted, plus a recorded completion.
fn recorded_completion(
    label: &str,
) -> (TestStore, Arc<OperatorAccountService>, beacond::MissionView) {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(
        &store,
        &accounts,
        vec![
            "Gates pass".to_owned(),
            "Evidence filed".to_owned(),
            "A non-author seat reviewed it".to_owned(),
            "The change is reversible".to_owned(),
        ],
    );
    let boundary = admitted_sequence(&store, mission.id);
    let record = completion_for(
        &store,
        &mission,
        boundary,
        vec![
            beacond::ConditionOutcome::Met,
            beacond::ConditionOutcome::NotMet,
            beacond::ConditionOutcome::NotAssessed,
            beacond::ConditionOutcome::AssessmentInconclusive,
        ],
    );
    let mut connection = Connection::open(&store.path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    beacond::record_completion(&mut connection, &record).expect("the owner records it");
    drop(connection);
    (store, accounts, mission)
}

fn completion_reader(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
) -> beacond::MissionCompletionReadService {
    beacond::MissionCompletionReadService::new(Arc::clone(accounts), store.workspace_id)
}

#[test]
fn c3_owner_returns_the_exact_recorded_completion() {
    let (store, accounts, mission) = recorded_completion("c3-recorded");
    let member = &mission.members[0];
    let before = domain_row_counts(&store);

    let view = completion_reader(&store, &accounts)
        .get(mission.id)
        .expect("C3.OWNER.RECORDED_EXACT")
        .expect("C3.OWNER.RECORDED_EXACT: a completion was recorded");

    // The whole DTO, against literals held here.
    assert_eq!(view.mission_id, mission.id, "C3.OWNER.RECORDED_EXACT");
    assert_eq!(view.actor, "operator", "C3.OWNER.RECORDED_EXACT");
    assert_eq!(
        view.accountable_resident_id, member.resident_id,
        "C3.OWNER.RECORDED_EXACT"
    );
    assert_eq!(
        view.accountable_resident_configuration_id, member.admission_configuration_id,
        "C3.OWNER.RECORDED_EXACT: the SELECTED accountable configuration"
    );
    assert_eq!(view.disposition, "concluded", "C3.OWNER.RECORDED_EXACT");
    assert_eq!(
        view.condition_results
            .iter()
            .map(|result| (
                result.ordinal,
                result.outcome.as_str(),
                result.support_count,
                result.note.clone()
            ))
            .collect::<Vec<_>>(),
        vec![
            (0_u32, "met", 0_u32, None),
            (1, "not_met", 0, None),
            (2, "not_assessed", 0, None),
            (3, "assessment_inconclusive", 0, None),
        ],
        "C3.OWNER.RECORDED_EXACT: four outcomes, in declaration order"
    );
    assert_eq!(view.delivered_artifact_count, 0, "C3.OWNER.RECORDED_EXACT");
    assert_eq!(view.unfinished_work_count, 0, "C3.OWNER.RECORDED_EXACT");

    // A read writes nothing.
    assert_eq!(
        domain_row_counts(&store),
        before,
        "C3.OWNER.RECORDED_EXACT: the read changed a domain row or Event"
    );
}

/// Rows and Events a read must not move.
fn domain_row_counts(store: &TestStore) -> (i64, i64, i64) {
    let connection = Connection::open(&store.path).unwrap();
    let one = |sql: &str| {
        connection
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    (
        one("SELECT count(*) FROM mission_completions"),
        one("SELECT count(*) FROM mission_completion_conditions"),
        one("SELECT count(*) FROM events"),
    )
}

#[test]
fn c3_owner_absent_is_not_broken_and_corruption_is_not_absence() {
    let store = TestStore::new("c3-absent");
    let accounts = enrolled_accounts(&store);
    let mission = admit_with_conditions(&store, &accounts, vec!["Gates pass".to_owned()]);
    assert!(
        completion_reader(&store, &accounts)
            .get(mission.id)
            .expect("C3.OWNER.ABSENT_IS_NOT_BROKEN")
            .is_none(),
        "C3.OWNER.ABSENT_IS_NOT_BROKEN: no completion recorded is null, not an error"
    );

    // The positive control first, then the corruption.
    let (store, accounts, mission) = recorded_completion("c3-corrupt");
    assert!(
        completion_reader(&store, &accounts)
            .get(mission.id)
            .expect("C3.OWNER.ABSENT_IS_NOT_BROKEN control")
            .is_some(),
        "C3.OWNER.ABSENT_IS_NOT_BROKEN: the control must read before it is corrupted"
    );
    // INTENTIONAL CORRUPTION after a real owner mint: the stored bytes no longer hash to the digest.
    // The immutability trigger is dropped and recreated BYTE-IDENTICALLY to 0026's own text, so
    // the schema signature startup verifies is unchanged.
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_completions_no_update;
             UPDATE mission_completions SET canonical_bytes = zeroblob(64);",
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER mission_completions_no_update\nBEFORE UPDATE ON mission_completions\nBEGIN\n    SELECT RAISE(ABORT, 'Mission completions are immutable');\nEND;",
        )
        .unwrap();
    drop(connection);
    let refusal = completion_reader(&store, &accounts)
        .get(mission.id)
        .expect_err("C3.OWNER.ABSENT_IS_NOT_BROKEN: corruption is a refusal");
    assert!(
        matches!(
            refusal,
            beacond::MissionCompletionReadError::Unavailable(
                beacond::CompletionUnavailable::CompletionUnverified(_)
            )
        ),
        "C3.OWNER.ABSENT_IS_NOT_BROKEN: {refusal:?}"
    );
}

#[test]
fn c3_owner_an_orphan_completed_event_is_not_absence() {
    let (store, accounts, mission) = recorded_completion("c3-orphan");
    assert!(
        completion_reader(&store, &accounts)
            .get(mission.id)
            .expect("C3.OWNER.ORPHAN_EVENT_IS_NOT_ABSENCE control")
            .is_some(),
        "C3.OWNER.ORPHAN_EVENT_IS_NOT_ABSENCE: assert the successful control first"
    );

    // INTENTIONAL CORRUPTION: the row goes, its owner Event stays.
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_completion_conditions_no_delete;
             DROP TRIGGER mission_completions_no_delete;
             DELETE FROM mission_completion_conditions;
             DELETE FROM mission_completions;",
        )
        .unwrap();
    let retained: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'mission.completed'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retained, 1, "C3.OWNER.ORPHAN_EVENT_IS_NOT_ABSENCE fixture");
    drop(connection);

    let refusal = completion_reader(&store, &accounts)
        .get(mission.id)
        .expect_err("C3.OWNER.ORPHAN_EVENT_IS_NOT_ABSENCE");
    assert!(
        matches!(
            refusal,
            beacond::MissionCompletionReadError::Unavailable(
                beacond::CompletionUnavailable::OrphanCompletedEvent
            )
        ),
        "C3.OWNER.ORPHAN_EVENT_IS_NOT_ABSENCE: {refusal:?}"
    );
}

#[test]
fn c3_owner_absent_and_cross_workspace_are_byte_identical() {
    let (store, accounts, mission) = recorded_completion("c3-scope");
    let reader = completion_reader(&store, &accounts);

    let unknown = reader
        .get(Uuid::new_v4())
        .expect("C3.OWNER.SCOPE_OPACITY: an unknown Mission is not an error");
    let foreign = beacond::MissionCompletionReadService::new(Arc::clone(&accounts), Uuid::new_v4())
        .get(mission.id)
        .expect("C3.OWNER.SCOPE_OPACITY: a foreign Workspace is not an error");

    assert!(unknown.is_none(), "C3.OWNER.SCOPE_OPACITY");
    assert!(
        foreign.is_none(),
        "C3.OWNER.SCOPE_OPACITY: a foreign Workspace cannot leak a completion"
    );
    assert_eq!(
        serde_json::to_string(&unknown).unwrap(),
        serde_json::to_string(&foreign).unwrap(),
        "C3.OWNER.SCOPE_OPACITY: the two answers must be byte-identical"
    );
    assert_eq!(
        serde_json::to_string(&unknown).unwrap(),
        "null",
        "C3.OWNER.SCOPE_OPACITY: absence is JSON null"
    );
}

#[test]
fn c3_owner_keeps_its_authentication_refusal() {
    let (store, accounts, mission) = recorded_completion("c3-auth");
    assert!(
        completion_reader(&store, &accounts)
            .get(mission.id)
            .expect("C3.OWNER.AUTHENTICATION_RETAINED control")
            .is_some()
    );
    accounts.lock().expect("the session locks");

    let refusal = completion_reader(&store, &accounts)
        .get(mission.id)
        .expect_err("C3.OWNER.AUTHENTICATION_RETAINED");
    assert!(
        matches!(
            refusal,
            beacond::MissionCompletionReadError::Account(
                beacond::OperatorAccountError::NotAuthenticated
            )
        ),
        "C3.OWNER.AUTHENTICATION_RETAINED: a locked session is its own refusal, not null and not \
         a generic storage error: {refusal:?}"
    );
}

// Astra-authored C3 Event-bytes probe, added verbatim as a committed regression.
#[test]
fn astra_c3_owner_verifies_the_event_bytes_not_only_its_digest_selector() {
    let (store, accounts, mission) = recorded_completion("c3-event-bytes");
    let reader = completion_reader(&store, &accounts);
    assert!(
        reader
            .get(mission.id)
            .expect("C3.OWNER.EVENT_BYTES_CONTROL")
            .is_some(),
        "C3.OWNER.EVENT_BYTES_CONTROL"
    );
    // INTENTIONAL CORRUPTION after the real owner minted BOTH the record and its Event.
    // Keep Mission/digest selectors and sequence untouched; change only the Event's byte claim.
    let connection = Connection::open(&store.path).unwrap();
    let (sequence, before): (i64, String) = connection
        .query_row(
            "SELECT sequence, payload_json FROM events WHERE workspace_id = ?1
         AND kind = 'mission.completed' AND json_extract(payload_json, '$.missionId') = ?2",
            rusqlite::params![store.workspace_id.to_string(), mission.id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let before: serde_json::Value = serde_json::from_str(&before).unwrap();
    assert_ne!(
        before["canonicalBytesBase64"], "AA==",
        "C3.OWNER.EVENT_BYTES_PLANT_DIFFERS"
    );
    assert_eq!(connection.execute(
        "UPDATE events SET payload_json = json_set(payload_json, '$.canonicalBytesBase64', 'AA==')
         WHERE sequence = ?1", [sequence],
    ).unwrap(), 1, "C3.OWNER.EVENT_BYTES_ONE_PLANT");
    let after: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE sequence = ?1",
            [sequence],
            |row| row.get(0),
        )
        .unwrap();
    let after: serde_json::Value = serde_json::from_str(&after).unwrap();
    assert_eq!(
        after["missionId"], before["missionId"],
        "C3.OWNER.EVENT_BYTES_SELECTOR_UNCHANGED"
    );
    assert_eq!(
        after["digestValue"], before["digestValue"],
        "C3.OWNER.EVENT_BYTES_SELECTOR_UNCHANGED"
    );
    drop(connection);
    let refusal = reader
        .get(mission.id)
        .expect_err("C3.OWNER.EVENT_BYTES_ARE_OWNER_VERIFIED");
    assert!(
        matches!(
            refusal,
            beacond::MissionCompletionReadError::Unavailable(
                beacond::CompletionUnavailable::CompletionUnverified(
                    beacond::CompletionReadRefusal::BytesDisagree
                )
            )
        ),
        "C3.OWNER.EVENT_BYTES_ARE_OWNER_VERIFIED: {refusal:?}"
    );
}

/// C3.WIRE.DTO_EXACT — the DTO across the real socket, not a service call.
///
/// A real `LocalSupervisorServer` is bound to the SAME store the owner fixture minted into, and a
/// real `LocalSupervisorClient` asks for the completion. This proves the command name, the result
/// arm and the serialized shape together; a service-level call proves none of them.
///
/// **Not yet executed:** Blue owned Cargo when this was written.
#[test]
fn c3_wire_returns_the_exact_dto_and_null_over_the_real_socket() {
    let (store, accounts, mission) = recorded_completion("c3-wire");
    let expected =
        beacond::MissionCompletionReadService::new(Arc::clone(&accounts), store.workspace_id)
            .get(mission.id)
            .expect("the owner reads it")
            .expect("a completion was recorded");
    // The owner service and the client must be talking to the same store, so the fixture's own
    // handles are dropped before the server binds it.
    drop(accounts);

    let root = store
        .path
        .parent()
        .unwrap()
        .join(format!("beacon-c3-wire-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    // The supervisor refuses a runtime directory anyone else can read.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = root.join("beacond.sock");
    let token = root.join("client.token");
    beacond::initialize_local_auth_token(&token).unwrap();
    let server = beacond::LocalSupervisorServer::bind(beacond::LocalServerConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
        socket_path: socket.clone(),
        auth_token_path: token.clone(),
        lock_path: root.join("supervisor.lock"),
        local_model_endpoint: beacond::LocalModelEndpoint::ollama_default(),
    })
    .expect("C3.WIRE.DTO_EXACT: the server binds the fixture's store");
    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let thread = thread::spawn(move || server.serve_until(server_running).unwrap());
    let client = beacond::LocalSupervisorClient::connect(socket, token)
        .expect("C3.WIRE.DTO_EXACT: the client connects");
    client
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            beacond::AccessOrigin::SameMachine,
        )
        .expect("C3.WIRE.DTO_EXACT: the existing session semantics are unchanged");

    let over_the_wire = client
        .mission_completion(mission.id)
        .expect("C3.WIRE.DTO_EXACT")
        .expect("C3.WIRE.DTO_EXACT: a completion was recorded");
    assert_eq!(
        over_the_wire, expected,
        "C3.WIRE.DTO_EXACT: the wire must carry the owner's own value"
    );
    // The serialized shape, against the camelCase contract.
    let json: serde_json::Value = serde_json::to_value(&over_the_wire).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "accountableResidentConfigurationId",
            "accountableResidentId",
            "actor",
            "completionId",
            "conditionResults",
            "deliveredArtifactCount",
            "disposition",
            "missionId",
            "unfinishedWorkCount",
        ],
        "C3.WIRE.DTO_EXACT: exactly the C3-interface field set, camelCase"
    );
    let mut condition_keys: Vec<&str> = json["conditionResults"][0]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    condition_keys.sort();
    assert_eq!(
        condition_keys,
        vec!["note", "ordinal", "outcome", "supportCount"],
        "C3.WIRE.DTO_EXACT: the condition shape"
    );

    // An unknown Mission crosses as null, never as a distinct answer.
    let absent = client
        .mission_completion(Uuid::new_v4())
        .expect("C3.WIRE.DTO_EXACT: an unknown Mission is not an error");
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        "null",
        "C3.WIRE.DTO_EXACT: absence is JSON null"
    );

    running.store(false, std::sync::atomic::Ordering::Release);
    let _ = client;
    thread.join().unwrap();
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------------------------
// C4 — the Operator completion owner, through its public service
//
// Every bar below drives the REAL store through `MissionCompletionCommandService`. The C3 bars
// above prove the READER; not one of them calls `preview`, `record` or `get_intent`, so none of
// them is coverage for this owner.
// ---------------------------------------------------------------------------------------------

const C4_CONDITIONS: [&str; 4] = [
    "Gates pass",
    "Evidence filed",
    "A non-author seat reviewed it",
    "The change is reversible",
];

fn c4_owner(
    store: &TestStore,
    accounts: &Arc<OperatorAccountService>,
) -> beacond::MissionCompletionCommandService {
    beacond::MissionCompletionCommandService::new(Arc::clone(accounts), store.workspace_id)
}

/// One admitted Mission with `members` roster entries and the four declared conditions above.
fn c4_admitted(
    label: &str,
    members: usize,
) -> (
    TestStore,
    Arc<OperatorAccountService>,
    PreparedTeam,
    beacond::MissionView,
) {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let prepared = prepare_team(&store, &accounts, members);
    let mut draft = mission_draft(&prepared);
    draft.success_conditions = C4_CONDITIONS
        .iter()
        .map(|condition| (*condition).to_owned())
        .collect();
    let mission = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .admit(draft)
        .unwrap();
    (store, accounts, prepared, mission)
}

/// The same Mission, with a completion minted through the OWNER rather than the store writer.
fn c4_recorded(label: &str) -> (TestStore, Arc<OperatorAccountService>, beacond::MissionView) {
    let (store, accounts, _prepared, mission) = c4_admitted(label, 1);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let token = owner
        .preview(c4_request(&mission, resident))
        .expect("the fixture previews")
        .intent_sha256;
    owner
        .record(c4_request(&mission, resident), &token)
        .expect("the fixture records");
    (store, accounts, mission)
}

fn c4_hex32(byte: u8) -> String {
    format!("{byte:02x}").repeat(32)
}

fn c4_hex(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn c4_uuid(last: u8) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-4000-8000-0000000000{last:02}")).unwrap()
}

/// A complete intent: every citation class, both narrative lists, a recorded spend, notes present
/// and explicitly absent — and the two SETS submitted in the REVERSE of their canonical order.
fn c4_request(
    mission: &beacond::MissionView,
    accountable_resident_id: Uuid,
) -> beacond::CompletionIntentRequest {
    beacond::CompletionIntentRequest {
        mission_id: mission.id,
        accountable_resident_id,
        disposition: "concluded".to_owned(),
        condition_results: vec![
            beacond::ConditionResultDto {
                ordinal: 0,
                outcome: "met".to_owned(),
                support: vec![
                    beacond::SupportRefDto::Ruling(c4_hex32(0x44)),
                    beacond::SupportRefDto::Evaluation(c4_hex32(0x33)),
                    beacond::SupportRefDto::Receipt {
                        run_id: c4_uuid(7),
                        receipt_version: 2,
                        receipt_bytes_sha256: c4_hex32(0x22),
                    },
                    beacond::SupportRefDto::Artifact(c4_hex32(0x11)),
                ],
                note: Some("Every declared gate passed.".to_owned()),
            },
            beacond::ConditionResultDto {
                ordinal: 1,
                outcome: "not_met".to_owned(),
                support: vec![],
                note: None,
            },
            beacond::ConditionResultDto {
                ordinal: 2,
                outcome: "not_assessed".to_owned(),
                support: vec![],
                note: None,
            },
            beacond::ConditionResultDto {
                ordinal: 3,
                outcome: "assessment_inconclusive".to_owned(),
                support: vec![beacond::SupportRefDto::Artifact(c4_hex32(0x77))],
                note: Some("The probe disagreed with itself.".to_owned()),
            },
        ],
        delivered_artifacts: vec![c4_hex32(0x66), c4_hex32(0x55)],
        unfinished_work: vec![beacond::NarrativeItemDto {
            ordinal: 0,
            kind: "deferred_scope".to_owned(),
            condition_ordinal: Some(1),
            summary: "The second condition's probe was deferred.".to_owned(),
        }],
        uncertainty: vec![beacond::NarrativeItemDto {
            ordinal: 0,
            kind: "unmeasured_quantity".to_owned(),
            condition_ordinal: None,
            summary: "Nothing measured model spend for this window.".to_owned(),
        }],
        spend: beacond::SpendDto::Recorded {
            spend_id: c4_uuid(8),
            spend_version: 1,
            spend_sha256: c4_hex32(0x88),
        },
        summary: "One reviewed feature shipped; the second condition is unresolved.".to_owned(),
    }
}

/// The stored completion's own bytes and digest, read outside the owner.
fn c4_stored_bytes(store: &TestStore) -> Vec<(String, Vec<u8>)> {
    let connection = Connection::open(&store.path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT digest_value, canonical_bytes FROM mission_completions ORDER BY digest_value",
        )
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

fn c4_chain_head(store: &TestStore) -> i64 {
    Connection::open(&store.path)
        .unwrap()
        .query_row(
            "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
            [store.workspace_id.to_string()],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn c4_preview_is_read_only_and_complete() {
    let (store, accounts, _prepared, mission) = c4_admitted("c4-preview", 1);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let submitted = c4_request(&mission, resident);
    let before = domain_row_counts(&store);

    let preview = owner
        .preview(c4_request(&mission, resident))
        .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE");

    // The bindings, against values this test holds from the Mission — not from the owner.
    assert_eq!(
        preview.workspace_id, store.workspace_id,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the Workspace is the service's"
    );
    assert_eq!(
        preview.project_id, mission.project_id,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the Project is the Mission's"
    );
    assert_eq!(
        preview.actor, "operator",
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );
    assert_eq!(
        preview.accountable_resident_configuration_id,
        mission.members[0].admission_configuration_id,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the admission roster entry's configuration"
    );
    assert_eq!(
        preview.declared_conditions_sha256,
        c4_hex(beacond::declared_conditions_digest(
            &mission.success_conditions
        )),
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );
    assert_eq!(
        preview.conditions, mission.success_conditions,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the Mission's own declaration"
    );

    // The normalization: the two SETS moved into canonical order and nothing else moved at all.
    assert_eq!(
        preview.request.delivered_artifacts,
        vec![c4_hex32(0x55), c4_hex32(0x66)],
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: delivered Artifacts ascend"
    );
    assert_eq!(
        preview.request.condition_results[0].support,
        vec![
            beacond::SupportRefDto::Artifact(c4_hex32(0x11)),
            beacond::SupportRefDto::Receipt {
                run_id: c4_uuid(7),
                receipt_version: 2,
                receipt_bytes_sha256: c4_hex32(0x22),
            },
            beacond::SupportRefDto::Evaluation(c4_hex32(0x33)),
            beacond::SupportRefDto::Ruling(c4_hex32(0x44)),
        ],
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: support in the codec's own (kind, key) order"
    );
    assert_eq!(
        preview
            .request
            .condition_results
            .iter()
            .map(|result| (result.ordinal, result.outcome.clone(), result.note.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                0_u32,
                "met".to_owned(),
                Some("Every declared gate passed.".to_owned())
            ),
            (1, "not_met".to_owned(), None),
            (2, "not_assessed".to_owned(), None),
            (
                3,
                "assessment_inconclusive".to_owned(),
                Some("The probe disagreed with itself.".to_owned())
            ),
        ],
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: four explicit outcomes, sequence preserved"
    );
    assert_eq!(
        preview.request.unfinished_work, submitted.unfinished_work,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: a narrative SEQUENCE is not a set"
    );
    assert_eq!(
        preview.request.uncertainty, submitted.uncertainty,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );
    assert_eq!(
        preview.request.spend, submitted.spend,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );
    assert_eq!(
        preview.request.summary, submitted.summary,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: text is preserved character for character"
    );
    assert_eq!(
        (
            preview.request.mission_id,
            preview.request.accountable_resident_id,
            preview.request.disposition.clone()
        ),
        (mission.id, resident, "concluded".to_owned()),
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );

    // A preview writes nothing at all.
    assert_eq!(
        domain_row_counts(&store),
        before,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: a preview moved a row or an Event"
    );
    assert!(
        owner
            .get_intent(mission.id)
            .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE")
            .is_none(),
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: a preview is not a completion"
    );

    // A SET permutation is the SAME intent; a repeated entry is refused.
    let mut permuted = c4_request(&mission, resident);
    permuted.condition_results[0].support.reverse();
    permuted.delivered_artifacts.reverse();
    assert_eq!(
        owner
            .preview(permuted)
            .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE")
            .intent_sha256,
        preview.intent_sha256,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: a set order is not part of the intent"
    );
    let mut duplicated = c4_request(&mission, resident);
    duplicated.condition_results[0]
        .support
        .push(beacond::SupportRefDto::Artifact(c4_hex32(0x11)));
    let refusal = owner
        .preview(duplicated)
        .expect_err("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: a repeated citation refuses");
    assert_eq!(
        refusal.name(),
        "DuplicateSetEntry",
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: {refusal:?}"
    );

    // And the reviewed token is exactly the one the later `get` returns.
    let recorded = owner
        .record(c4_request(&mission, resident), &preview.intent_sha256)
        .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the reviewed intent records");
    assert_eq!(
        recorded.intent_sha256, preview.intent_sha256,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE"
    );
    assert_eq!(
        owner
            .get_intent(mission.id)
            .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE")
            .expect("C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: it was recorded")
            .intent_sha256,
        preview.intent_sha256,
        "C4.OWNER.PREVIEW_IS_READ_ONLY_AND_COMPLETE: the stored record answers the reviewed token"
    );
}

#[test]
fn c4_record_is_one_atomic_transaction() {
    let (store, accounts, _prepared, mission) = c4_admitted("c4-atomic", 1);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let token = owner
        .preview(c4_request(&mission, resident))
        .expect("C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION")
        .intent_sha256;
    let before = domain_row_counts(&store);

    // A FORCED failure INSIDE the writer: the completion row inserts, its condition projections
    // cannot. Planted on a second connection after the account service opened its own — nothing
    // re-verifies the schema signature against it — and removed before the store is reopened.
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER c4_forced_write_failure
             BEFORE INSERT ON mission_completion_conditions
             BEGIN SELECT RAISE(ABORT, 'c4 forced write failure'); END;",
        )
        .unwrap();
    drop(connection);

    let refusal = owner
        .record(c4_request(&mission, resident), &token)
        .expect_err("C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: the forced failure must refuse");
    assert!(
        matches!(
            refusal,
            beacond::CompletionCommandError::Unavailable(
                beacond::CompletionCommandUnavailable::Write(
                    beacond::CompletionReadRefusal::Store(_)
                )
            )
        ),
        "C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: {refusal:?}"
    );
    assert_eq!(
        domain_row_counts(&store),
        before,
        "C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: a failed write left a row, a projection or an \
         Event behind"
    );

    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch("DROP TRIGGER c4_forced_write_failure;")
        .unwrap();
    drop(connection);

    // Reopened from disk, through C3's reader: the rollback survives the process, not just the
    // handle that made it.
    let reopened = Arc::new(OperatorAccountService::open(&store.path).unwrap());
    reopened
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            beacond::AccessOrigin::SameMachine,
        )
        .expect("C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: the store reopens");
    assert!(
        beacond::MissionCompletionReadService::new(Arc::clone(&reopened), store.workspace_id)
            .get(mission.id)
            .expect("C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION")
            .is_none(),
        "C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: the reopened store must show no completion"
    );

    // The same intent, unblocked: one row, its four projections and exactly one owner Event.
    c4_owner(&store, &reopened)
        .record(c4_request(&mission, resident), &token)
        .expect("C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: the unblocked write records");
    assert_eq!(
        domain_row_counts(&store),
        (before.0 + 1, before.1 + 4, before.2 + 1),
        "C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: one completion, four conditions, one Event"
    );
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE workspace_id = ?1 AND kind = 'mission.completed'
                   AND json_extract(payload_json, '$.missionId') = ?2",
                rusqlite::params![store.workspace_id.to_string(), mission.id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "C4.OWNER.RECORD_IS_ONE_ATOMIC_TRANSACTION: exactly one owner Event names this Mission"
    );
}

#[test]
fn c4_a_stale_intent_writes_nothing() {
    let (store, accounts, _prepared, mission) = c4_admitted("c4-stale", 2);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let reviewed = owner
        .preview(c4_request(&mission, resident))
        .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING")
        .intent_sha256;
    let before = domain_row_counts(&store);

    // A changed user field is a different intent.
    let mut changed = c4_request(&mission, resident);
    changed.summary = "One reviewed feature shipped; the second condition is resolved.".to_owned();
    let refusal = owner
        .record(changed, &reviewed)
        .expect_err("C4.OWNER.STALE_INTENT_WRITES_NOTHING: changed text must refuse");
    assert_eq!(
        refusal.name(),
        "IntentChanged",
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING: {refusal:?}"
    );

    // A changed stable BINDING is a different intent: the other roster member is another
    // accountability, not a detail.
    let other = mission.members[1].resident_id;
    assert_ne!(
        other, resident,
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING fixture"
    );
    let refusal = owner
        .record(c4_request(&mission, other), &reviewed)
        .expect_err("C4.OWNER.STALE_INTENT_WRITES_NOTHING: a changed binding must refuse");
    assert_eq!(
        refusal.name(),
        "IntentChanged",
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING: {refusal:?}"
    );
    assert_eq!(
        domain_row_counts(&store),
        before,
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING: a stale intent wrote something"
    );

    // The three TRANSIENT owner values move on their own, and a review survives them: an unrelated
    // Event advances the chain head this record would report at, and the clock advances by itself.
    let head_before = c4_chain_head(&store);
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    let current = lifecycle
        .list()
        .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING fixture")
        .into_iter()
        .find(|resident| resident.id == other)
        .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING fixture");
    lifecycle
        .suspend(other, current.latest_decision_id)
        .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING: an unrelated owner Event");
    let head_after = c4_chain_head(&store);
    assert!(
        head_after > head_before,
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING: the boundary must actually have moved \
         ({head_before} -> {head_after})"
    );
    owner
        .record(c4_request(&mission, resident), &reviewed)
        .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING: a moved boundary is not a changed intent");
    assert_eq!(
        owner
            .get_intent(mission.id)
            .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING")
            .expect("C4.OWNER.STALE_INTENT_WRITES_NOTHING")
            .intent_sha256,
        reviewed,
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING"
    );
    let connection = Connection::open(&store.path).unwrap();
    let boundary: i64 = connection
        .query_row(
            "SELECT boundary_event_seq FROM mission_completions",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        boundary, head_after,
        "C4.OWNER.STALE_INTENT_WRITES_NOTHING: the record reports at the CURRENT cut, not the \
         reviewed one"
    );
}

#[test]
fn c4_a_second_intent_never_overwrites() {
    let (store, accounts, _prepared, mission) = c4_admitted("c4-conflict", 1);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let first = owner
        .preview(c4_request(&mission, resident))
        .expect("C4.OWNER.CONFLICT_NEVER_OVERWRITES")
        .intent_sha256;
    owner
        .record(c4_request(&mission, resident), &first)
        .expect("C4.OWNER.CONFLICT_NEVER_OVERWRITES");
    let stored = c4_stored_bytes(&store);
    let counts = domain_row_counts(&store);

    // A genuinely different completion of the same Mission.
    let mut different = c4_request(&mission, resident);
    different.disposition = "abandoned".to_owned();
    let second = owner
        .preview(different.clone())
        .expect("C4.OWNER.CONFLICT_NEVER_OVERWRITES: preview stays read-only after a record")
        .intent_sha256;
    assert_ne!(
        second, first,
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the fixture must actually differ"
    );
    let refusal = owner
        .record(different, &second)
        .expect_err("C4.OWNER.CONFLICT_NEVER_OVERWRITES: a second completion must refuse");
    assert_eq!(
        refusal.name(),
        "AlreadyRecorded",
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: {refusal:?}"
    );

    assert_eq!(
        c4_stored_bytes(&store),
        stored,
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the stored bytes moved"
    );
    assert_eq!(
        domain_row_counts(&store),
        counts,
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the refused write left something behind"
    );
    assert_eq!(
        owner
            .get_intent(mission.id)
            .expect("C4.OWNER.CONFLICT_NEVER_OVERWRITES")
            .expect("C4.OWNER.CONFLICT_NEVER_OVERWRITES")
            .intent_sha256,
        first,
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the ORIGINAL intent still answers"
    );

    // An equal token is content equality and NOTHING else. The stored record carries a fresh
    // identity and a boundary this review never saw, and still answers the same token — so the
    // token cannot be read as evidence that this particular write was the reviewed one.
    let connection = Connection::open(&store.path).unwrap();
    let (completion_id, boundary): (String, i64) = connection
        .query_row(
            "SELECT completion_id, boundary_event_seq FROM mission_completions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_ne!(
        completion_id,
        Uuid::nil().to_string(),
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the identity is the owner's, not the token's sentinel"
    );
    assert!(
        boundary > 0,
        "C4.OWNER.CONFLICT_NEVER_OVERWRITES: the boundary is the owner's, not the token's sentinel"
    );
}

#[test]
fn c4_absence_scope_and_broken_evidence_stay_distinct() {
    // Genuine absence, an unknown Mission and a foreign Workspace are one answer.
    let (store, accounts, _prepared, mission) = c4_admitted("c4-absent", 1);
    let owner = c4_owner(&store, &accounts);
    let absent = owner
        .get_intent(mission.id)
        .expect("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT");
    let unknown = owner
        .get_intent(Uuid::new_v4())
        .expect("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT");
    let foreign =
        beacond::MissionCompletionCommandService::new(Arc::clone(&accounts), Uuid::new_v4())
            .get_intent(mission.id)
            .expect("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT");
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        "null",
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: absence is JSON null"
    );
    assert_eq!(
        serde_json::to_string(&unknown).unwrap(),
        serde_json::to_string(&foreign).unwrap(),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: scope must not be distinguishable"
    );
    assert_eq!(
        serde_json::to_string(&absent).unwrap(),
        serde_json::to_string(&foreign).unwrap(),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT"
    );

    // Corrupt canonical bytes. INTENTIONAL, after a real owner mint; the immutability trigger is
    // dropped and recreated BYTE-IDENTICALLY to 0026's own text.
    let (store, accounts, mission) = c4_recorded("c4-corrupt");
    let owner = c4_owner(&store, &accounts);
    assert!(
        owner
            .get_intent(mission.id)
            .expect("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT control")
            .is_some(),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: assert the control before corrupting it"
    );
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_completions_no_update;
             UPDATE mission_completions SET canonical_bytes = zeroblob(64);",
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER mission_completions_no_update\nBEFORE UPDATE ON mission_completions\nBEGIN\n    SELECT RAISE(ABORT, 'Mission completions are immutable');\nEND;",
        )
        .unwrap();
    drop(connection);
    let refusal = owner
        .get_intent(mission.id)
        .expect_err("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: corruption is not absence");
    assert!(
        matches!(
            refusal,
            beacond::CompletionCommandError::Unavailable(
                beacond::CompletionCommandUnavailable::Read(
                    beacond::MissionCompletionReadError::Unavailable(
                        beacond::CompletionUnavailable::CompletionUnverified(_)
                    )
                )
            )
        ),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: {refusal:?}"
    );

    // A surviving owner Event with no row is positive evidence, never null.
    let (store, accounts, mission) = c4_recorded("c4-orphan");
    let owner = c4_owner(&store, &accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch(
            "DROP TRIGGER mission_completion_conditions_no_delete;
             DROP TRIGGER mission_completions_no_delete;
             DELETE FROM mission_completion_conditions;
             DELETE FROM mission_completions;",
        )
        .unwrap();
    drop(connection);
    let refusal = owner
        .get_intent(mission.id)
        .expect_err("C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: an orphan is not absence");
    assert!(
        matches!(
            refusal,
            beacond::CompletionCommandError::Unavailable(
                beacond::CompletionCommandUnavailable::Read(
                    beacond::MissionCompletionReadError::Unavailable(
                        beacond::CompletionUnavailable::OrphanCompletedEvent
                    )
                )
            )
        ),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: {refusal:?}"
    );

    // The Event's byte claim, with its selectors untouched.
    let (store, accounts, mission) = c4_recorded("c4-event-bytes");
    let owner = c4_owner(&store, &accounts);
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE events SET payload_json =
                   json_set(payload_json, '$.canonicalBytesBase64', 'AA==')
                 WHERE workspace_id = ?1 AND kind = 'mission.completed'
                   AND json_extract(payload_json, '$.missionId') = ?2",
                rusqlite::params![store.workspace_id.to_string(), mission.id.to_string()],
            )
            .unwrap(),
        1,
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: exactly one Event is planted"
    );
    drop(connection);
    let refusal = owner.get_intent(mission.id).expect_err(
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: an Event byte mismatch is not absence",
    );
    assert!(
        matches!(
            refusal,
            beacond::CompletionCommandError::Unavailable(
                beacond::CompletionCommandUnavailable::Read(
                    beacond::MissionCompletionReadError::Unavailable(
                        beacond::CompletionUnavailable::CompletionUnverified(
                            beacond::CompletionReadRefusal::BytesDisagree
                        )
                    )
                )
            )
        ),
        "C4.OWNER.ABSENT_SCOPE_AND_BROKEN_STAY_DISTINCT: {refusal:?}"
    );
}

#[test]
fn c4_assessment_is_explicit_and_the_roster_is_historical() {
    let (store, accounts, prepared, mission) = c4_admitted("c4-roster", 2);
    let owner = c4_owner(&store, &accounts);
    // NOT the first member and NOT a sole member: the Operator chose this one.
    let chosen = mission.members[1].clone();

    // The chosen Resident is reconfigured AND suspended AFTER admission.
    let previous = prepared
        .configurations
        .iter()
        .find(|configuration| configuration.resident_id == chosen.resident_id)
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER fixture");
    let replacement = ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id)
        .configure(
            chosen.resident_id,
            Some(previous.id),
            previous
                .binding
                .as_ref()
                .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER fixture")
                .activation_id,
            vec![BeaconCapability::WorkCreateMission],
        )
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: the Resident is reconfigured");
    assert_ne!(
        replacement.id, chosen.admission_configuration_id,
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: the fixture must actually move"
    );
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    let current = lifecycle
        .list()
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER fixture")
        .into_iter()
        .find(|resident| resident.id == chosen.resident_id)
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER fixture");
    lifecycle
        .suspend(chosen.resident_id, current.latest_decision_id)
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: and is then suspended");

    let preview = owner
        .preview(c4_request(&mission, chosen.resident_id))
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER");
    assert_eq!(
        preview.accountable_resident_configuration_id, chosen.admission_configuration_id,
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: the ADMISSION configuration"
    );
    assert_ne!(
        preview.accountable_resident_configuration_id, replacement.id,
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: never the current configuration"
    );

    let before = domain_row_counts(&store);

    // A Resident who was never admitted to this Mission.
    for refusal in [
        owner
            .preview(c4_request(&mission, Uuid::new_v4()))
            .expect_err("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: preview"),
        owner
            .record(c4_request(&mission, Uuid::new_v4()), &preview.intent_sha256)
            .expect_err("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: record"),
    ] {
        assert_eq!(
            refusal.name(),
            "ResidentNotInAdmissionRoster",
            "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: {refusal:?}"
        );
    }

    // A missing condition, and a duplicated one. Nothing is inferred and nothing defaults.
    let mut missing = c4_request(&mission, chosen.resident_id);
    missing.condition_results.pop();
    let mut duplicated = c4_request(&mission, chosen.resident_id);
    duplicated.condition_results[3].ordinal = 2;
    for refusal in [
        owner
            .record(missing, &preview.intent_sha256)
            .expect_err("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: a missing condition"),
        owner.record(duplicated, &preview.intent_sha256).expect_err(
            "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: a duplicated condition",
        ),
    ] {
        assert_eq!(
            refusal.name(),
            "ConditionCoverageIncomplete",
            "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: {refusal:?}"
        );
    }
    assert_eq!(
        domain_row_counts(&store),
        before,
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: a refused request wrote something"
    );

    // All four outcomes survive the round trip as chosen, against the HISTORICAL configuration.
    owner
        .record(
            c4_request(&mission, chosen.resident_id),
            &preview.intent_sha256,
        )
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER");
    let view =
        beacond::MissionCompletionReadService::new(Arc::clone(&accounts), store.workspace_id)
            .get(mission.id)
            .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER")
            .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER");
    assert_eq!(
        view.condition_results
            .iter()
            .map(|result| result.outcome.as_str())
            .collect::<Vec<_>>(),
        ["met", "not_met", "not_assessed", "assessment_inconclusive"],
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: four explicit human states"
    );
    assert_eq!(
        view.disposition, "concluded",
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER"
    );
    assert_eq!(
        view.accountable_resident_configuration_id, chosen.admission_configuration_id,
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: the stored record kept the history"
    );

    // Both dispositions are reachable human choices.
    let (other_store, other_accounts, _other_prepared, other_mission) =
        c4_admitted("c4-abandoned", 1);
    let other_owner = c4_owner(&other_store, &other_accounts);
    let mut abandoned = c4_request(&other_mission, other_mission.members[0].resident_id);
    abandoned.disposition = "abandoned".to_owned();
    let token = other_owner
        .preview(abandoned.clone())
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER")
        .intent_sha256;
    other_owner
        .record(abandoned, &token)
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER");
    assert_eq!(
        beacond::MissionCompletionReadService::new(
            Arc::clone(&other_accounts),
            other_store.workspace_id
        )
        .get(other_mission.id)
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER")
        .expect("C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER")
        .disposition,
        "abandoned",
        "C4.OWNER.EXPLICIT_ASSESSMENT_AND_HISTORICAL_ROSTER: abandoned is a recordable choice"
    );
}

#[test]
fn c4_the_owner_keeps_its_authentication_refusal() {
    let (store, accounts, _prepared, mission) = c4_admitted("c4-auth", 1);
    let owner = c4_owner(&store, &accounts);
    let resident = mission.members[0].resident_id;
    let token = owner
        .preview(c4_request(&mission, resident))
        .expect("C4.OWNER.AUTHENTICATION_RETAINED control")
        .intent_sha256;
    accounts.lock().expect("the session locks");

    for refusal in [
        owner
            .preview(c4_request(&mission, resident))
            .expect_err("C4.OWNER.AUTHENTICATION_RETAINED: preview"),
        owner
            .record(c4_request(&mission, resident), &token)
            .expect_err("C4.OWNER.AUTHENTICATION_RETAINED: record"),
        owner
            .get_intent(mission.id)
            .expect_err("C4.OWNER.AUTHENTICATION_RETAINED: get_intent"),
    ] {
        assert!(
            matches!(
                refusal,
                beacond::CompletionCommandError::Account(
                    beacond::OperatorAccountError::NotAuthenticated
                )
            ),
            "C4.OWNER.AUTHENTICATION_RETAINED: a locked session keeps its own meaning: {refusal:?}"
        );
    }
    assert_eq!(
        domain_row_counts(&store),
        (0, 0, domain_row_counts(&store).2),
        "C4.OWNER.AUTHENTICATION_RETAINED: a locked session wrote a completion"
    );
}
