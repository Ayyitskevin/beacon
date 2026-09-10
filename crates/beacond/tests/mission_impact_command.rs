//! C5 — the Operator classification owner, through its public service.
//!
//! Every record here is minted through an existing public service (Project, Policy, Resident,
//! model activation, Team, Mission). Raw SQL appears only for deliberately named faults, and only
//! after a real mint.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use beacond::{
    AutonomyProfile, BeaconCapability, ClassificationIntentRequest, LocalModelActivationService,
    LocalModelEndpoint, MissionDraft, MissionImpactCommandService, MissionMemberDraft,
    MissionService, MissionView, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService, ProjectService,
    ProjectView, ProviderFeature, ResidentConfiguration, ResidentConfigurationService,
    ResidentLifecycleService, SecretString, StarterOrganizationService, StarterResidentDraft,
    StarterRole, StartupConfig, SupersedesDto, Supervisor, TeamDraft, TeamLeadAmbiguityRule,
    TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft,
    TeamService, TrackRecordEvidenceKind, TrackRecordRequirement,
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
            "beacond-impact-command-{label}-{}.sqlite3",
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

    fn connect(&self) -> Connection {
        Connection::open(&self.path).unwrap()
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

fn project_draft() -> ProjectDraft {
    ProjectDraft {
        name: "Beacon Alpha".to_owned(),
        charter: ProjectCharterDraft {
            purpose: "Build a trustworthy local software product.".to_owned(),
            desired_outcomes: vec!["Ship a verified vertical slice".to_owned()],
            constraints: vec!["Keep Workspace data local".to_owned()],
            prohibitions: vec!["Do not deploy publicly".to_owned()],
            autonomy_profile: AutonomyProfile::Supervised,
        },
    }
}

fn policy_draft() -> ProjectPolicyDraft {
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
    }
}

/// The three-number fixture.
///
/// Charter 1 -> Policy revision 1 (cites charter 1) -> Charter 2 -> Policy revision 2 (cites
/// charter 2) -> Charter 3. The Mission is admitted last, so it binds charter **3**.
///
/// Three distinct numbers is the point: the correct citation is `(revision 2, charter 2)`, a mutant
/// copying the Project's CURRENT charter would cite 3, and a mutant copying the MISSION's charter
/// would also cite 3. My first draft of this fixture used two revisions, where the Policy's charter
/// and the Project's current charter are both 2 -- a witness whose two arms produce the same number
/// and therefore kills nothing (root's spec read).
struct Fixture {
    store: TestStore,
    accounts: Arc<OperatorAccountService>,
    project: ProjectView,
    mission: MissionView,
    configurations: Vec<ResidentConfiguration>,
}

