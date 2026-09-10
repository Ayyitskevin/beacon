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
    MissionService, MissionView, OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft,
    ProjectDraft, ProjectPolicyDraft, ProjectPolicyService, ProjectService, ProjectView,
    ProviderFeature, ResidentConfiguration, ResidentConfigurationService, ResidentLifecycleService,
    SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig,
    SupersedesDto, Supervisor, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement,
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

/// A charter draft that actually differs from the previous one.
///
/// `revise_charter` refuses an unchanged charter (`ProjectError::UnchangedCharter`), which is what
/// the first run of this fixture hit: reusing `project_draft().charter` verbatim is not a revision.
fn charter_draft(revision: u32) -> ProjectCharterDraft {
    ProjectCharterDraft {
        purpose: format!("Build a trustworthy local software product, take {revision}."),
        ..project_draft().charter
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
    fixture_with(label, true)
}

/// `publish_policies: false` leaves the Project with no published Policy revision at all, which is
/// its own answer and not a failure to read one.
fn fixture_with(label: &str, publish_policies: bool) -> Fixture {
    let store = TestStore::new(label);
    let accounts = enrolled_accounts(&store);
    let projects = ProjectService::new(Arc::clone(&accounts), store.workspace_id);
    let policies = ProjectPolicyService::new(Arc::clone(&accounts), store.workspace_id);

    let project = projects.create(project_draft()).unwrap();
    assert_eq!(project.charter.revision, 1, "C5 fixture: charter 1");
    if publish_policies {
        let first = policies
            .publish(project.id, 1, None, policy_draft())
            .unwrap();
        assert_eq!(
            (first.revision, first.charter_revision),
            (1, 1),
            "C5 fixture: Policy 1 cites charter 1"
        );
    }
    let project = projects
        .revise_charter(project.id, 1, charter_draft(2))
        .unwrap();
    assert_eq!(project.charter.revision, 2, "C5 fixture: charter 2");
    if publish_policies {
        let second = policies
            .publish(project.id, 2, Some(1), policy_draft())
            .unwrap();
        assert_eq!(
            (second.revision, second.charter_revision),
            (2, 2),
            "C5 fixture: Policy 2 cites charter 2"
        );
    }
    let project = projects
        .revise_charter(project.id, 2, charter_draft(3))
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
        (
            view.digest.algorithm.as_str(),
            view.digest.canonicalizer.as_str()
        ),
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

// ---------------------------------------------------------------------------------------------
// Root-authored probes, appended verbatim from
// private-evidence/block13-20260909-supervised-mission/astra-c5-owner-probes.rs
// ---------------------------------------------------------------------------------------------
#[test]
fn astra_c5_orphan_classification_event_is_not_an_empty_ladder() {
    let f = fixture("astra-orphan-classification");
    let first = f
        .owner()
        .classify(high_declaration(f.mission.id))
        .expect("C5.ASTRA.ORPHAN.PREMISE_FIRST_CLASSIFICATION");
    let c = f.store.connect();
    let payload: String = c.query_row(
        "SELECT payload_json FROM events WHERE kind='mission.impact_classified' AND json_extract(payload_json,'$.digest.value')=?1",
        [&first.digest.value], |r| r.get(0)).expect("C5.ASTRA.ORPHAN.PREMISE_REAL_EVENT");
    let json: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(
        json["mission_id"],
        f.mission.id.to_string(),
        "C5.ASTRA.ORPHAN.PREMISE_OWNER_KEY_IS_SNAKE_CASE"
    );
    assert!(
        json.get("missionId").is_none(),
        "C5.ASTRA.ORPHAN.PREMISE_NO_CAMEL_KEY"
    );
    c.execute_batch("DROP TRIGGER mission_impact_classifications_no_delete;")
        .expect("C5.ASTRA.ORPHAN.FAULT_ONLY_DISPOSABLE_STORE");
    assert_eq!(
        c.execute(
            "DELETE FROM mission_impact_classifications WHERE digest_value=?1",
            [&first.digest.value]
        )
        .unwrap(),
        1,
        "C5.ASTRA.ORPHAN.PREMISE_ONE_ROW_REMOVED"
    );
    let before = f.counts();
    let got = f.owner().classify(high_declaration(f.mission.id));
    assert!(
        matches!(
            got,
            Err(beacond::ClassificationCommandError::Unavailable(_))
        ),
        "C5.ASTRA.ORPHAN_EVENT_IS_NOT_ABSENCE: {got:?}"
    );
    assert_eq!(f.counts(), before, "C5.ASTRA.ORPHAN.REFUSAL_WRITES_NOTHING");
}

#[test]
fn astra_c5_event_digest_domain_is_part_of_owner_proof() {
    for field in ["algorithm", "canonicalizer"] {
        let f = fixture("astra-event-digest-domain");
        let first = f
            .owner()
            .classify(high_declaration(f.mission.id))
            .expect("C5.ASTRA.EVENT_DIGEST.PREMISE_FIRST_CLASSIFICATION");
        let c = f.store.connect();
        let path = format!("$.digest.{field}");
        assert_eq!(c.execute(
            "UPDATE events SET payload_json=json_set(payload_json,?1,'not-the-owner-domain') WHERE kind='mission.impact_classified' AND json_extract(payload_json,'$.digest.value')=?2",
            rusqlite::params![path, first.digest.value]).unwrap(),1,
            "C5.ASTRA.EVENT_DIGEST.PREMISE_ONE_FIELD_CHANGED");
        let before = f.counts();
        let got = f.owner().classify(high_declaration(f.mission.id));
        assert!(
            matches!(
                got,
                Err(beacond::ClassificationCommandError::Unavailable(_))
            ),
            "C5.ASTRA.EVENT_DIGEST_DOMAIN_IS_PROVEN_BEFORE_ALREADY_CLASSIFIED ({field}): {got:?}"
        );
        assert_eq!(
            f.counts(),
            before,
            "C5.ASTRA.EVENT_DIGEST.REFUSAL_WRITES_NOTHING"
        );
    }
}

#[test]
fn astra_c5_policy_revision_projection_is_checked_before_narrowing() {
    for column in ["policy_revision", "charter_revision"] {
        let f = fixture("astra-numeric-domain");
        let first = f
            .owner()
            .classify(high_declaration(f.mission.id))
            .expect("C5.ASTRA.NUMERIC.PREMISE_FIRST_CLASSIFICATION");
        let c = f.store.connect();
        c.execute_batch(
            "PRAGMA foreign_keys=OFF; DROP TRIGGER mission_impact_classifications_no_update;",
        )
        .expect("C5.ASTRA.NUMERIC.FAULT_ONLY_DISPOSABLE_STORE");
        let sql = format!(
            "UPDATE mission_impact_classifications SET {column}={column}+4294967296 WHERE digest_value=?1"
        );
        assert_eq!(
            c.execute(&sql, [&first.digest.value]).unwrap(),
            1,
            "C5.ASTRA.NUMERIC.PREMISE_ONE_PROJECTION_CHANGED"
        );
        let raw: i64 = c
            .query_row(
                &format!(
                    "SELECT {column} FROM mission_impact_classifications WHERE digest_value=?1"
                ),
                [&first.digest.value],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            raw > i64::from(u32::MAX),
            "C5.ASTRA.NUMERIC.PREMISE_OUT_OF_DOMAIN"
        );
        let before = f.counts();
        let got = f.owner().classify(high_declaration(f.mission.id));
        assert!(
            matches!(
                got,
                Err(beacond::ClassificationCommandError::Unavailable(_))
            ),
            "C5.ASTRA.NUMERIC_DOMAIN_IS_PROVEN_BEFORE_ALREADY_CLASSIFIED ({column}): {got:?}"
        );
        assert_eq!(
            f.counts(),
            before,
            "C5.ASTRA.NUMERIC.REFUSAL_WRITES_NOTHING"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// The bounded observation read — one Deferred snapshot, zero writes, the SAME proof as the writer.
// ---------------------------------------------------------------------------------------------

/// C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE — no classification beside a present Policy.
#[test]
fn c5_observe_reports_an_absent_classification_beside_a_present_policy() {
    let fixture = fixture("observe-absent");
    let before = fixture.counts();

    let observation = fixture
        .owner()
        .observe(fixture.mission.id)
        .expect("C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE");

    assert_eq!(
        observation.mission_id, fixture.mission.id,
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE"
    );
    assert_eq!(
        observation.latest, None,
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: nothing has been classified"
    );
    assert_eq!(
        observation.published_policy,
        Some(beacond::PolicyCitationDto {
            project_id: fixture.project.id,
            policy_revision: 2,
            charter_revision: 2,
        }),
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: the Policy a NEW classification would cite"
    );
    // A canonical nonnegative decimal in the u64 domain: no sign, no space, no leading zero.
    let cursor = &observation.observed_at_event_seq;
    assert!(
        !cursor.is_empty()
            && cursor.bytes().all(|byte| byte.is_ascii_digit())
            && (cursor == "0" || !cursor.starts_with('0')),
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: {cursor:?} is not a canonical decimal"
    );
    assert_eq!(
        cursor
            .parse::<u64>()
            .expect("C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE"),
        u64::try_from(
            fixture
                .store
                .connect()
                .query_row(
                    "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
                    [fixture.store.workspace_id.to_string()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        )
        .unwrap(),
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: the cursor is this Workspace's chain head"
    );
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: a read wrote something"
    );

    // The JSON shape the client is promised: four camelCase keys, both nullables PRESENT as null.
    let json: serde_json::Value = serde_json::to_value(&beacond::ClassificationObservation {
        mission_id: fixture.mission.id,
        observed_at_event_seq: "9007199254740993".to_owned(),
        published_policy: None,
        latest: None,
    })
    .unwrap();
    assert_eq!(
        json.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        // `serde_json`'s object map is SORTED, so the order here is the map's and not the
        // contract's. What the contract fixes is the set of keys and their spelling.
        [
            "latest",
            "missionId",
            "observedAtEventSeq",
            "publishedPolicy"
        ],
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: exactly four keys, in the contract's spelling"
    );
    assert_eq!(
        json["observedAtEventSeq"],
        serde_json::Value::String("9007199254740993".to_owned()),
        "C5.OWNER.OBSERVE_IS_A_PROVED_ABSENCE: a cursor above 2^53 stays a string, verbatim"
    );
    assert!(json["publishedPolicy"].is_null() && json["latest"].is_null());
}

/// C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY — null is a proved absence, not a failed read.
#[test]
fn c5_observe_reports_an_absent_policy_as_its_own_answer() {
    let fixture = fixture_with("observe-no-policy", false);

    let observation = fixture
        .owner()
        .observe(fixture.mission.id)
        .expect("C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY");
    assert_eq!(
        observation.published_policy, None,
        "C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY"
    );
    assert_eq!(
        observation.latest, None,
        "C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY"
    );

    // And the writer says so in its own words rather than through a foreign-key refusal.
    let refusal = fixture
        .owner()
        .classify(ClassificationIntentRequest {
            expected_policy_revision: None,
            ..high_declaration(fixture.mission.id)
        })
        .expect_err(
            "C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY: a write cannot cite what is absent",
        );
    assert_eq!(
        refusal.name(),
        "NoPolicyPublished",
        "C5.OWNER.OBSERVE_A_PROJECT_WITH_NO_POLICY: {refusal:?}"
    );
}

/// C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW — the cited Policy and the current one differ.
#[test]
fn c5_observe_distinguishes_the_historical_citation_from_the_current_policy() {
    let fixture = fixture("observe-history-vs-now");
    let recorded = fixture
        .owner()
        .classify(high_declaration(fixture.mission.id))
        .expect("C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW");

    // Policy moves AFTER the classification.
    let third =
        ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
            .publish(fixture.project.id, 3, Some(2), policy_draft())
            .expect("C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW: Policy 3");
    assert_eq!(
        (third.revision, third.charter_revision),
        (3, 3),
        "C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW fixture"
    );

    let observation = fixture
        .owner()
        .observe(fixture.mission.id)
        .expect("C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW");
    assert_eq!(
        observation
            .published_policy
            .as_ref()
            .map(|policy| (policy.policy_revision, policy.charter_revision)),
        Some((3, 3)),
        "C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW: what a NEW classification would cite"
    );
    let latest = observation
        .latest
        .as_ref()
        .expect("C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW");
    assert_eq!(
        (
            latest.policy.policy_revision,
            latest.policy.charter_revision
        ),
        (2, 2),
        "C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW: what the existing record DID cite"
    );
    assert_eq!(
        latest, &recorded,
        "C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW: the same view"
    );
    assert_eq!(
        latest.rule_id, None,
        "C5.OWNER.OBSERVE_SEPARATES_HISTORY_FROM_NOW: a declaration names no rule"
    );
}

/// C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY — a `policy_rule` row is read as what it says.
///
/// The record is minted through the EXISTING public `MissionImpactService::classify`, which admits
/// `PolicyRule` with a rule id (`mission_impact.rs:332-343`). This owner writes no such record and
/// gains no permission to; it must simply not relabel one that exists.
#[test]
fn c5_observe_reads_a_legacy_policy_rule_record_with_its_actual_rule_identity() {
    let fixture = fixture("observe-legacy-rule");
    let legacy = beacond::MissionImpactClassificationV1 {
        workspace_id: fixture.store.workspace_id,
        mission_id: fixture.mission.id,
        classification_id: Uuid::new_v4(),
        version: 1,
        impact: beacond::ImpactLevel::High,
        basis: beacond::ClassificationBasis::PolicyRule,
        policy: beacond::PolicyRevisionRef {
            project_id: fixture.project.id,
            policy_revision: 2,
            charter_revision: 2,
        },
        rule_id: Some("appendix.high-impact".to_owned()),
        reason: "The shipped store admits this basis with a named rule.".to_owned(),
        classified_by: beacond::ClassificationActor::Operator,
        classified_at_ms: 1_789_000_000_000,
        supersedes: beacond::ClassificationSupersedes::None,
    };
    let bytes = legacy
        .canonical_bytes()
        .expect("C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY: the grammar admits it");
    beacond::MissionImpactService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .classify(&bytes)
        .expect("C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY: the shipped writer records it");

    let latest = fixture
        .owner()
        .observe(fixture.mission.id)
        .expect("C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY")
        .latest
        .expect("C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY");
    assert_eq!(
        (latest.basis.as_str(), latest.rule_id.as_deref()),
        ("policy_rule", Some("appendix.high-impact")),
        "C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY: a recorded basis is not relabelled"
    );
    // The key is present on the wire for both shapes.
    let json = serde_json::to_value(&latest).unwrap();
    assert_eq!(
        json["ruleId"],
        serde_json::Value::String("appendix.high-impact".to_owned())
    );
    assert_eq!(
        json.as_object().unwrap().len(),
        12,
        "C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY: exactly twelve view fields"
    );
    assert!(
        json.get("ruleId").is_some(),
        "C5.OWNER.OBSERVE_KEEPS_A_LEGACY_RULE_IDENTITY: ruleId is a required key"
    );
}

/// C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE — corruption and orphans are never a null.
#[test]
fn c5_observe_refuses_corrupt_and_orphan_evidence() {
    // A corrupt row.
    let corrupted = fixture("observe-corrupt");
    corrupted
        .owner()
        .classify(high_declaration(corrupted.mission.id))
        .expect("C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE control");
    let connection = corrupted.store.connect();
    connection
        .execute_batch(
            "DROP TRIGGER mission_impact_classifications_no_update;
             UPDATE mission_impact_classifications SET canonical_bytes = zeroblob(64);",
        )
        .unwrap();
    let before = corrupted.counts();
    let refusal = corrupted
        .owner()
        .observe(corrupted.mission.id)
        .expect_err("C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE: corruption is not absence");
    assert_eq!(
        refusal.name(),
        "Unavailable",
        "C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE: {refusal:?}"
    );
    assert_eq!(
        corrupted.counts(),
        before,
        "C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE: a refused read wrote"
    );
    drop(connection);

    // An orphan Event, on the READ path this time.
    let orphaned = fixture("observe-orphan");
    let recorded = orphaned
        .owner()
        .classify(high_declaration(orphaned.mission.id))
        .expect("C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE control");
    let connection = orphaned.store.connect();
    connection
        .execute_batch("DROP TRIGGER mission_impact_classifications_no_delete;")
        .unwrap();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM mission_impact_classifications WHERE digest_value = ?1",
                [&recorded.digest.value],
            )
            .unwrap(),
        1,
        "C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE fixture"
    );
    drop(connection);
    let refusal = orphaned
        .owner()
        .observe(orphaned.mission.id)
        .expect_err("C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE: an orphan is not absence");
    assert_eq!(
        refusal.name(),
        "Unavailable",
        "C5.OWNER.OBSERVE_REFUSES_BROKEN_EVIDENCE: {refusal:?}"
    );
}

/// C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT — missing or duplicated, both refuse.
#[test]
fn c5_observe_refuses_a_missing_or_duplicated_policy_event() {
    // Duplicated.
    let duplicated = fixture("observe-policy-event-double");
    // INTENTIONAL: revision 1's own Event is relabelled as a second claim about revision 2. The
    // chain columns 0036 added (`predecessor_kind` and its siblings) make an INSERT here a forgery
    // this test has no business building; relabelling produces the same two-Events-one-revision
    // state without pretending to extend the chain.
    let connection = duplicated.store.connect();
    assert_eq!(
        connection
            .execute(
                "UPDATE events
                 SET payload_json = json_set(payload_json, '$.policy.revision', 2)
                 WHERE kind = 'project.policy_published'
                   AND json_extract(payload_json, '$.policy.revision') = 1",
                [],
            )
            .expect("C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: relabel revision 1"),
        1,
        "C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: exactly one Event is relabelled"
    );
    drop(connection);
    let refusal = duplicated
        .owner()
        .observe(duplicated.mission.id)
        .expect_err("C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: two Events, one revision");
    assert_eq!(
        refusal.name(),
        "Unavailable",
        "C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: {refusal:?}"
    );

    // Missing.
    let missing = fixture("observe-policy-event-missing");
    let connection = missing.store.connect();
    assert_eq!(
        connection
            .execute(
                "DELETE FROM events WHERE kind = 'project.policy_published'
                   AND json_extract(payload_json, '$.policy.revision') = 2",
                [],
            )
            .unwrap(),
        1,
        "C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT fixture"
    );
    drop(connection);
    let refusal = missing
        .owner()
        .observe(missing.mission.id)
        .expect_err("C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: a row the log does not account for");
    assert_eq!(
        refusal.name(),
        "Unavailable",
        "C5.OWNER.OBSERVE_PROVES_THE_POLICY_EVENT: {refusal:?}"
    );
}

/// C5.OWNER.SCOPE_IS_OPAQUE — an unknown Mission and a foreign Workspace are one answer, on both.
#[test]
fn c5_scope_opacity_and_authentication_hold_on_both_methods() {
    let fixture = fixture("scope-and-auth");
    let foreign = MissionImpactCommandService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());

    let unknown = fixture
        .owner()
        .observe(Uuid::new_v4())
        .expect_err("C5.OWNER.SCOPE_IS_OPAQUE: unknown");
    let elsewhere = foreign
        .observe(fixture.mission.id)
        .expect_err("C5.OWNER.SCOPE_IS_OPAQUE: another Workspace");
    assert_eq!(
        format!("{unknown}"),
        format!("{elsewhere}"),
        "C5.OWNER.SCOPE_IS_OPAQUE: the two answers must be indistinguishable"
    );
    assert_eq!(
        format!("{unknown}"),
        "Unavailable",
        "C5.OWNER.SCOPE_IS_OPAQUE"
    );

    let unknown = fixture
        .owner()
        .classify(high_declaration(Uuid::new_v4()))
        .expect_err("C5.OWNER.SCOPE_IS_OPAQUE: unknown, writing");
    let elsewhere = foreign
        .classify(high_declaration(fixture.mission.id))
        .expect_err("C5.OWNER.SCOPE_IS_OPAQUE: another Workspace, writing");
    assert_eq!(
        format!("{unknown}"),
        format!("{elsewhere}"),
        "C5.OWNER.SCOPE_IS_OPAQUE"
    );

    // A locked session keeps its own meaning on both methods.
    let before = fixture.counts();
    fixture.accounts.lock().expect("the session locks");
    for refusal in [
        fixture
            .owner()
            .classify(high_declaration(fixture.mission.id))
            .expect_err("C5.OWNER.AUTHENTICATION_RETAINED: classify"),
        fixture
            .owner()
            .observe(fixture.mission.id)
            .expect_err("C5.OWNER.AUTHENTICATION_RETAINED: observe"),
    ] {
        assert!(
            matches!(
                refusal,
                beacond::ClassificationCommandError::Account(
                    beacond::OperatorAccountError::NotAuthenticated
                )
            ),
            "C5.OWNER.AUTHENTICATION_RETAINED: {refusal:?}"
        );
    }
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.AUTHENTICATION_RETAINED: a locked session wrote"
    );
}

// ---------------------------------------------------------------------------------------------
// The writer's remaining groups.
// ---------------------------------------------------------------------------------------------

/// C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR — and concurrent intent is never overwritten.
#[test]
fn c5_a_reclassification_names_and_proves_its_predecessor() {
    // Built first: `fixture` is shadowed by the binding below.
    let elsewhere = fixture("reclassify-foreign");
    let fixture = fixture("reclassify");
    let owner = fixture.owner();
    let first = owner
        .classify(high_declaration(fixture.mission.id))
        .expect("C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR");

    // A second "first" classification is refused: the ladder is not empty.
    let before = fixture.counts();
    let refusal = owner
        .classify(high_declaration(fixture.mission.id))
        .expect_err(
            "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR: none on a classified Mission",
        );
    assert_eq!(
        refusal.name(),
        "AlreadyClassified",
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR: {refusal:?}"
    );
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR: it wrote"
    );

    let reclassify = |predecessor: &str| ClassificationIntentRequest {
        impact: "ordinary".to_owned(),
        reason: "A non-author seat reviewed it and the blast radius is one file.".to_owned(),
        supersedes: SupersedesDto::Record {
            predecessor_digest: predecessor.to_owned(),
        },
        ..high_declaration(fixture.mission.id)
    };
    let second = owner
        .classify(reclassify(&first.digest.value))
        .expect("C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR");
    assert_eq!(
        second.version, 2,
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR"
    );
    assert_eq!(
        second.supersedes,
        SupersedesDto::Record {
            predecessor_digest: first.digest.value.clone()
        },
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR"
    );
    assert_eq!(
        second.impact, "ordinary",
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR"
    );
    assert_eq!(
        owner.observe(fixture.mission.id).unwrap().latest.unwrap(),
        second,
        "C5.OWNER.RECLASSIFICATION_NAMES_ITS_PREDECESSOR: the newest proven record"
    );

    // The predecessor that is no longer latest: concurrent intent is refused, not stacked.
    let before = fixture.counts();
    let refusal = owner
        .classify(reclassify(&first.digest.value))
        .expect_err("C5.OWNER.CONCURRENT_INTENT_IS_NEVER_OVERWRITTEN");
    assert_eq!(
        refusal.name(),
        "PredecessorNotLatest",
        "C5.OWNER.CONCURRENT_INTENT_IS_NEVER_OVERWRITTEN: {refusal:?}"
    );

    // A predecessor that is not this Mission's, and one that is not stored at all.
    let foreign = elsewhere
        .owner()
        .classify(high_declaration(elsewhere.mission.id))
        .expect("C5.OWNER.CONCURRENT_INTENT_IS_NEVER_OVERWRITTEN fixture");
    for digest in [foreign.digest.value.as_str(), &"0".repeat(64)] {
        let refusal = owner
            .classify(reclassify(digest))
            .expect_err("C5.OWNER.PREDECESSOR_IS_PROVEN");
        assert_eq!(
            refusal.name(),
            "PredecessorUnproven",
            "C5.OWNER.PREDECESSOR_IS_PROVEN: {refusal:?}"
        );
    }
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.PREDECESSOR_IS_PROVEN: a refused write left something"
    );
}

