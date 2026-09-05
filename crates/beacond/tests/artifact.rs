//! Artifact v1, slice 1 (ADR 0043): the canonical grammar and the identity, nothing else.
//!
//! Slice 1 owns no store, no Event, and no content bytes, so a test that needed one would be
//! testing a later slice. The canonical bytes are assembled twice in this file: once by Beacon's
//! encoder through `canonical_bytes`, and once by [`vector_bytes_by_hand`], which follows ADR
//! 0043's published field table rather than calling that encoder. The pinned digest was produced
//! independently of both, in Python, from the same table. An encoder that quietly changed its
//! layout would agree with itself and disagree with this file.

use beacond::{
    Artifact, ArtifactContext, ArtifactDigest, ArtifactKind, ArtifactV1, MalformedArtifact,
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const CANONICALIZER: &str = "beacon.artifact.v1";
const MAGIC: &[u8] = b"beacon.artifact.v1\0";
const RAW_CONTENT: &str = "beacon.raw.v1";
const MEDIA_MARKDOWN: &str = "text/markdown; charset=utf-8";

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const PROJECT: &str = "00000000-0000-4000-8000-000000000002";
const MISSION: &str = "00000000-0000-4000-8000-000000000003";
const RESIDENT: &str = "00000000-0000-4000-8000-000000000004";
const CONFIG: &str = "00000000-0000-4000-8000-000000000005";

const RECORDED_AT_MS: u64 = 1_788_000_000_000;
const CONTENT_BYTES: u64 = 11_638;

/// ADR 0043's published vector: 221 bytes, produced independently in Python from the field table.
const VECTOR_SHA256: &str = "83e3056b4cc665a458d279fa8602f9b137f87cc8032b5a5037afab614ed4eff0";

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("canonical uuid")
}

