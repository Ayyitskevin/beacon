//! Outcome Document slice 3-A: the immutable manifest store and `prepare` (ADR 0040, §7 slice 3).
//!
//! One test per bullet of the slice's list, plus the mutation gate's probes. The fixture chain down
//! to `admitted_mission` is `tests/artifact.rs`'s, reused verbatim: the manifest store's foreign
//! keys reach `missions`, `mission_completions` and `artifacts`, so a row cannot exist without the
//! admission chain that produces one. Copied rather than shared, because the two suites must be
//! able to disagree about what a Mission is -- a shared fixture is how two tests agree about
//! something wrong.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    AccessOrigin, AppendixMode, AppendixRequest, ArtifactContext, ArtifactKind, ArtifactService,
    AutonomyProfile, BeaconCapability, COMPLETION_CANONICALIZER, ClassificationActor,
    ClassificationBasis, ClassificationSupersedes, CompletionActor, CompletionDisposition,
    CompletionSupersedes, ConditionOutcome, ConditionResult, ConditionResultRecord, ImpactLevel,
    LocalModelActivationService, LocalModelEndpoint, MISSION_IMPACT_CANONICALIZER,
    ManifestStoreRefusal, MissionCompletionV1, MissionDraft, MissionImpactClassificationV1,
    MissionImpactService, MissionMemberDraft, MissionService, OperatorAccountService,
    OperatorSessionPhase, OutcomeManifestV1, OutcomeService, OutcomeState, PolicyRevisionRef,
    PrepareRefusal, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService,
    ProjectService, ProviderFeature, ResidentConfigurationService, ResidentLifecycleService,
    SecretString, SourceDigest, SourceKind, SourceRef, SpendReference, StarterOrganizationService,
    StarterResidentDraft, StarterRole, StartupConfig, SuccessCondition, Supervisor, TeamDraft,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TeamMemberDraft, TeamService, TrackRecordEvidenceKind, TrackRecordRequirement, VerifiedSources,
    VerifiedText, declared_conditions_digest, record_completion, render, verified_manifest_within,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

const CONDITION_TEXT: &str = "Relevant gates pass";
const CONDITION_ID: &str = "sc-gates";

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-outcome-prepare-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
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

struct Fixture {
    store: TestStore,
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    other_resident_id: Uuid,
    other_configuration_id: Uuid,
    policy_revision: u32,
    charter_revision: u32,
    outcomes: OutcomeService,
}

fn tags_response() -> String {
    "{\"models\":[{\"name\":\"qwen3.8:27b\",\"model\":\"qwen3.8:27b\",\
     \"modified_at\":\"2026-08-14T14:38:20.13373148-04:00\",\"size\":17741872154,\
     \"digest\":\"22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643\",\
     \"details\":{\"format\":\"gguf\",\"family\":\"qwen35\",\"families\":[\"qwen35\"],\
     \"parameter_size\":\"27.3B\",\"quantization_level\":\"Q4_K_M\"}}]}"
        .to_owned()
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

fn serve_ollama_inspections(count: usize) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for _ in 0..count {
            for expected in ["GET /api/tags", "POST /api/show", "GET /api/tags"] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                assert!(request.starts_with(expected), "unexpected: {request}");
                let body = if expected.starts_with("POST") {
                    "{\"capabilities\":[\"completion\",\"tools\"]}".to_owned()
                } else {
                    tags_response()
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .unwrap();
            }
        }
    });
    (address, handle)
}