/// C5.OWNER.STALE_POLICY_REFUSES — the echoed revision is the Operator's guard, not decoration.
#[test]
fn c5_a_stale_policy_revision_refuses_before_writing() {
    let fixture = fixture("stale-policy");
    let before = fixture.counts();

    for echoed in [Some(1), Some(3), None] {
        let refusal = fixture
            .owner()
            .classify(ClassificationIntentRequest {
                expected_policy_revision: echoed,
                ..high_declaration(fixture.mission.id)
            })
            .expect_err("C5.OWNER.STALE_POLICY_REFUSES");
        assert_eq!(
            refusal.name(),
            "StalePolicyRevision",
            "C5.OWNER.STALE_POLICY_REFUSES ({echoed:?}): {refusal:?}"
        );
    }
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.STALE_POLICY_REFUSES: a refusal wrote"
    );

    // And a revision published between the read and the write is refused with the same name.
    let owner = fixture.owner();
    ProjectPolicyService::new(Arc::clone(&fixture.accounts), fixture.store.workspace_id)
        .publish(fixture.project.id, 3, Some(2), policy_draft())
        .expect("C5.OWNER.STALE_POLICY_REFUSES: Policy 3");
    let refusal = owner
        .classify(high_declaration(fixture.mission.id))
        .expect_err("C5.OWNER.STALE_POLICY_REFUSES: the form's revision is gone");
    assert_eq!(
        refusal.name(),
        "StalePolicyRevision",
        "C5.OWNER.STALE_POLICY_REFUSES: {refusal:?}"
    );
}