fn content_digest(value: [u8; 32]) -> ArtifactDigest {
    ArtifactDigest {
        algorithm: "sha256".to_owned(),
        canonicalizer: RAW_CONTENT.to_owned(),
        value: value.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

/// The vector record: one outcome-document Artifact, mission-scoped, first of its chain.
fn vector_artifact() -> ArtifactV1 {
    ArtifactV1 {
        workspace_id: uuid(WORKSPACE),
        kind: ArtifactKind::OutcomeDocument,
        media_type: MEDIA_MARKDOWN.to_owned(),
        content_digest: content_digest([0x66; 32]),
        content_bytes: CONTENT_BYTES,
        accountable_resident_id: uuid(RESIDENT),
        accountable_configuration_id: uuid(CONFIG),
        context: ArtifactContext::Mission {
            project_id: uuid(PROJECT),
            mission_id: uuid(MISSION),
        },
        supersedes: None,
        recorded_at_ms: RECORDED_AT_MS,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// An independent assembly: ADR 0043's published table, not Beacon's encoder
// ---------------------------------------------------------------------------------------------

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

/// The vector record's canonical bytes, assembled from ADR 0043's table by hand.
fn vector_bytes_by_hand() -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(uuid(WORKSPACE).as_bytes());
    out.push(0x01); // kind = outcome_document
    push_string(&mut out, MEDIA_MARKDOWN);
    push_string(&mut out, "sha256");
    push_string(&mut out, RAW_CONTENT);
    out.extend_from_slice(&[0x66; 32]);
    out.extend_from_slice(&CONTENT_BYTES.to_be_bytes());
    out.extend_from_slice(uuid(RESIDENT).as_bytes());
    out.extend_from_slice(uuid(CONFIG).as_bytes());
    out.push(0x01); // context = mission
    out.extend_from_slice(uuid(PROJECT).as_bytes());
    out.extend_from_slice(uuid(MISSION).as_bytes());
    out.push(0x00); // supersedes = none
    out.extend_from_slice(&RECORDED_AT_MS.to_be_bytes());
    out
}

/// Finds a needle in the canonical bytes, for refusal probes at exact positions.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("needle present")
}

// ---------------------------------------------------------------------------------------------
// The slice-1 tests of the design, one per bullet
// ---------------------------------------------------------------------------------------------

#[test]
fn a_known_artifact_fixture_reproduces_its_published_digest() {
    let bytes = vector_artifact().canonical_bytes().expect("encodes");
    assert_eq!(
        bytes,
        vector_bytes_by_hand(),
        "Beacon's encoder and ADR 0043's published table disagree"
    );
    assert_eq!(bytes.len(), 221, "the published vector's byte count moved");
    assert_eq!(
        sha256_hex(&bytes),
        VECTOR_SHA256,
        "the published vector's digest moved"
    );
    let digest = vector_artifact().digest().expect("digest");
    assert_eq!(digest.value, VECTOR_SHA256);
    assert_eq!(digest.algorithm, "sha256");
    assert_eq!(digest.canonicalizer, CANONICALIZER);
    // The identity is proved, not stored: the bytes-proved wrapper agrees.
    let proved = Artifact::from_canonical_bytes(&bytes).expect("verified");
    assert_eq!(proved.digest().value, VECTOR_SHA256);
    assert_eq!(proved.canonical_bytes(), bytes.as_slice());
    assert_eq!(proved.payload(), &vector_artifact());
}

#[test]
fn each_identity_bearing_field_changes_the_digest() {
    let base = vector_artifact().digest().expect("digest").value;
    let mut seen = vec![base];

    let mut mutations: Vec<(&str, ArtifactV1)> = Vec::new();
    let mut moved = vector_artifact();
    moved.workspace_id = uuid(PROJECT);
    mutations.push(("workspace", moved));
    let mut moved = vector_artifact();
    moved.content_digest = content_digest([0x67; 32]);
    mutations.push(("content digest value", moved));
    let mut moved = vector_artifact();
    moved.content_bytes += 1;
    mutations.push(("content byte count", moved));
    let mut moved = vector_artifact();
    moved.accountable_resident_id = uuid(CONFIG);
    mutations.push(("accountable Resident", moved));
    let mut moved = vector_artifact();
    moved.accountable_configuration_id = uuid(RESIDENT);
    mutations.push(("accountable Configuration", moved));
    let mut moved = vector_artifact();
    moved.context = ArtifactContext::None;
    mutations.push(("context", moved));
    let mut moved = vector_artifact();
    moved.supersedes = Some([0x99; 32]);
    mutations.push(("supersedes", moved));
    let mut moved = vector_artifact();
    moved.recorded_at_ms += 1;
    mutations.push(("recorded time", moved));

    for (label, manifest) in &mutations {
        let digest = manifest.digest().expect("digest").value;
        assert!(
            !seen.contains(&digest),
            "changing the {label} did not change the Artifact identity"
        );
        seen.push(digest);
    }
    assert_eq!(seen.len(), mutations.len() + 1);

    // The kind and the media type are identity-bearing too, but a writer cannot vary them alone:
    // the kind/media relation is closed, so the only spellings are refused ones (proved by the
    // refusal tests below), and a flipped kind byte in the bytes is an unknown tag (also below).
}

#[test]
fn one_changed_byte_trailing_byte_or_unknown_tag_is_refused() {
    let bytes = vector_bytes_by_hand();

    let mut broken_magic = bytes.clone();
    broken_magic[0] ^= 0xff;
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&broken_magic).expect_err("refuses"),
        MalformedArtifact::Magic
    );

    let mut trailing = bytes.clone();
    trailing.push(0x00);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&trailing).expect_err("refuses"),
        MalformedArtifact::TrailingBytes
    );

    let mut truncated = bytes.clone();
    truncated.pop();
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&truncated).expect_err("refuses"),
        MalformedArtifact::Truncated
    );

    // The kind byte sits immediately after the magic and the Workspace UUID. An unassigned tag
    // there is refused, not defaulted.
    let kind_offset = MAGIC.len() + 16;
    assert_eq!(bytes[kind_offset], 0x01);
    let mut unknown_tag = bytes.clone();
    unknown_tag[kind_offset] = 0x07;
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&unknown_tag).expect_err("refuses"),
        MalformedArtifact::UnknownTag
    );

    // A changed byte inside the content digest value changes the identity but still decodes:
    // it is a different Artifact, never a mistaken reading of this one.
    let mut other = bytes.clone();
    let digest_offset = find(&bytes, &[0x66; 32]);
    other[digest_offset] = 0x67;
    let decoded = ArtifactV1::from_canonical_bytes(&other).expect("decodes");
    assert_ne!(
        decoded.digest().expect("digest").value,
        VECTOR_SHA256,
        "a changed content digest byte did not change the identity"
    );
}

#[test]
fn a_zero_filled_buffer_is_not_a_valid_record() {
    let zeros = vec![0x00; 221];
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&zeros).expect_err("refuses"),
        MalformedArtifact::Magic
    );
    // With the magic intact, a zero-filled body fails at the kind tag: 0x00 is unassigned there,
    // so a zeroed buffer can never decode as a valid record.
    let mut body_zeros = vec![0x00; 221];
    body_zeros[..MAGIC.len()].copy_from_slice(MAGIC);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&body_zeros).expect_err("refuses"),
        MalformedArtifact::UnknownTag
    );
}