/// A Workspace with one Project, one published Policy revision, and one admitted Mission whose
/// roster holds two Residents at their exact admission Configurations.
fn admitted_mission(label: &str) -> Fixture {
    let store = TestStore::new(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let workspace_id = report.workspace.id;
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

    let project = ProjectService::new(Arc::clone(&accounts), workspace_id)
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
    let policy = ProjectPolicyService::new(Arc::clone(&accounts), workspace_id)
        .publish(
            project.id,
            project.charter.revision,
            None,
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
            },
        )
        .unwrap();

    let residents = StarterOrganizationService::new(Arc::clone(&accounts), workspace_id)
        .accept(
            1,
            vec![
                StarterResidentDraft {
                    role: StarterRole::Lead,
                    display_name: None,
                },
                StarterResidentDraft {
                    role: StarterRole::Builder,
                    display_name: None,
                },
            ],
        )
        .unwrap()
        .residents;
    let lifecycle = ResidentLifecycleService::new(Arc::clone(&accounts), workspace_id);
    for resident in &residents {
        lifecycle.activate(resident.id, None).unwrap();
    }

    let (address, ollama) = serve_ollama_inspections(2);
    let models = LocalModelActivationService::new(
        Arc::clone(&accounts),
        workspace_id,
        LocalModelEndpoint::new(address).unwrap(),
    );
    let review = models
        .inspect(
            "qwen3.8:27b",
            "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
        )
        .unwrap();
    let activation = models
        .activate(review, vec![ProviderFeature::Completion])
        .unwrap();
    ollama.join().unwrap();

    let configurations = residents
        .iter()
        .map(|resident| {
            ResidentConfigurationService::new(Arc::clone(&accounts), workspace_id)
                .configure(
                    resident.id,
                    None,
                    activation.id,
                    vec![
                        BeaconCapability::WorkCreateMission,
                        BeaconCapability::WorkCoordinateMission,
                    ],
                )
                .unwrap()
        })
        .collect::<Vec<_>>();
    let team = TeamService::new(Arc::clone(&accounts), workspace_id)
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
    let mission = MissionService::new(Arc::clone(&accounts), workspace_id)
        .admit(MissionDraft {
            project_id: project.id,
            expected_charter_revision: project.charter.revision,
            team_id: team.id,
            desired_outcome: "Ship one reviewed Beacon feature.".to_owned(),
            success_conditions: vec![CONDITION_TEXT.to_owned()],
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

    Fixture {
        outcomes: OutcomeService::new(Arc::clone(&accounts), workspace_id),
        store,
        accounts,
        workspace_id,
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configurations[0].resident_id,
        configuration_id: configurations[0].id,
        other_resident_id: configurations[1].resident_id,
        other_configuration_id: configurations[1].id,
        policy_revision: policy.revision,
        charter_revision: project.charter.revision,
    }
}

impl Fixture {
    fn latest_event_seq(&self) -> u64 {
        self.store
            .connect()
            .query_row(
                "SELECT coalesce(max(sequence), 0) FROM events WHERE workspace_id = ?1",
                params![self.workspace_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map(|sequence| u64::try_from(sequence).unwrap())
            .unwrap()
    }

    /// One recorded completion, accountable to `resident_id` at its admission Configuration.
    fn complete(&self, accountable: (Uuid, Uuid)) -> (String, MissionCompletionV1) {
        let boundary = self.latest_event_seq();
        let record = MissionCompletionV1 {
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            completion_id: Uuid::new_v4(),
            actor: CompletionActor::Operator,
            accountable_resident_id: accountable.0,
            accountable_resident_configuration_id: accountable.1,
            boundary_event_seq: boundary,
            disposition: CompletionDisposition::Concluded,
            declared_conditions_sha256: declared_conditions_digest(&[CONDITION_TEXT.to_owned()]),
            condition_results: vec![ConditionResultRecord {
                ordinal: 0,
                outcome: ConditionOutcome::Met,
                support: vec![],
                note: None,
            }],
            delivered_artifacts: vec![],
            unfinished_work: vec![],
            uncertainty: vec![],
            spend: SpendReference::NotMeasured(
                "No spend record exists for this Mission.".to_owned(),
            ),
            supersedes: CompletionSupersedes::None,
            summary: "One reviewed feature shipped.".to_owned(),
            completed_at_ms: 1_788_400_000_000,
        };
        let mut connection = self.store.connect();
        let digest = record_completion(&mut connection, &record).expect("the completion records");
        (digest, record)
    }

    /// One recorded Mission impact classification at the requested impact level.
    fn classify(&self, impact: ImpactLevel) -> (Uuid, u32, String) {
        let classification = MissionImpactClassificationV1 {
            workspace_id: self.workspace_id,
            mission_id: self.mission_id,
            classification_id: Uuid::new_v4(),
            version: 1,
            impact,
            basis: ClassificationBasis::OperatorDeclared,
            policy: PolicyRevisionRef {
                project_id: self.project_id,
                policy_revision: self.policy_revision,
                charter_revision: self.charter_revision,
            },
            rule_id: None,
            reason: "The Mission writes to the Operator's own repository.".to_owned(),
            classified_by: ClassificationActor::Operator,
            classified_at_ms: 1_788_400_000_000,
            supersedes: ClassificationSupersedes::None,
        };
        let bytes = classification.canonical_bytes().unwrap();
        let recorded = MissionImpactService::new(Arc::clone(&self.accounts), self.workspace_id)
            .classify(&bytes)
            .expect("the classification records");
        (
            classification.classification_id,
            classification.version,
            recorded.digest().value.clone(),
        )
    }

    fn artifacts(&self) -> ArtifactService {
        ArtifactService::new(Arc::clone(&self.accounts), self.workspace_id)
    }

    fn restart(&self) {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .expect("the store restarts");
    }
}

fn source(
    kind: SourceKind,
    object_id: Uuid,
    canonicalizer: &str,
    digest_value: &str,
    schema_version: u32,
    workspace_id: Uuid,
) -> SourceRef {
    SourceRef {
        workspace_id,
        kind,
        object_id,
        schema_version,
        digest: SourceDigest {
            algorithm: "sha256".to_owned(),
            canonicalizer: canonicalizer.to_owned(),
            value: digest_value.to_owned(),
        },
        pinned_revision: OutcomeState::Absent(
            "this source type pins no content revision.".to_owned(),
        ),
    }
}

/// A manifest frozen on one recorded completion and one recorded classification, plus the verified
/// values its references name.
fn manifest_for(
    fixture: &Fixture,
    completion_digest: &str,
    completion: &MissionCompletionV1,
    classification: &(Uuid, u32, String),
    policy_high_impact: bool,
    appendix_request: AppendixRequest,
) -> (OutcomeManifestV1, VerifiedSources) {
    let manifest = OutcomeManifestV1 {
        workspace_id: fixture.workspace_id,
        // Identity-only, per R18.4: the Mission is cited by `object_id`, and the digest field --
        // which slice 1's `SourceRef` requires and which no Mission on `main` can supply -- says
        // so, carrying the completion digest as the anchor and the identity-only canonicalizer as
        // the claim. It goes away with the R18.3-5 grammar revision.
        mission: source(
            SourceKind::Mission,
            fixture.mission_id,
            beacond::MISSION_IDENTITY_ONLY,
            completion_digest,
            1,
            fixture.workspace_id,
        ),
        completion: source(
            SourceKind::Completion,
            completion.completion_id,
            COMPLETION_CANONICALIZER,
            completion_digest,
            1,
            fixture.workspace_id,
        ),
        policy_classification: source(
            SourceKind::Policy,
            classification.0,
            MISSION_IMPACT_CANONICALIZER,
            &classification.2,
            classification.1,
            fixture.workspace_id,
        ),
        boundary_event_seq: completion.boundary_event_seq,
        completion_recorded_at_ms: completion.completed_at_ms,
        accountable_resident_id: completion.accountable_resident_id,
        policy_high_impact,
        appendix_request,
        canonicalizer_version: "beacon.outcome.manifest.v1".to_owned(),
        renderer_version: "beacon.outcome.md.v1".to_owned(),
        success_conditions: vec![SuccessCondition {
            ordinal: 0,
            condition_id: CONDITION_ID.to_owned(),
            outcome: OutcomeState::Verified(ConditionResult::Met),
            citations: OutcomeState::Absent(
                "the completion recorded no supporting object for this condition.".to_owned(),
            ),
        }],
        delivered_artifacts: OutcomeState::Absent(
            "the completion delivered no Artifacts.".to_owned(),
        ),
        proposal_refs: OutcomeState::Absent("no Proposal is cited at this boundary.".to_owned()),
        ruling_refs: OutcomeState::Absent("no Ruling is cited at this boundary.".to_owned()),
        evaluation_refs: OutcomeState::NotEvaluated(
            "no Evaluation names this Mission at the boundary event.".to_owned(),
        ),
        receipt_refs: OutcomeState::Absent("no Receipt is cited at this boundary.".to_owned()),
        spend_ref: OutcomeState::NotEvaluated(
            "No spend record exists for this Mission.".to_owned(),
        ),
        unfinished_work: OutcomeState::Absent(
            "the completion recorded no unfinished work.".to_owned(),
        ),
        uncertainty: OutcomeState::Absent("the completion recorded no uncertainty.".to_owned()),
    };
    let sources = VerifiedSources {
        mission_title: "Ship one reviewed Beacon feature.".to_owned(),
        condition_statements: vec![VerifiedText {
            id: CONDITION_ID.to_owned(),
            text: CONDITION_TEXT.to_owned(),
        }],
        artifacts: Vec::new(),
        unfinished_work: Vec::new(),
        uncertainty: Vec::new(),
        verified_source_bytes: 4_096,
    };
    (manifest, sources)
}

/// The default arrangement: one completion, one ordinary classification, `policy_default`.
fn ordinary(label: &str) -> (Fixture, OutcomeManifestV1, VerifiedSources) {
    let fixture = admitted_mission(label);
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    (fixture, manifest, sources)
}

/// Runs `tamper` with an immutability trigger down, then restores it -- leaving the schema
/// signature intact and the store holding a row nothing should have written.
fn with_trigger_down(
    store: &TestStore,
    name: &str,
    table: &str,
    message: &str,
    tamper: impl FnOnce(&Connection),
) {
    let connection = store.connect();
    connection
        .execute_batch(&format!("DROP TRIGGER {name};"))
        .unwrap();
    tamper(&connection);
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER {name}
BEFORE UPDATE ON {table}
BEGIN
    SELECT RAISE(ABORT, '{message}');
END;"
        ))
        .unwrap();
}

// ---------------------------------------------------------------------------------------------
// The migration
// ---------------------------------------------------------------------------------------------

#[test]
fn the_ladder_produces_an_outcome_manifest_store() {
    let store = TestStore::new("ladder");
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let connection = store.connect();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .expect("the outcome manifest store does not exist after the ladder");
    assert_eq!(count, 0);
}

#[test]
fn stored_manifests_are_immutable_under_update_and_delete() {
    let (fixture, manifest, sources) = ordinary("immutable");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let connection = fixture.store.connect();
    for statement in [
        "UPDATE outcome_manifests SET renderer_version = 'beacon.outcome.md.v2'",
        "DELETE FROM outcome_manifests",
    ] {
        let error = connection.execute(statement, []).unwrap_err().to_string();
        assert!(
            error.contains("Outcome manifests are immutable"),
            "{statement} was not refused: {error}"
        );
    }
    // And the record still reads back as itself.
    fixture
        .outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the manifest still reads");
}

// ---------------------------------------------------------------------------------------------
// The verified reader
// ---------------------------------------------------------------------------------------------

#[test]
fn the_manifest_reader_recomputes_the_digest_over_the_stored_bytes_on_every_read() {
    let (fixture, manifest, sources) = ordinary("byte-flip");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let digest = candidate.manifest_digest.clone();

    // The stored bytes, with one byte flipped, filed under the digest they no longer hash to. No
    // write path builds this row, which is exactly why the reader has to be the thing that catches
    // it: a reader that returns the row instead of recomputing would hand back a manifest whose
    // digest is a lie.
    let stored = fixture
        .outcomes
        .manifest(&digest)
        .unwrap()
        .canonical_bytes()
        .to_vec();
    let mut flipped = stored.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    assert_ne!(flipped, stored);
    with_trigger_down(
        &fixture.store,
        "outcome_manifests_no_update",
        "outcome_manifests",
        "Outcome manifests are immutable",
        |connection| {
            connection
                .execute(
                    "UPDATE outcome_manifests SET canonical_bytes = ?1 WHERE manifest_digest = ?2",
                    params![flipped, digest],
                )
                .unwrap();
        },
    );

    let refusal = fixture.outcomes.manifest(&digest).expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::BytesDisagree)
        ),
        "a flipped byte was not refused: {refusal:?}"
    );
}