/// C5.OWNER.INPUT_DOMAINS — explicit tags, present-but-nullable keys, and the codec's text classes.
#[test]
fn c5_the_request_domains_are_explicit() {
    // Built first: `fixture` is shadowed by the binding below.
    let broken = fixture("input-domains-digest");
    let fixture = fixture("input-domains");
    let before = fixture.counts();

    // The impact tag is closed and has no default.
    for token in ["", "High", "unclassified", "ordinary "] {
        let refusal = fixture
            .owner()
            .classify(ClassificationIntentRequest {
                impact: token.to_owned(),
                ..high_declaration(fixture.mission.id)
            })
            .expect_err("C5.OWNER.INPUT_DOMAINS: an unknown impact must refuse");
        assert!(
            matches!(&refusal, beacond::ClassificationCommandError::MalformedRequest(field) if *field == "impact"),
            "C5.OWNER.INPUT_DOMAINS ({token:?}): {refusal:?}"
        );
    }

    // The codec's own text classes, each its own refusal and only one of them length-shaped.
    for (reason, expected) in [
        ("x".repeat(2_001), "TextOverLimit"),
        ("\u{1f9ed}".repeat(2_001), "TextOverLimit"),
        (String::new(), "MalformedRequest"),
        (" untrimmed".to_owned(), "MalformedRequest"),
        ("a\u{7}bell".to_owned(), "MalformedRequest"),
    ] {
        let refusal = fixture
            .owner()
            .classify(ClassificationIntentRequest {
                reason: reason.clone(),
                ..high_declaration(fixture.mission.id)
            })
            .expect_err("C5.OWNER.INPUT_DOMAINS: a malformed reason must refuse");
        assert_eq!(
            refusal.name(),
            expected,
            "C5.OWNER.INPUT_DOMAINS ({} scalars): {refusal:?}",
            reason.chars().count()
        );
    }
    // 2000 scalars is legal, astral included.
    fixture
        .owner()
        .classify(ClassificationIntentRequest {
            reason: "\u{1f9ed}".repeat(2_000),
            ..high_declaration(fixture.mission.id)
        })
        .expect("C5.OWNER.INPUT_DOMAINS: 2000 astral scalars are within the bound");

    // The predecessor digest's shape.
    for digest in [
        "0".repeat(63),
        "0".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let refusal = broken
            .owner()
            .classify(ClassificationIntentRequest {
                supersedes: SupersedesDto::Record {
                    predecessor_digest: digest.clone(),
                },
                ..high_declaration(broken.mission.id)
            })
            .expect_err("C5.OWNER.INPUT_DOMAINS: a malformed digest must refuse");
        assert!(
            matches!(&refusal, beacond::ClassificationCommandError::MalformedRequest(field) if field.ends_with("predecessorDigest")),
            "C5.OWNER.INPUT_DOMAINS ({digest}): {refusal:?}"
        );
    }
    assert_eq!(
        broken.counts(),
        before,
        "C5.OWNER.INPUT_DOMAINS: a refused request wrote"
    );

    // Present-but-nullable at the DTO boundary: an OMITTED key refuses, an explicit null passes.
    let complete = serde_json::json!({
        "missionId": Uuid::new_v4(),
        "impact": "high",
        "reason": "A reason.",
        "expectedPolicyRevision": serde_json::Value::Null,
        "supersedes": { "kind": "none" },
    });
    serde_json::from_value::<ClassificationIntentRequest>(complete.clone())
        .expect("C5.OWNER.INPUT_DOMAINS: an explicit null is a claim");
    let mut omitted = complete.as_object().unwrap().clone();
    omitted.remove("expectedPolicyRevision");
    let refusal =
        serde_json::from_value::<ClassificationIntentRequest>(serde_json::Value::Object(omitted))
            .expect_err("C5.OWNER.INPUT_DOMAINS: an omitted key is not a null");
    assert!(
        refusal.to_string().contains("expectedPolicyRevision"),
        "C5.OWNER.INPUT_DOMAINS: {refusal}"
    );
    let mut unknown = complete.as_object().unwrap().clone();
    unknown.insert(
        "basis".to_owned(),
        serde_json::Value::String("policy_rule".to_owned()),
    );
    serde_json::from_value::<ClassificationIntentRequest>(serde_json::Value::Object(unknown))
        .expect_err("C5.OWNER.INPUT_DOMAINS: basis is not an input and an unknown key refuses");
}