#[test]
fn a_media_type_the_kind_does_not_own_is_refused_by_the_codec() {
    // Encode direction: a caller cannot pair `outcome_document` with another media type.
    let mut wrong = vector_artifact();
    wrong.media_type = "text/plain".to_owned();
    assert_eq!(
        wrong.canonical_bytes().expect_err("refuses"),
        MalformedArtifact::MediaTypeForKind
    );

    // Decode direction: hand-built bytes naming another media type are refused at the same rule,
    // so a record that could not be written cannot be read back either.
    let mut bytes = vector_bytes_by_hand();
    let start = find(&bytes, MEDIA_MARKDOWN.as_bytes());
    let replacement = b"text/plain; charset=utf-8   ";
    assert_eq!(replacement.len(), MEDIA_MARKDOWN.len());
    bytes[start..start + MEDIA_MARKDOWN.len()].copy_from_slice(replacement);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&bytes).expect_err("refuses"),
        MalformedArtifact::MediaTypeForKind
    );
}

#[test]
fn an_unassigned_kind_tag_is_refused() {
    let kind_offset = MAGIC.len() + 16;
    for tag in [0x00_u8, 0x02, 0xff] {
        let mut bytes = vector_bytes_by_hand();
        bytes[kind_offset] = tag;
        assert_eq!(
            ArtifactV1::from_canonical_bytes(&bytes).expect_err("refuses"),
            MalformedArtifact::UnknownTag,
            "kind tag {tag:#04x} was accepted"
        );
    }
}

#[test]
fn a_record_over_4_kib_is_refused_before_any_allocation() {
    // The envelope is checked before the magic: an over-long input is refused as over-limit, not
    // as whatever its first bytes happen to fail.
    let mut over = vector_bytes_by_hand();
    over.resize(4_097, 0x00);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&over).expect_err("refuses"),
        MalformedArtifact::OverLimitLength
    );
    // The field table admits at most ~260 bytes, so the encoder cannot reach the envelope; the
    // bound is defensive and stated, in ADR 0038's shape.
    assert!(vector_bytes_by_hand().len() <= 4_096);
}

#[test]
fn a_declared_length_that_cannot_fit_the_remaining_input_is_refused_by_arithmetic() {
    let bytes = vector_bytes_by_hand();
    // The media type's u64 length prefix sits right after the kind byte.
    let length_offset = MAGIC.len() + 16 + 1;

    // Beyond the field's own limit: refused as over-limit before the remaining input is consulted.
    let mut over_limit = bytes.clone();
    over_limit[length_offset..length_offset + 8].copy_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&over_limit).expect_err("refuses"),
        MalformedArtifact::OverLimitLength
    );

    // Within the field's limit but beyond what remains: refused as truncated, by arithmetic,
    // before any byte is copied. u64::MAX above also proves the length arithmetic cannot overflow.
    let mut foreshortened = bytes.clone();
    foreshortened.truncate(length_offset + 8 + 4);
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&foreshortened).expect_err("refuses"),
        MalformedArtifact::Truncated
    );
}

#[test]
fn the_decoder_consumes_the_entire_input_and_reencode_is_byte_identical() {
    let bytes = vector_bytes_by_hand();
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, vector_artifact());
    assert_eq!(
        decoded.canonical_bytes().expect("re-encodes"),
        bytes,
        "decode and re-encode are not byte-identical"
    );
}

#[test]
fn none_arms_encode_as_values_and_a_missing_field_refuses() {
    // `context = none` and `supersedes = none` are values, never missing fields.
    let mut contextless = vector_artifact();
    contextless.context = ArtifactContext::None;
    let bytes = contextless.canonical_bytes().expect("encodes");
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(decoded, contextless);
    assert_eq!(decoded.context, ArtifactContext::None);

    // Removing the supersedes tag entirely leaves the record ending early: refused, not defaulted.
    let mut missing = bytes;
    missing.pop();
    missing.pop();
    missing.pop();
    assert_eq!(
        ArtifactV1::from_canonical_bytes(&missing).expect_err("refuses"),
        MalformedArtifact::Truncated
    );
}