#[test]
fn a_decoy_projection_column_cannot_change_what_the_reader_returns() {
    let (fixture, manifest, sources) = ordinary("decoy");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let digest = candidate.manifest_digest.clone();

    // Every projection lies; the bytes stay correct. The reader must follow the object, never the
    // column -- and here "follow the object" means refuse and name the column, because a row that
    // disagrees with its own bytes is invalid state rather than a differently-shaped record.
    // The digest-envelope pair is here because 0029 PERSISTS it: a reader that hard-codes the
    // envelope it returns cannot refuse a lie in either column, and hands back a v1 envelope over
    // bytes filed under something else (gpt2's cold read, P1 #1).
    for (column, lie) in [
        (
            "renderer_version",
            Value::from("beacon.outcome.md.v2".to_owned()),
        ),
        (
            "canonicalizer_version",
            Value::from("beacon.outcome.manifest.v2".to_owned()),
        ),
        ("mission_id", Value::from(Uuid::new_v4().to_string())),
        ("manifest_id", Value::from(Uuid::new_v4().to_string())),
        ("completion_digest", Value::from("aa".repeat(32))),
        ("digest_algorithm", Value::from("sha512".to_owned())),
        (
            "digest_canonicalizer",
            Value::from("beacon.outcome.manifest.v2".to_owned()),
        ),
        // The reader compared this one and nothing witnessed it: dropping it from `StoredRow`, the
        // SELECT and the comparison left all 21 tests green and clippy at exit 0 (gpt2's P2,
        // reproduced here before this row existed). A check with no witness is a check that can be
        // deleted by accident.
        (
            "boundary_event_seq",
            Value::from(i64::try_from(manifest.boundary_event_seq).unwrap() + 1),
        ),
    ] {
        let previous: Value = fixture
            .store
            .connect()
            .query_row(
                &format!("SELECT {column} FROM outcome_manifests WHERE manifest_digest = ?1"),
                params![digest],
                |row| row.get(0),
            )
            .unwrap();
        with_trigger_down(
            &fixture.store,
            "outcome_manifests_no_update",
            "outcome_manifests",
            "Outcome manifests are immutable",
            |connection| {
                connection
                    .pragma_update(None, "foreign_keys", "OFF")
                    .unwrap();
                connection
                    .pragma_update(None, "ignore_check_constraints", "ON")
                    .unwrap();
                connection
                    .execute(
                        &format!(
                            "UPDATE outcome_manifests SET {column} = ?1 WHERE manifest_digest = ?2"
                        ),
                        params![lie, digest],
                    )
                    .unwrap();
            },
        );
        let refusal = fixture.outcomes.manifest(&digest).expect_err("refuses");
        assert!(
            matches!(
                refusal,
                PrepareRefusal::Manifest(ManifestStoreRefusal::ColumnDisagrees { column: named })
                    if named == column
            ),
            "a lie in {column} was not refused by name: {refusal:?}"
        );
        with_trigger_down(
            &fixture.store,
            "outcome_manifests_no_update",
            "outcome_manifests",
            "Outcome manifests are immutable",
            |connection| {
                connection
                    .pragma_update(None, "foreign_keys", "OFF")
                    .unwrap();
                connection
                    .pragma_update(None, "ignore_check_constraints", "ON")
                    .unwrap();
                connection
                    .execute(
                        &format!(
                            "UPDATE outcome_manifests SET {column} = ?1 WHERE manifest_digest = ?2"
                        ),
                        params![previous, digest],
                    )
                    .unwrap();
            },
        );
        fixture
            .outcomes
            .manifest(&digest)
            .expect("the restored row reads");
    }
}