/// C5.OWNER.ONE_ATOMIC_TRANSACTION — a failed write leaves no row and no Event.
#[test]
fn c5_a_failed_write_leaves_no_row_and_no_event() {
    let fixture = fixture("atomic");
    let before = fixture.counts();

    // A FORCED failure inside the writer, planted on a second connection after the account service
    // opened its own, and removed before the store is reopened.
    let connection = fixture.store.connect();
    connection
        .execute_batch(
            "CREATE TRIGGER c5_forced_write_failure
             BEFORE INSERT ON mission_impact_classifications
             BEGIN SELECT RAISE(ABORT, 'c5 forced write failure'); END;",
        )
        .unwrap();
    drop(connection);

    let refusal = fixture
        .owner()
        .classify(high_declaration(fixture.mission.id))
        .expect_err("C5.OWNER.ONE_ATOMIC_TRANSACTION: the forced failure must refuse");
    assert_eq!(
        refusal.name(),
        "Unavailable",
        "C5.OWNER.ONE_ATOMIC_TRANSACTION: {refusal:?}"
    );
    assert_eq!(
        fixture.counts(),
        before,
        "C5.OWNER.ONE_ATOMIC_TRANSACTION: a failed write left a row or an Event behind"
    );

    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER c5_forced_write_failure;")
        .unwrap();
    drop(connection);

    // Reopened from disk: the rollback survives the process, not just the handle that made it.
    let reopened = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    reopened
        .authenticate_password(
            SecretString::new("a long local passphrase".to_owned()),
            beacond::AccessOrigin::SameMachine,
        )
        .expect("C5.OWNER.ONE_ATOMIC_TRANSACTION: the store reopens");
    let owner = MissionImpactCommandService::new(Arc::clone(&reopened), fixture.store.workspace_id);
    assert_eq!(
        owner
            .observe(fixture.mission.id)
            .expect("C5.OWNER.ONE_ATOMIC_TRANSACTION")
            .latest,
        None,
        "C5.OWNER.ONE_ATOMIC_TRANSACTION: the reopened store must show no classification"
    );
    owner
        .classify(high_declaration(fixture.mission.id))
        .expect("C5.OWNER.ONE_ATOMIC_TRANSACTION: the unblocked write records");
    assert_eq!(
        fixture.counts(),
        (before.0 + 1, before.1 + 1),
        "C5.OWNER.ONE_ATOMIC_TRANSACTION: one row, one owning Event"
    );
}