#[test]
fn a_mission_scoped_context_carries_the_project_and_mission() {
    let artifact = vector_artifact();
    let bytes = artifact.canonical_bytes().expect("encodes");
    let decoded = ArtifactV1::from_canonical_bytes(&bytes).expect("decodes");
    assert_eq!(
        decoded.context,
        ArtifactContext::Mission {
            project_id: uuid(PROJECT),
            mission_id: uuid(MISSION),
        }
    );
    // The scope is inside the identity: the same record without it is a different Artifact.
    let mut contextless = vector_artifact();
    contextless.context = ArtifactContext::None;
    assert_ne!(
        contextless.digest().expect("digest").value,
        artifact.digest().expect("digest").value
    );
}

// ---------------------------------------------------------------------------------------------
// Slice 2: the store, the Event, the cursor, and the verified reader, through the real service
// ---------------------------------------------------------------------------------------------

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    ArtifactError, ArtifactService, AutonomyProfile, BeaconCapability, InconsistentArtifact,
    LocalModelActivationService, LocalModelEndpoint, MissionDraft, MissionMemberDraft,
    MissionService, OperatorAccountService, OperatorSessionPhase, ProjectCharterDraft,
    ProjectDraft, ProjectPolicyDraft, ProjectPolicyService, ProjectService, ProviderFeature,
    ResidentConfigurationService, ResidentLifecycleService, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    SupervisorError, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
use rusqlite::Connection;

const DOC_MARKDOWN: &[u8] = b"# Task Tracker outcome\n\nThe exporter landed.\n";

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-artifact-{label}-{}.sqlite3",
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
    artifacts: ArtifactService,
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

/// Builds a Workspace with one Project, one Policy revision, and one admitted Mission whose roster
/// holds two Residents at their exact admission Configurations.
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
    ProjectPolicyService::new(Arc::clone(&accounts), workspace_id)
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

    Fixture {
        artifacts: ArtifactService::new(Arc::clone(&accounts), workspace_id),
        store,
        accounts,
        workspace_id,
        project_id: project.id,
        mission_id: mission.id,
        resident_id: configurations[0].resident_id,
        configuration_id: configurations[0].id,
        other_resident_id: configurations[1].resident_id,
        other_configuration_id: configurations[1].id,
    }
}

impl Fixture {
    fn mission_context(&self) -> ArtifactContext {
        ArtifactContext::Mission {
            project_id: self.project_id,
            mission_id: self.mission_id,
        }
    }

    /// Records one workspace-scoped document from the accountable Resident.
    fn record_doc(&self, content: &[u8]) -> beacond::VerifiedArtifactRecord {
        self.artifacts
            .record(
                ArtifactKind::OutcomeDocument,
                content,
                ArtifactContext::None,
                self.resident_id,
                self.configuration_id,
                None,
            )
            .unwrap()
    }

    fn restart(&self) -> Result<beacond::StartupReport, SupervisorError> {
        Supervisor::bootstrap(StartupConfig {
            store_path: self.store.path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
    }
}

/// Runs `tamper` with the immutability trigger down, then restores it -- the evaluation.rs shape:
/// leaving the schema signature intact and the store holding a row nothing should have written.
fn with_update_trigger_down(store: &TestStore, tamper: impl FnOnce(&Connection)) {
    let connection = store.connect();
    connection
        .execute_batch("DROP TRIGGER artifacts_no_update;")
        .unwrap();
    tamper(&connection);
    connection
        .execute_batch(
            "CREATE TRIGGER artifacts_no_update
BEFORE UPDATE ON artifacts
BEGIN
    SELECT RAISE(ABORT, 'Artifacts are immutable');
END;",
        )
        .unwrap();
}

#[test]
fn recording_writes_one_row_one_event_and_its_cursor_in_one_transaction() {
    let fixture = admitted_mission("artifact-record");
    let artifact = fixture.record_doc(DOC_MARKDOWN);

    let connection = fixture.store.connect();
    let (rows, events): (i64, i64) = (
        connection
            .query_row("SELECT count(*) FROM artifacts", [], |row| row.get(0))
            .unwrap(),
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'artifact.recorded'",
                [],
                |row| row.get(0),
            )
            .unwrap(),
    );
    assert_eq!((rows, events), (1, 1), "one row, one Event, one record");

    let event_sequence: i64 = connection
        .query_row(
            "SELECT sequence FROM events WHERE kind = 'artifact.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        artifact.creation_event_seq(),
        event_sequence as u64,
        "the cursor is the paired Event's own sequence"
    );
    assert!(artifact.payload().recorded_at_ms > 0);
    assert_eq!(artifact.content(), DOC_MARKDOWN);
}

#[test]
fn identical_content_recorded_twice_is_two_artifacts() {
    let fixture = admitted_mission("artifact-twice");
    let first = fixture.record_doc(DOC_MARKDOWN);
    let second = fixture.record_doc(DOC_MARKDOWN);
    assert_ne!(
        first.digest().value,
        second.digest().value,
        "the accountability event differs, so the identity differs"
    );
    assert_eq!(first.content(), second.content());
}

#[test]
fn a_tampered_row_or_content_is_refused_at_read_and_at_startup() {
    let fixture = admitted_mission("artifact-tamper-row");
    let artifact = fixture.record_doc(DOC_MARKDOWN);

    // A row whose projection disagrees with its own bytes: refused at read. (The tamper targets a
    // column the DDL's CHECKs allow but the bytes contradict -- the media_type CHECK would make
    // the store itself refuse the write, which is a different, also-correct layer.)
    with_update_trigger_down(&fixture.store, |connection| {
        connection
            .execute(
                "UPDATE artifacts SET content_bytes = content_bytes + 1 WHERE digest_value = ?1",
                rusqlite::params![artifact.digest().value],
            )
            .unwrap();
    });
    assert!(
        matches!(
            fixture.artifacts.get(&artifact.digest().value),
            Err(ArtifactError::InvalidStore)
        ),
        "a projection that disagrees with the bytes was served"
    );
    // Startup refuses at the event-replay layer, which runs before the state layer and
    // re-proves the recorded Event's bytes against the row -- a mismatch there is the
    // deterministic outcome for this tamper.
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "a tampered projection did not refuse startup"
    );
}