#[test]
fn a_manifest_digest_from_another_workspace_is_refused_rather_than_absent() {
    let (fixture, manifest, sources) = ordinary("cross-workspace");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let connection = fixture.store.connect();
    let stranger = OutcomeService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    let refusal = stranger
        .manifest(&candidate.manifest_digest)
        .expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::CrossWorkspace)
        ),
        "{refusal:?}"
    );
    // ...and a digest nothing holds is `Absent`, which is a different sentence.
    let absent = verified_manifest_within(&connection, fixture.workspace_id, &"bb".repeat(32))
        .expect_err("refuses");
    assert_eq!(absent, ManifestStoreRefusal::Absent);
}

// ---------------------------------------------------------------------------------------------
// prepare
// ---------------------------------------------------------------------------------------------

#[test]
fn prepare_is_idempotent_for_same_manifest_and_renderer_version() {
    let (fixture, manifest, sources) = ordinary("idempotent");
    let first = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let rows = |table: &str| -> i64 {
        fixture
            .store
            .connect()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    let manifests_after_first = rows("outcome_manifests");
    let artifacts_after_first = rows("artifacts");
    let events_after_first = rows("events");

    let second = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    assert_eq!(
        first, second,
        "a second prepare produced different identities"
    );
    assert_eq!(rows("outcome_manifests"), manifests_after_first);
    assert_eq!(rows("artifacts"), artifacts_after_first);
    assert_eq!(
        rows("events"),
        events_after_first,
        "a second prepare wrote an Event"
    );
    assert_eq!(manifests_after_first, 1);
}

#[test]
fn the_renderer_version_is_inside_the_bytes_and_an_unknown_one_stores_nothing() {
    let (fixture, manifest, sources) = ordinary("renderer-version");
    let first = fixture.outcomes.prepare(&manifest, &sources).unwrap();

    // "The same manifest bytes AND the same renderer version" is one condition, not two: the
    // renderer version is a manifest field, so it is already inside the bytes the digest covers.
    // The stored manifest proves it by decoding to the version it was prepared under.
    let stored = fixture.outcomes.manifest(&first.manifest_digest).unwrap();
    assert_eq!(stored.manifest().renderer_version, "beacon.outcome.md.v1");

    // And an unknown version is refused by the manifest's OWN codec, before a transaction is
    // opened -- earlier than the renderer, which is where an unknown version is also refused. A
    // caller cannot reach a store with one.
    let unknown = OutcomeManifestV1 {
        renderer_version: "beacon.outcome.md.v2".to_owned(),
        ..manifest
    };
    let refusal = fixture
        .outcomes
        .prepare(&unknown, &sources)
        .expect_err("refuses");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::Malformed(_))
        ),
        "an unknown renderer version was not refused by the codec: {refusal:?}"
    );
    let manifests: i64 = fixture
        .store
        .connect()
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(manifests, 1, "a refused manifest was stored");
}