fn fixture(label: &str) -> Fixture {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let projects = ProjectService::new(Arc::clone(&accounts), store.workspace_id);
    let policies = ProjectPolicyService::new(Arc::clone(&accounts), store.workspace_id);

    let project = projects.create(project_draft()).unwrap();
    assert_eq!(project.charter.revision, 1, "C5 fixture: charter 1");
    let first = policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();
    assert_eq!(
        (first.revision, first.charter_revision),
        (1, 1),
        "C5 fixture: Policy 1 cites charter 1"
    );
    let project = projects
        .revise_charter(project.id, 1, project_draft().charter)
        .unwrap();
    assert_eq!(project.charter.revision, 2, "C5 fixture: charter 2");
    let second = policies
        .publish(project.id, 2, Some(1), policy_draft())
        .unwrap();
    assert_eq!(
        (second.revision, second.charter_revision),
        (2, 2),
        "C5 fixture: Policy 2 cites charter 2"
    );
    let project = projects
        .revise_charter(project.id, 2, project_draft().charter)
        .unwrap();
    assert_eq!(
        project.charter.revision, 3,
        "C5 fixture: charter 3, so the Policy's charter and the Project's current charter differ"
    );

    let residents = StarterOrganizationService::new(Arc::clone(&accounts), store.workspace_id)
        .accept(
            1,
            vec![StarterResidentDraft {
                role: StarterRole::Lead,
                display_name: None,
            }],
        )
        .unwrap()
        .residents;
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), store.workspace_id);
    for resident in &residents {
        lifecycle.activate(resident.id, None).unwrap();
    }
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
    let configurations = residents
        .iter()
        .map(|resident| {
            ResidentConfigurationService::new(Arc::clone(&accounts), store.workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    vec![BeaconCapability::WorkCreateMission],
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let team = TeamService::new(Arc::clone(&accounts), store.workspace_id)
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
    let mission = MissionService::new(Arc::clone(&accounts), store.workspace_id)
        .admit(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: team.id,
            desired_outcome: "Ship one reviewed Beacon feature.".to_owned(),
            success_conditions: vec!["Relevant gates pass".to_owned()],
            constraints: vec!["Keep the change reversible".to_owned()],
            prohibitions: vec!["Do not deploy".to_owned()],
            members: configurations
                .iter()
                .map(|configuration| MissionMemberDraft {
                    resident_id: configuration.resident_id,
                    expected_configuration_id: configuration.id,
                })
                .collect(),
        })
        .unwrap();
    assert_eq!(
        mission.charter_revision, 3,
        "C5 fixture: the Mission binds charter 3, a third distinct number"
    );

    Fixture {
        store,
        accounts,
        project,
        mission,
        configurations,
    }
}

impl Fixture {
    fn owner(&self) -> MissionImpactCommandService {
        MissionImpactCommandService::new(Arc::clone(&self.accounts), self.store.workspace_id)
    }

    /// Classifications, and every Event. A refusal must move neither.
    fn counts(&self) -> (i64, i64) {
        let connection = self.store.connect();
        let one = |sql: &str| {
            connection
                .query_row(sql, [], |row| row.get::<_, i64>(0))
                .unwrap()
        };
        (
            one("SELECT count(*) FROM mission_impact_classifications"),
            one("SELECT count(*) FROM events"),
        )
    }
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

fn high_declaration(mission_id: Uuid) -> ClassificationIntentRequest {
    ClassificationIntentRequest {
        mission_id,
        impact: "high".to_owned(),
        reason: "It changes how completions reach the store.".to_owned(),
        expected_policy_revision: Some(2),
        supersedes: SupersedesDto::None,
    }
}

/// C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT — the first positive, on one real store.
///
/// The Operator says two things; the owner derives eight. Every expectation below is held here as a
/// literal or read from a service that is not the one under test.
#[test]
fn c5_the_first_classification_records_the_operator_judgment_and_the_owner_bindings() {
    let fixture = fixture("first-positive");
    let before = fixture.counts();
    let opened_at = now_ms();

    let view = fixture
        .owner()
        .classify(high_declaration(fixture.mission.id))
        .expect("C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT");

    // The Operator's judgment, unchanged.
    assert_eq!(
        (view.impact.as_str(), view.reason.as_str()),
        ("high", "It changes how completions reach the store."),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: the judgment is the Operator's"
    );

    // The owner's bindings.
    assert_eq!(
        view.mission_id, fixture.mission.id,
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT"
    );
    assert_ne!(
        view.classification_id,
        Uuid::nil(),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: a real reference identity"
    );
    assert_eq!(
        view.version, 1,
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: the ladder starts at one"
    );
    assert_eq!(
        view.basis, "operator_declared",
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: v1 has one basis and it is not an input"
    );
    assert_eq!(
        view.classified_by, "operator",
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT"
    );
    assert_eq!(
        view.supersedes,
        SupersedesDto::None,
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: a first classification replaces nothing"
    );

    // THE three-number assertion: the Policy revision's OWN charter, not the Project's current one
    // (3) and not the Mission's (3).
    assert_eq!(
        (
            view.policy.project_id,
            view.policy.policy_revision,
            view.policy.charter_revision
        ),
        (fixture.project.id, 2, 2),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: the citation is the Policy revision's own, \
         while the Project is at charter {} and the Mission binds {}",
        fixture.project.charter.revision,
        fixture.mission.charter_revision
    );

    // The clock is the owner's, inside this test's own window.
    assert!(
        (opened_at..=now_ms()).contains(&view.classified_at_ms),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: {} is outside [{opened_at}, now]",
        view.classified_at_ms
    );

    // Content identity, whole.
    assert_eq!(
        (view.digest.algorithm.as_str(), view.digest.canonicalizer.as_str()),
        ("sha256", "beacon.mission-impact.v1"),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT"
    );
    assert!(
        view.digest.value.len() == 64
            && view
                .digest
                .value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: {}",
        view.digest.value
    );

    // Exactly one row and exactly one Event.
    assert_eq!(
        fixture.counts(),
        (before.0 + 1, before.1 + 1),
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: one row, one owning Event"
    );
    let connection = fixture.store.connect();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE workspace_id = ?1
                   AND kind = 'mission.impact_classified'
                   AND json_extract(payload_json, '$.digest.value') = ?2",
                rusqlite::params![fixture.store.workspace_id.to_string(), view.digest.value],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1,
        "C5.OWNER.DERIVES_EVERYTHING_BUT_THE_JUDGMENT: the Event names this exact record"
    );
    assert!(
        !fixture.configurations.is_empty(),
        "C5 fixture: a roster exists"
    );
}