#[test]
fn tampered_content_is_refused_at_read_and_at_startup() {
    let fixture = admitted_mission("artifact-tamper-content");
    let artifact = fixture.record_doc(DOC_MARKDOWN);

    with_update_trigger_down(&fixture.store, |connection| {
        connection
            .execute(
                "UPDATE artifacts SET content_inline = ?1 WHERE digest_value = ?2",
                rusqlite::params![b"# a forged document\n".as_slice(), artifact.digest().value],
            )
            .unwrap();
    });
    assert!(
        matches!(
            fixture.artifacts.get(&artifact.digest().value),
            Err(ArtifactError::InvalidStore)
        ),
        "content that does not hash to the record's digest was served"
    );
    // Same replay-layer refusal as the projection tamper above: the recorded Event's content
    // digest is re-proved against the row before the state layer runs.
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "tampered content did not refuse startup"
    );
}

#[test]
fn a_tampered_event_content_digest_envelope_refuses_startup() {
    let fixture = admitted_mission("artifact-event-content-envelope");
    let artifact = fixture.record_doc(DOC_MARKDOWN);
    let connection = fixture.store.connect();
    let changed = connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(
                 payload_json,
                 '$.artifact.contentDigest.algorithm', 'md5',
                 '$.artifact.contentDigest.canonicalizer', 'attacker'
             )
             WHERE kind = 'artifact.recorded'
               AND json_extract(payload_json, '$.artifact.digest.value') = ?1",
            rusqlite::params![artifact.digest().value],
        )
        .unwrap();
    assert_eq!(changed, 1, "the probe did not alter the paired Event");
    drop(connection);

    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "startup accepted an Event whose content-digest algorithm and canonicalizer were forged"
    );
}

#[test]
fn missing_duplicate_orphaned_or_differently_encoded_event_evidence_refuses_startup() {
    // Missing: the row with no Event.
    let fixture = admitted_mission("artifact-event-missing");
    fixture.record_doc(DOC_MARKDOWN);
    fixture
        .store
        .connect()
        .execute("DELETE FROM events WHERE kind = 'artifact.recorded'", [])
        .unwrap();
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactStateMismatch)
                | Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "a missing Event did not refuse startup"
    );

    // Duplicate: two Events, one record.
    let fixture = admitted_mission("artifact-event-duplicate");
    fixture.record_doc(DOC_MARKDOWN);
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, payload_json, occurred_at_ms
             FROM events WHERE kind = 'artifact.recorded'",
            [],
        )
        .unwrap();
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactStateMismatch)
                | Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "a duplicate Event did not refuse startup"
    );

    // Differently encoded: the same bytes, re-wrapped. Different evidence, not the same evidence
    // reformatted.
    let fixture = admitted_mission("artifact-event-reencoded");
    fixture.record_doc(DOC_MARKDOWN);
    fixture
        .store
        .connect()
        .execute(
            "UPDATE events SET payload_json = replace(payload_json, 'canonicalBytesBase64', 'bytes')
             WHERE kind = 'artifact.recorded'",
            [],
        )
        .unwrap();
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactStateMismatch)
                | Err(SupervisorError::ArtifactEventMismatch(_))
                | Err(SupervisorError::InvalidEventKind(_))
        ),
        "a re-keyed Event payload did not refuse startup"
    );

    // Orphan: an Event naming no stored Artifact.
    let fixture = admitted_mission("artifact-event-orphan");
    let artifact = fixture.record_doc(DOC_MARKDOWN);
    let connection = fixture.store.connect();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'artifact.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let orphan = payload.replace(&artifact.digest().value, &"0".repeat(64));
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'artifact.recorded', ?2, 1)",
            rusqlite::params![fixture.workspace_id.to_string(), orphan],
        )
        .unwrap();
    drop(connection);
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactStateMismatch)
                | Err(SupervisorError::ArtifactEventMismatch(_))
        ),
        "an orphan Event did not refuse startup"
    );
}