#[test]
fn candidate_artifact_digest_covers_exact_markdown_bytes() {
    let (fixture, manifest, sources) = ordinary("exact-bytes");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let expected = render(&manifest, &sources).expect("renders");

    let record = fixture
        .outcomes
        .candidate(&candidate.manifest_digest)
        .expect("the candidate reads");
    assert_eq!(
        record.content(),
        expected.as_bytes(),
        "the candidate's content is not the exact rendered bytes"
    );
    assert_eq!(record.digest().value, candidate.candidate_artifact_digest);
    assert_eq!(
        record.payload().content_digest.value,
        data_encoding::HEXLOWER.encode(&sha2_256(expected.as_bytes())),
        "the content digest does not cover the exact Markdown bytes"
    );
    assert_eq!(record.payload().media_type, "text/markdown; charset=utf-8");
}

#[test]
fn candidate_accountable_author_equals_completion_accountable_resident() {
    let fixture = admitted_mission("author");
    // Deliberately the SECOND roster member, so a candidate authored by "the first Resident" or by
    // "the Operator" cannot pass by coincidence.
    let accountable = (fixture.other_resident_id, fixture.other_configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let record = fixture
        .outcomes
        .candidate(&candidate.manifest_digest)
        .unwrap();
    assert_eq!(
        record.payload().accountable_resident_id,
        completion.accountable_resident_id
    );
    assert_eq!(
        record.payload().accountable_configuration_id,
        completion.accountable_resident_configuration_id
    );
    assert_ne!(
        record.payload().accountable_resident_id,
        fixture.resident_id
    );
}

#[test]
fn the_accountable_author_comes_from_the_verified_record_not_the_column_beside_it() {
    let fixture = admitted_mission("author-column");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );

    // The completion row's accountable-Resident COLUMN now names the other roster member while its
    // bytes still name the first. A `prepare` that read the column would record a candidate
    // authored by the wrong Resident and report success; one that reads through the completion's
    // verified reader refuses, in the reader's own words.
    with_trigger_down(
        &fixture.store,
        "mission_completions_no_update",
        "mission_completions",
        "Mission completions are immutable",
        |connection| {
            connection
                .pragma_update(None, "foreign_keys", "OFF")
                .unwrap();
            connection
                .execute(
                    "UPDATE mission_completions SET accountable_resident_id = ?1
                     WHERE digest_value = ?2",
                    params![fixture.other_resident_id.to_string(), completion_digest],
                )
                .unwrap();
        },
    );

    let refusal = fixture
        .outcomes
        .prepare(&manifest, &sources)
        .expect_err("refuses");
    assert!(
        matches!(refusal, PrepareRefusal::Completion(_)),
        "the tampered column did not reach the completion's reader: {refusal:?}"
    );
    let manifests: i64 = fixture
        .store
        .connect()
        .query_row("SELECT count(*) FROM outcome_manifests", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(manifests, 0, "a refused prepare still stored a manifest");
}

#[test]
fn prepare_refuses_a_manifest_that_disagrees_with_its_completion() {
    let fixture = admitted_mission("reconcile");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (base, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );

    let disagreements: Vec<(&str, OutcomeManifestV1)> = vec![
        (
            "accountable Resident",
            OutcomeManifestV1 {
                accountable_resident_id: fixture.other_resident_id,
                ..base.clone()
            },
        ),
        (
            "boundary event",
            OutcomeManifestV1 {
                boundary_event_seq: base.boundary_event_seq + 1,
                ..base.clone()
            },
        ),
        (
            "completion recorded time",
            OutcomeManifestV1 {
                completion_recorded_at_ms: base.completion_recorded_at_ms + 1,
                ..base.clone()
            },
        ),
        (
            "mission",
            OutcomeManifestV1 {
                mission: source(
                    SourceKind::Mission,
                    Uuid::new_v4(),
                    beacond::MISSION_IDENTITY_ONLY,
                    &completion_digest,
                    1,
                    fixture.workspace_id,
                ),
                ..base.clone()
            },
        ),
        (
            "completion reference identity",
            OutcomeManifestV1 {
                completion: source(
                    SourceKind::Completion,
                    Uuid::new_v4(),
                    COMPLETION_CANONICALIZER,
                    &completion_digest,
                    1,
                    fixture.workspace_id,
                ),
                ..base.clone()
            },
        ),
    ];
    for (what, manifest) in disagreements {
        let refusal = fixture
            .outcomes
            .prepare(&manifest, &sources)
            .expect_err("refuses");
        match refusal {
            PrepareRefusal::Irreconcilable { detail } => assert!(
                detail.contains(what),
                "the refusal for {what} did not name it: {detail}"
            ),
            other => panic!("{what} was not refused as irreconcilable: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The appendix policy
// ---------------------------------------------------------------------------------------------

#[test]
fn high_impact_policy_forces_appendix_even_when_caller_requests_default() {
    let fixture = admitted_mission("high-impact");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::High);
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        true,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(manifest.appendix_request, AppendixRequest::PolicyDefault);
    assert_eq!(manifest.appendix_mode(), AppendixMode::Included);

    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap()
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(
        document.contains("appendix_mode: included"),
        "a high-impact Mission's document omitted the appendix:\n{document}"
    );
}

#[test]
fn a_manifest_that_understates_the_policy_bit_is_refused_rather_than_rendered() {
    let fixture = admitted_mission("policy-bit");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::High);
    // The caller writes `false` beside a high-impact classification -- the one move that would
    // otherwise buy a caller an omitted appendix, since the appendix mode is derived from this bit.
    let (manifest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(
        manifest.appendix_mode(),
        AppendixMode::PolicyStateNotHighImpact,
        "the arrangement does not exercise the dodge it exists to close"
    );
    let refusal = fixture
        .outcomes
        .prepare(&manifest, &sources)
        .expect_err("refuses");
    assert!(
        matches!(refusal, PrepareRefusal::Classification(_)),
        "the understated bit was not caught against the classification: {refusal:?}"
    );
}

#[test]
fn ordinary_policy_allows_explicit_include_but_api_has_no_force_omit() {
    let fixture = admitted_mission("ordinary-include");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);

    let (included, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::Include,
    );
    assert_eq!(included.appendix_mode(), AppendixMode::Included);
    let candidate = fixture.outcomes.prepare(&included, &sources).unwrap();
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap()
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(document.contains("appendix_mode: included"), "{document}");

    // The request vocabulary is closed at two arms and neither omits. Two guards, because one was
    // not enough: an earlier version of this test counted a hand-written array, and a `force_omit`
    // arm added to the enum passed it -- a guard that cannot fail is a false green, measured here.
    //
    // First, at compile time. This match is exhaustive TODAY, so a third arm makes this test stop
    // compiling: a closed vocabulary is extended by a named change, never in silence.
    for request in [AppendixRequest::PolicyDefault, AppendixRequest::Include] {
        match request {
            AppendixRequest::PolicyDefault | AppendixRequest::Include => {}
        }
        // Second, the property the vocabulary exists to protect: whatever the caller asks for, a
        // high-impact classification still resolves to `Included`. An arm that suppressed the
        // appendix would have to break this to be worth adding.
        let mandatory = OutcomeManifestV1 {
            policy_high_impact: true,
            appendix_request: request,
            ..included.clone()
        };
        assert_eq!(
            mandatory.appendix_mode(),
            AppendixMode::Included,
            "a caller's request suppressed a mandatory appendix"
        );
    }
    let (default, _) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    assert_eq!(
        default.appendix_mode(),
        AppendixMode::PolicyStateNotHighImpact
    );
    let ordinary_document = render(&default, &sources).unwrap();
    assert!(
        ordinary_document.contains("appendix_mode: policy_state_not_high_impact"),
        "the ordinary document did not print its appendix policy state:\n{ordinary_document}"
    );
}

// ---------------------------------------------------------------------------------------------
// Retention and restart
// ---------------------------------------------------------------------------------------------

#[test]
fn manifest_bytes_remain_retrievable_for_every_retained_outcome_version() {
    // Two prepared manifests over one Mission -- the shape a correction takes once versions arrive
    // in 3-B. Both sets of canonical bytes must still be retrievable and still recompute their own
    // digests: an appendix lets a reader recompute a digest from the exported Markdown, and a
    // superseded version whose bytes were dropped makes that impossible for the record it names.
    let (fixture, first_manifest, sources) = ordinary("retained");
    let first = fixture.outcomes.prepare(&first_manifest, &sources).unwrap();
    let second_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..first_manifest.clone()
    };
    let second = fixture
        .outcomes
        .prepare(&second_manifest, &sources)
        .unwrap();
    assert_ne!(first.manifest_digest, second.manifest_digest);

    for (candidate, expected) in [(&first, &first_manifest), (&second, &second_manifest)] {
        let stored = fixture
            .outcomes
            .manifest(&candidate.manifest_digest)
            .unwrap();
        assert_eq!(stored.manifest(), expected);
        assert_eq!(
            stored.canonical_bytes(),
            expected.canonical_bytes().unwrap().as_slice()
        );
        assert_eq!(
            data_encoding::HEXLOWER.encode(&sha2_256(stored.canonical_bytes())),
            candidate.manifest_digest,
            "the retained bytes no longer recompute their own digest"
        );
    }
}

#[test]
fn prepare_survives_restart_without_activity_or_trace() {
    let (fixture, manifest, sources) = ordinary("restart");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let expected = render(&manifest, &sources).unwrap();

    fixture.restart();

    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    assert_eq!(
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("the store unlocks")
            .phase,
        OperatorSessionPhase::Unlocked
    );
    let outcomes = OutcomeService::new(Arc::clone(&accounts), fixture.workspace_id);

    let stored = outcomes
        .manifest(&candidate.manifest_digest)
        .expect("the manifest reads back after restart");
    assert_eq!(stored.manifest(), &manifest);
    assert_eq!(stored.manifest_id(), candidate.manifest_id);
    assert_eq!(
        stored.candidate_artifact_digest(),
        candidate.candidate_artifact_digest
    );
    let record = outcomes
        .candidate(&candidate.manifest_digest)
        .expect("the candidate reads back after restart");
    assert_eq!(record.content(), expected.as_bytes());
}

#[test]
fn the_manifests_reference_identity_is_derived_from_its_content_identity() {
    let (fixture, manifest, sources) = ordinary("reference-identity");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    assert_eq!(
        candidate.manifest_id,
        beacond::manifest_reference_id(&candidate.manifest_digest).unwrap(),
        "the stored reference identity is not a function of the content identity"
    );
    // Which is the property that makes it safe to store beside the bytes: no citation can name a
    // UUID and a digest that disagree, because one IS the other's first sixteen bytes.
    let digest_head = data_encoding::HEXLOWER
        .decode(candidate.manifest_digest.as_bytes())
        .unwrap();
    assert_eq!(candidate.manifest_id.as_bytes(), &digest_head[..16]);
}

// ---------------------------------------------------------------------------------------------
// The startup pass (R18.7)
// ---------------------------------------------------------------------------------------------

/// Inserts one manifest row directly, behind the ledger, naming `candidate` as its candidate.
///
/// Every column is self-consistent: the bytes hash to the digest, the projections repeat what the
/// bytes say, and every foreign key is satisfied. No write path produces this row -- which is the
/// point, because a store that only re-proves on READ answers for it as though `prepare` had made
/// it.
fn insert_manifest_behind_the_ledger(
    fixture: &Fixture,
    manifest: &OutcomeManifestV1,
    candidate_artifact_digest: &str,
) -> String {
    let bytes = manifest.canonical_bytes().unwrap();
    let digest = data_encoding::HEXLOWER.encode(&sha2_256(&bytes));
    let manifest_id = beacond::manifest_reference_id(&digest).unwrap();
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO outcome_manifests (
                 workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                 canonical_bytes, manifest_id, mission_id, completion_digest,
                 canonicalizer_version, renderer_version, boundary_event_seq,
                 candidate_artifact_digest
             ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                       ?9, ?10)",
            params![
                fixture.workspace_id.to_string(),
                digest,
                bytes,
                manifest_id.to_string(),
                manifest.mission.object_id.to_string(),
                manifest.completion.digest.value.as_str(),
                manifest.canonicalizer_version.as_str(),
                manifest.renderer_version.as_str(),
                i64::try_from(manifest.boundary_event_seq).unwrap(),
                candidate_artifact_digest,
            ],
        )
        .unwrap();
    digest
}

#[test]
fn an_outcome_manifest_no_artifact_event_reaches_refuses_startup() {
    let (fixture, manifest, sources) = ordinary("orphan");
    let real = fixture.outcomes.prepare(&manifest, &sources).unwrap();

    // A REAL, ledger-accounted Artifact -- recorded through the service, so it has its own
    // `artifact.recorded` Event and satisfies every foreign key -- but Workspace-scoped, and so
    // not the Outcome Document of this manifest's Mission by this manifest's accountable Resident.
    // The manifest row below claims it anyway.
    let unrelated = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Not this Mission's outcome\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .unwrap();
    let orphan_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest.clone()
    };
    let orphan =
        insert_manifest_behind_the_ledger(&fixture, &orphan_manifest, &unrelated.digest().value);
    assert_ne!(orphan, real.manifest_digest);

    // It reads clean: that is exactly the gap. Re-proving on every read cannot tell a row the
    // ledger accounts for from one inserted behind it.
    fixture
        .outcomes
        .manifest(&orphan)
        .expect("the orphan row reads clean through the verified reader");

    // And every foreign key is satisfied, so whatever refuses this row below, it is not the
    // schema. Measured before the fix: `foreign_key_violations: 0, status: Ready`.
    assert_eq!(
        fixture
            .store
            .connect()
            .query_row("PRAGMA foreign_key_check", [], |row| row
                .get::<_, String>(0))
            .optional()
            .unwrap(),
        None,
        "the arrangement leans on a foreign key rather than on the startup pass"
    );

    let refusal = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("startup accepted a manifest the ledger does not account for");
    assert!(
        format!("{refusal:?}").contains("OutcomeManifestStateMismatch"),
        "the orphan was not refused by name: {refusal:?}"
    );
}