/// C5.OWNER.CLOSED_PROJECTION — every outward arm, against literals written here.
#[test]
fn c5_every_outward_refusal_projects_one_closed_literal() {
    use beacond::ClassificationCommandError as Refusal;
    use beacond::OperatorAccountError as Account;

    // The whole outward vocabulary, spelled out here rather than read from `name()`.
    let arms: Vec<(Refusal, &str)> = vec![
        (Refusal::Account(Account::NotAuthenticated), "Account"),
        (Refusal::StalePolicyRevision, "StalePolicyRevision"),
        (Refusal::NoPolicyPublished, "NoPolicyPublished"),
        (Refusal::AlreadyClassified, "AlreadyClassified"),
        (Refusal::PredecessorNotLatest, "PredecessorNotLatest"),
        (Refusal::PredecessorUnproven, "PredecessorUnproven"),
        (Refusal::VersionCeilingReached, "VersionCeilingReached"),
        (Refusal::TextOverLimit, "TextOverLimit"),
        (
            Refusal::MalformedRequest("C5_PRIVATE_FIELD\r\nneedle"),
            "MalformedRequest",
        ),
        (
            Refusal::Unavailable(beacond::ClassificationCommandUnavailable::LadderUnprovable(
                "C5_PRIVATE_LADDER\r\nneedle".to_owned(),
            )),
            "Unavailable",
        ),
    ];
    assert_eq!(
        arms.len(),
        10,
        "C5.OWNER.CLOSED_PROJECTION: ten outward arms"
    );

    for (refusal, expected) in &arms {
        assert_eq!(
            &refusal.to_string(),
            expected,
            "C5.OWNER.CLOSED_PROJECTION: {refusal:?}"
        );
        assert_eq!(refusal.name(), *expected, "C5.OWNER.CLOSED_PROJECTION");
        let line = refusal.to_string();
        assert!(
            !line.contains("C5_PRIVATE") && !line.contains('\n') && !line.contains('\r'),
            "C5.OWNER.CLOSED_PROJECTION: a needle reached the caller line: {line:?}"
        );
    }
    // The needles survive in Debug, escaped, and only there.
    assert!(
        format!("{:?}", arms[8].0).contains("C5_PRIVATE_FIELD"),
        "C5.OWNER.CLOSED_PROJECTION: Debug keeps the field"
    );
    assert!(
        format!("{:?}", arms[9].0).contains("C5_PRIVATE_LADDER"),
        "C5.OWNER.CLOSED_PROJECTION: Debug keeps the reason"
    );

    // The nested account child keeps its own meaning and its own source.
    let account = Refusal::Account(Account::NotAuthenticated);
    assert!(
        std::error::Error::source(&account).is_some(),
        "C5.OWNER.CLOSED_PROJECTION: the account child is reachable through source"
    );
    assert!(
        std::error::Error::source(&Refusal::AlreadyClassified).is_none(),
        "C5.OWNER.CLOSED_PROJECTION: a leaf has no source"
    );
}