#[test]
fn the_creation_cursor_is_rederived_against_the_event_at_every_read() {
    let fixture = admitted_mission("artifact-cursor");
    let artifact = fixture.record_doc(DOC_MARKDOWN);

    with_update_trigger_down(&fixture.store, |connection| {
        connection
            .execute(
                "UPDATE artifacts SET creation_event_seq = creation_event_seq + 1
                 WHERE digest_value = ?1",
                rusqlite::params![artifact.digest().value],
            )
            .unwrap();
    });
    assert!(
        matches!(
            fixture.artifacts.get(&artifact.digest().value),
            Err(ArtifactError::InvalidStore)
        ),
        "a cursor that disagrees with the paired Event was trusted"
    );
    assert!(matches!(
        fixture.restart(),
        Err(SupervisorError::ArtifactStateMismatch)
    ));
}

#[test]
fn an_unauthenticated_caller_receives_only_the_authentication_refusal() {
    let fixture = admitted_mission("artifact-locked");
    fixture.accounts.lock().unwrap();

    // The request is ALSO inconsistent -- a Resident nothing admits -- so if any other check ran
    // first it would report that instead, and the authentication boundary would be decorative.
    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            ArtifactContext::None,
            Uuid::new_v4(),
            Uuid::new_v4(),
            None,
        )
        .expect_err("a locked session must refuse");
    assert!(
        matches!(error, ArtifactError::Account(_)),
        "authentication did not come first: {error:?}"
    );
}

#[test]
fn a_cross_workspace_read_is_not_found_never_a_value() {
    let fixture = admitted_mission("artifact-cross-workspace");
    let artifact = fixture.record_doc(DOC_MARKDOWN);

    // A service scoped to a Workspace that does not exist, asked for this Workspace's Artifact:
    // the answer is the refusal, not the record.
    let foreign = ArtifactService::new(Arc::clone(&fixture.accounts), Uuid::new_v4());
    assert!(
        matches!(
            foreign.get(&artifact.digest().value),
            Err(ArtifactError::NotFound)
        ),
        "a cross-Workspace read returned something other than the refusal"
    );
}

#[test]
fn recording_leaves_run_receipt_ruling_candidate_and_evaluation_counts_unchanged() {
    let fixture = admitted_mission("artifact-inert");
    let connection = fixture.store.connect();
    let counts = |connection: &Connection| -> Vec<i64> {
        [
            "SELECT count(*) FROM runs",
            "SELECT count(*) FROM receipts",
            "SELECT count(*) FROM rulings",
            "SELECT count(*) FROM candidates",
            "SELECT count(*) FROM evaluations",
        ]
        .iter()
        .map(|sql| connection.query_row(sql, [], |row| row.get(0)).unwrap())
        .collect()
    };
    let before = counts(&connection);
    drop(connection);
    fixture.record_doc(DOC_MARKDOWN);
    assert_eq!(before, counts(&fixture.store.connect()));
}