#[test]
fn a_fabricated_mission_digest_is_not_stored_read_back_or_rendered_as_verified() {
    // gpt2's cold read, P1 #2. `main` has no Mission declaration digest, so nothing could verify
    // this field -- and a fabricated value was accepted, returned by the reader, and printed in the
    // candidate Markdown underneath "Every fact below was verified at boundary event N".
    // gpt2's probe observed `read_back=true rendered=true blanket_verified=true`; at least one has
    // to be false, and this asserts exactly those three.
    let fixture = admitted_mission("mission-digest");
    let accountable = (fixture.resident_id, fixture.configuration_id);
    let (completion_digest, completion) = fixture.complete(accountable);
    let classification = fixture.classify(ImpactLevel::Ordinary);
    let (honest, sources) = manifest_for(
        &fixture,
        &completion_digest,
        &completion,
        &classification,
        false,
        AppendixRequest::PolicyDefault,
    );
    let fabricated = "de".repeat(32);
    let lying = OutcomeManifestV1 {
        mission: SourceRef {
            digest: SourceDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.mission.v1".to_owned(),
                value: fabricated.clone(),
            },
            ..honest.mission.clone()
        },
        ..honest.clone()
    };

    let refusal = fixture
        .outcomes
        .prepare(&lying, &sources)
        .expect_err("a fabricated Mission declaration digest was accepted by prepare");
    assert!(
        matches!(
            refusal,
            PrepareRefusal::Manifest(ManifestStoreRefusal::MissionCitationNotIdentityOnly)
        ),
        "{refusal:?}"
    );

    // read_back: nothing carries it.
    let stored_digest =
        data_encoding::HEXLOWER.encode(&sha2_256(&lying.canonical_bytes().unwrap()));
    let read_back = fixture.outcomes.manifest(&stored_digest).is_ok();
    // rendered / blanket_verified: no candidate exists to carry it.
    let rendered = fixture
        .store
        .connect()
        .query_row(
            "SELECT count(*) FROM artifacts WHERE instr(CAST(content_inline AS TEXT), ?1) > 0",
            params![fabricated],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        > 0;
    assert!(!read_back, "the fabricated digest read back");
    assert!(
        !rendered,
        "the fabricated digest reached a rendered document"
    );

    // The positive control, and what an honest citation says instead: the Mission is cited by
    // IDENTITY, anchored on the completion digest, and the citation's own canonicalizer token says
    // so beside the value in the rendered document -- so nothing claims a Mission declaration was
    // hashed (R18.4; the field itself goes with the R18.3-5 grammar revision).
    let candidate = fixture.outcomes.prepare(&honest, &sources).unwrap();
    let document = String::from_utf8(
        fixture
            .outcomes
            .candidate(&candidate.manifest_digest)
            .unwrap()
            .content()
            .to_vec(),
    )
    .unwrap();
    assert!(
        document.contains(beacond::MISSION_IDENTITY_ONLY),
        "the document does not say the Mission citation is identity-only:\n{document}"
    );
    assert!(
        document.contains(&completion_digest),
        "the Mission citation does not show its verified anchor"
    );
}