#[test]
fn a_mission_scoped_artifact_requires_the_accountable_resident_on_the_roster() {
    let fixture = admitted_mission("artifact-roster");

    // The roster's exact pairing: admitted member, admitted Configuration. Records.
    fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            fixture.mission_context(),
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("the admitted member under the admitted Configuration records");

    // A real Resident with a real Configuration of their own, accountable for an Artifact scoped
    // to a Mission whose roster does not hold them. The roster is the property, so the probe needs
    // a Mission the second Resident was never admitted to: form a one-member Team and admit it.
    let solo_team = TeamService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .form(TeamDraft {
            project_id: fixture.project_id,
            name: "Solo Team".to_owned(),
            members: vec![TeamMemberDraft {
                resident_id: fixture.resident_id,
                expected_configuration_id: fixture.configuration_id,
            }],
        })
        .unwrap();
    let second_mission = MissionService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .admit(MissionDraft {
            project_id: fixture.project_id,
            expected_charter_revision: 1,
            team_id: solo_team.id,
            desired_outcome: "A second outcome.".to_owned(),
            success_conditions: vec!["It holds.".to_owned()],
            constraints: vec!["Small.".to_owned()],
            prohibitions: vec!["None.".to_owned()],
            members: vec![MissionMemberDraft {
                resident_id: fixture.resident_id,
                expected_configuration_id: fixture.configuration_id,
            }],
        })
        .unwrap();
    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: second_mission.id,
            },
            fixture.other_resident_id,
            fixture.other_configuration_id,
            None,
        )
        .expect_err("a Resident the roster does not hold must refuse");
    assert!(
        matches!(
            error,
            ArtifactError::Inconsistent(InconsistentArtifact::NotAnAdmittedMember)
        ),
        "the refusal did not name the roster rule: {error:?}"
    );

    // A context Project that is not the Mission's: refused.
    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            ArtifactContext::Mission {
                project_id: Uuid::new_v4(),
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect_err("a context Project the Mission is not in must refuse");
    assert!(
        matches!(
            error,
            ArtifactError::Inconsistent(InconsistentArtifact::ProjectScopeMismatch)
        ),
        "the refusal did not name the scope rule: {error:?}"
    );

    // A Mission that does not exist: a different refusal.
    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: Uuid::new_v4(),
            },
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect_err("a Mission that does not exist must refuse");
    assert!(
        matches!(
            error,
            ArtifactError::Inconsistent(InconsistentArtifact::MissionNotFound)
        ),
        "the refusal did not name the missing Mission: {error:?}"
    );
}

#[test]
fn a_workspace_scoped_artifact_records_without_a_roster() {
    let fixture = admitted_mission("artifact-no-scope");
    let artifact = fixture.record_doc(DOC_MARKDOWN);
    assert_eq!(artifact.payload().context, ArtifactContext::None);
    assert_eq!(artifact.superseded_by(), None);
}

#[test]
fn a_correction_cites_the_current_tail_and_preserves_the_superseded_record() {
    let fixture = admitted_mission("artifact-correction");
    let first = fixture.record_doc(DOC_MARKDOWN);
    let corrected = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Task Tracker outcome\n\nThe exporter landed, reviewed.\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .unwrap();

    // The cited record stays readable and whole, and says who supersedes it -- beside, never
    // instead.
    let reread = fixture.artifacts.get(&first.digest().value).unwrap();
    assert_eq!(reread.content(), DOC_MARKDOWN);
    assert_eq!(
        reread.superseded_by(),
        Some(corrected.digest().value.as_str()),
        "the cited record does not name its successor at read time"
    );
    assert_eq!(corrected.superseded_by(), None);
}

#[test]
fn citing_an_already_superseded_artifact_is_refused_as_not_the_current_tail() {
    let fixture = admitted_mission("artifact-tail");
    let first = fixture.record_doc(DOC_MARKDOWN);
    fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# v2\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .unwrap();

    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# v3 off the wrong link\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .expect_err("citing a superseded record must refuse");
    assert!(
        matches!(
            error,
            ArtifactError::Inconsistent(InconsistentArtifact::NotTheCurrentTail)
        ),
        "the refusal did not name the tail rule: {error:?}"
    );
}

#[test]
fn a_correction_with_a_different_context_is_refused() {
    let fixture = admitted_mission("artifact-context-drift");
    let first = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            DOC_MARKDOWN,
            fixture.mission_context(),
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .unwrap();
    let error = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# the same document, re-scoped\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .expect_err("a correction that changes the scope must refuse");
    assert!(
        matches!(
            error,
            ArtifactError::Inconsistent(InconsistentArtifact::WrongContext)
        ),
        "the refusal did not name the scope rule: {error:?}"
    );
}

#[test]
fn get_returns_the_cited_digest_and_the_current_tail_beside_it() {
    let fixture = admitted_mission("artifact-tail-listing");
    let first = fixture.record_doc(DOC_MARKDOWN);
    let second = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# v2\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .unwrap();

    // The cited digest answers with itself, and the tail beside it: both links, never the tail
    // silently in place of the link (R12 item 10).
    let tail = fixture
        .artifacts
        .list_supersession_tail(&first.digest().value)
        .unwrap();
    assert_eq!(
        tail.iter().map(|digest| &digest.value).collect::<Vec<_>>(),
        vec![&first.digest().value, &second.digest().value],
        "the tail is not the chain in chain order"
    );
}

#[test]
fn update_and_delete_against_a_recorded_artifact_are_refused_by_the_store() {
    let fixture = admitted_mission("artifact-immutable");
    let artifact = fixture.record_doc(DOC_MARKDOWN);
    let connection = fixture.store.connect();
    assert!(
        connection
            .execute(
                "UPDATE artifacts SET content_bytes = content_bytes + 1 WHERE digest_value = ?1",
                rusqlite::params![artifact.digest().value],
            )
            .is_err(),
        "UPDATE was not refused by the store"
    );
    assert!(
        connection
            .execute(
                "DELETE FROM artifacts WHERE digest_value = ?1",
                rusqlite::params![artifact.digest().value],
            )
            .is_err(),
        "DELETE was not refused by the store"
    );
}

#[test]
fn a_supersession_the_bytes_contradict_refuses_startup() {
    let fixture = admitted_mission("artifact-forked-relation");
    let first = fixture.record_doc(DOC_MARKDOWN);
    let second = fixture
        .artifacts
        .record(
            ArtifactKind::OutcomeDocument,
            b"# v2\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            Some(digest_raw(&first.digest().value)),
        )
        .unwrap();
    let third = fixture.record_doc(b"# an independent document\n");

    // The store itself refuses a second successor to one predecessor.
    let connection = fixture.store.connect();
    assert!(
        connection
            .execute(
                "INSERT INTO artifact_supersessions
                     (workspace_id, artifact_digest, supersedes_digest)
                 VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    fixture.workspace_id.to_string(),
                    third.digest().value,
                    first.digest().value
                ],
            )
            .is_err(),
        "the store did not refuse one record superseded twice"
    );

    // And a relation the bytes contradict: retarget the successor's link at the independent
    // document, leaving the bytes naming the true predecessor.
    connection
        .execute_batch("DROP TRIGGER artifact_supersessions_no_update;")
        .unwrap();
    connection
        .execute(
            "UPDATE artifact_supersessions SET supersedes_digest = ?1
             WHERE artifact_digest = ?2",
            rusqlite::params![third.digest().value, second.digest().value],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER artifact_supersessions_no_update
BEFORE UPDATE ON artifact_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Artifact supersessions are immutable');
END;",
        )
        .unwrap();
    drop(connection);
    assert!(
        matches!(
            fixture.restart(),
            Err(SupervisorError::ArtifactStateMismatch)
        ),
        "a relation the bytes contradict did not refuse startup"
    );
}

#[test]
fn a_store_at_the_previous_version_migrates_forward_and_reconciles() {
    // The ladder step: a store declared at 26 must reach 27 and pass reconciliation. The migration
    // list is read from the directory, not copied, so the test cannot drift from the files.
    let store = TestStore::new("artifact-ladder");
    let migrations_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut files: Vec<_> = fs::read_dir(&migrations_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".sql"))
        .collect();
    files.sort();
    // The fixture is the store at this stack's parent version: every migration up to it, none
    // past it. The tripwire fires when the parent moves (a later lane landing first), which is
    // when this fixture must be regenerated -- never silently re-based. The parent is 0026: the
    // completion store landed first and this lane renumbered at its final rebase.
    files.retain(|name| name[..4].parse::<u32>().unwrap() <= 26);
    assert_eq!(
        files.last().map(|name| &name[..4]),
        Some("0026"),
        "the fixture's parent moved"
    );
    {
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        for name in &files {
            connection
                .execute_batch(&fs::read_to_string(migrations_dir.join(name)).unwrap())
                .unwrap();
        }
        let version: i64 = connection
            .query_row(
                "SELECT version FROM schema_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            version, 26,
            "the fixture is not a store at the previous version"
        );
    }
    Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .expect("a store at the previous version did not migrate forward");
    let connection = store.connect();
    let version: i64 = connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 30, "the ladder did not reach the current version");
    connection
        .query_row("SELECT count(*) FROM artifacts", [], |row| {
            row.get::<_, i64>(0)
        })
        .expect("the artifact tables do not exist after the ladder");
}

fn digest_raw(hex: &str) -> [u8; 32] {
    let decoded = data_encoding::HEXLOWER.decode(hex.as_bytes()).unwrap();
    let mut bytes = [0_u8; 32];
    bytes.copy_from_slice(&decoded);
    bytes
}