#[test]
fn a_manifest_cannot_exist_without_its_candidate() {
    // R18.6 rules that a prepared manifest is positioned by its candidate Artifact's
    // `artifact.recorded` Event rather than by an `outcome.prepared` kind of its own, and revisits
    // that only if a manifest can ever exist without a candidate. It cannot, and this is the test
    // that says so: the column is NOT NULL, and the foreign key requires a real Artifact in the
    // same Workspace.
    let (fixture, manifest, sources) = ordinary("no-candidate");
    fixture.outcomes.prepare(&manifest, &sources).unwrap();
    let orphan_manifest = OutcomeManifestV1 {
        appendix_request: AppendixRequest::Include,
        ..manifest
    };
    let bytes = orphan_manifest.canonical_bytes().unwrap();
    let digest = data_encoding::HEXLOWER.encode(&sha2_256(&bytes));
    let manifest_id = beacond::manifest_reference_id(&digest).unwrap();

    for (label, candidate) in [
        ("absent", None),
        ("naming no Artifact", Some("cc".repeat(32))),
    ] {
        let connection = fixture.store.connect();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        let error = connection
            .execute(
                "INSERT INTO outcome_manifests (
                     workspace_id, digest_algorithm, digest_canonicalizer, manifest_digest,
                     canonical_bytes, manifest_id, mission_id, completion_digest,
                     canonicalizer_version, renderer_version, boundary_event_seq,
                     candidate_artifact_digest
                 ) VALUES (?1, 'sha256', 'beacon.outcome.manifest.v1', ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                           ?9, ?10)",
                params![
                    fixture.workspace_id.to_string(),
                    digest,
                    bytes,
                    manifest_id.to_string(),
                    orphan_manifest.mission.object_id.to_string(),
                    orphan_manifest.completion.digest.value.as_str(),
                    orphan_manifest.canonicalizer_version.as_str(),
                    orphan_manifest.renderer_version.as_str(),
                    i64::try_from(orphan_manifest.boundary_event_seq).unwrap(),
                    candidate,
                ],
            )
            .expect_err("a manifest with a candidate {label} was stored");
        let error = error.to_string();
        assert!(
            error.contains("NOT NULL") || error.contains("FOREIGN KEY"),
            "a candidate {label} was refused for the wrong reason: {error}"
        );
    }
}

#[test]
fn a_prepared_manifest_still_reopens_after_the_startup_pass() {
    let (fixture, manifest, sources) = ordinary("startup-positive");
    let candidate = fixture.outcomes.prepare(&manifest, &sources).unwrap();
    // The positive control: the pass must not refuse the row `prepare` actually made, and the
    // record must still read back as itself afterwards.
    fixture.restart();
    let accounts = Arc::new(OperatorAccountService::open(&fixture.store.path).unwrap());
    assert_eq!(
        accounts
            .authenticate_password(
                SecretString::new("a long local passphrase".to_owned()),
                AccessOrigin::SameMachine,
            )
            .expect("the store unlocks")
            .phase,
        OperatorSessionPhase::Unlocked
    );
    let stored = OutcomeService::new(accounts, fixture.workspace_id)
        .manifest(&candidate.manifest_digest)
        .expect("the prepared manifest still reads after the startup pass");
    assert_eq!(stored.manifest(), &manifest);
}

fn sha2_256(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}
