//! Evaluation v1 through the real service (ADR 0038), slice 1.
//!
//! The Proposal fixture is the one `proposal.rs` uses, including its byte-level [`Record`] builder,
//! so the Proposal an Evaluation binds to is assembled from the published Proposal grammar rather
//! than by calling Beacon's encoder.
//!
//! Nothing here is a permission check. An Evaluation records that somebody judged; every test that
//! looks like an authority test is checking that no authority was created.

use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use beacond::{
    ArtifactContext, ArtifactDigest, ArtifactKind, ArtifactService, AutonomyProfile,
    BeaconCapability, Conflicts, DeclaredCheck, EvaluationDigest, EvaluationDraft, EvaluationError,
    EvaluationService, EvaluationSubjectDraft, EvaluationSupersedes, Evaluator, Finding,
    FindingSeverity, InconsistentEvaluation, LocalModelActivationService, LocalModelEndpoint,
    MalformedEvaluation, MissionDraft, MissionMemberDraft, MissionService, OperatorAccountService,
    OperatorSessionPhase, ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft,
    ProjectPolicyService, ProjectRepositoryService, ProjectService, ProposalService,
    ProviderFeature, PublicationSeamRefusal, RepositoryDraft, RepositorySourceKind,
    ResidentConfigurationService, ResidentLifecycleService, RubricDeclaration, SecretString,
    StarterOrganizationService, StarterResidentDraft, StarterRole, StartupConfig, Supervisor,
    TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule,
    TeamLeadSelectionPolicy, TeamMemberDraft, TeamService, TrackRecordEvidenceKind,
    TrackRecordRequirement, UnmetPrecondition, Verdict, publication_candidacy,
};
use data_encoding::{BASE64, HEXLOWER};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// A fabricated Repository Binding. Neither the path nor the commit id names anything that exists,
/// and the patterned revision is meant to be unmistakably synthetic.
const REPOSITORY_SOURCE: &str = "/home/operator/projects/sample-tracker";
const BASE_REVISION: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const MODEL_NAME: &str = "qwen3.8:27b";
const MODEL_DIGEST: &str = "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643";

const MAGIC: &[u8] = b"beacon.proposal.v1\0";

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(bytes);
    let output = digest.finalize();
    let mut value = [0_u8; 32];
    value.copy_from_slice(&output[..]);
    value
}

// ---------------------------------------------------------------------------------------------
// An independent encoder for the published grammar
// ---------------------------------------------------------------------------------------------

/// Narrower than `proposal.rs`'s builder on purpose: this file only ever builds the Proposal
/// shapes an Evaluation binds to, and a variant nothing constructs is dead weight in a fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Context {
    Declared {
        entries: Vec<(String, [u8; 32])>,
        /// `None` means "the digest of the encoded entry list", which is what a truthful record
        /// carries. `Some` is how a test states a manifest that does not match.
        manifest: Option<[u8; 32]>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Constraint {
    NotDeclared,
}

/// One canonical record, field by field, so a test can change exactly one thing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Record {
    magic: Vec<u8>,
    workspace_id: Uuid,
    project_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
    configuration_id: Uuid,
    bootstrap_tag: u8,
    bootstrap_reason: String,
    charter_revision: u32,
    policy_revision: u32,
    repository_revision: u32,
    base_revision: String,
    summary: String,
    context: Context,
    targets: Vec<String>,
    mutations: Vec<(u8, String)>,
    check_runs: Vec<String>,
    checks: Vec<(String, Vec<String>, String)>,
    requirements: Vec<Constraint>,
    bounds: [u64; 6],
    budget_tag: u8,
    rollback_tag: u8,
}

fn push_string(out: &mut Vec<u8>, value: &str) {
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

fn push_count(out: &mut Vec<u8>, count: usize) {
    out.extend_from_slice(&(count as u32).to_be_bytes());
}

fn encoded_entries(entries: &[(String, [u8; 32])]) -> Vec<u8> {
    let mut out = Vec::new();
    push_count(&mut out, entries.len());
    for (path, digest) in entries {
        push_string(&mut out, path);
        out.extend_from_slice(digest);
    }
    out
}

impl Record {
    fn bytes(&self) -> Vec<u8> {
        let mut out = self.magic.clone();
        for id in [
            self.workspace_id,
            self.project_id,
            self.mission_id,
            self.resident_id,
            self.configuration_id,
        ] {
            out.extend_from_slice(id.as_bytes());
        }
        out.push(self.bootstrap_tag);
        push_string(&mut out, &self.bootstrap_reason);

        for revision in [
            self.charter_revision,
            self.policy_revision,
            self.repository_revision,
        ] {
            out.extend_from_slice(&revision.to_be_bytes());
        }
        push_string(&mut out, &self.base_revision);

        push_string(&mut out, &self.summary);
        match &self.context {
            Context::Declared { entries, manifest } => {
                out.push(0x02);
                let encoded = encoded_entries(entries);
                out.extend_from_slice(&encoded);
                out.extend_from_slice(&manifest.unwrap_or_else(|| sha256(&encoded)));
            }
        }
        push_count(&mut out, self.targets.len());
        for target in &self.targets {
            push_string(&mut out, target);
        }
        push_count(&mut out, self.mutations.len());
        for (tag, target) in &self.mutations {
            out.push(*tag);
            push_string(&mut out, target);
        }
        push_count(&mut out, self.check_runs.len());
        for name in &self.check_runs {
            push_string(&mut out, name);
        }
        push_count(&mut out, self.checks.len());
        for (name, argv, working_directory) in &self.checks {
            push_string(&mut out, name);
            push_count(&mut out, argv.len());
            for argument in argv {
                push_string(&mut out, argument);
            }
            push_string(&mut out, working_directory);
        }

        for constraint in &self.requirements {
            match constraint {
                Constraint::NotDeclared => out.push(0x01),
            }
        }
        for bound in self.bounds {
            out.extend_from_slice(&bound.to_be_bytes());
        }
        out.push(self.budget_tag);
        out.push(self.rollback_tag);
        out
    }
}

// ---------------------------------------------------------------------------------------------
// One admitted Mission, built the way an Operator would build it
// ---------------------------------------------------------------------------------------------

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-proposal-{label}-{}.sqlite3",
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
    proposals: ProposalService,
}

fn tags_response() -> String {
    format!(
        "{{\"models\":[{{\"name\":\"{MODEL_NAME}\",\"model\":\"{MODEL_NAME}\",\
         \"modified_at\":\"2026-08-14T14:38:20.13373148-04:00\",\"size\":17741872154,\
         \"digest\":\"{MODEL_DIGEST}\",\"details\":{{\"format\":\"gguf\",\"family\":\"qwen35\",\
         \"families\":[\"qwen35\"],\"parameter_size\":\"27.3B\",\
         \"quantization_level\":\"Q4_K_M\"}}}}]}}"
    )
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

fn declared_checks() -> Vec<DeclaredCheck> {
    vec![
        DeclaredCheck {
            name: "cargo-test".to_owned(),
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--workspace".to_owned(),
            ],
            working_directory: ".".to_owned(),
        },
        DeclaredCheck {
            name: "ruff".to_owned(),
            argv: vec!["ruff".to_owned(), "check".to_owned(), ".".to_owned()],
            working_directory: ".".to_owned(),
        },
    ]
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

/// Builds a Workspace with one Project, one Policy revision, one Repository revision, and one
/// admitted Mission whose roster holds two Residents at their exact admission Configurations.
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
        .publish(project.id, project.charter.revision, None, policy_draft())
        .unwrap();
    ProjectRepositoryService::new(Arc::clone(&accounts), workspace_id)
        .bind(
            project.id,
            project.charter.revision,
            None,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: REPOSITORY_SOURCE.to_owned(),
                base_revision: BASE_REVISION.to_owned(),
                declared_checks: declared_checks(),
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
    let review = models.inspect(MODEL_NAME, MODEL_DIGEST).unwrap();
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
        store,
        proposals: ProposalService::new(Arc::clone(&accounts), workspace_id),
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
    /// The truthful record for this Mission: every reference resolves, every list is canonical.
    fn record(&self) -> Record {
        Record {
            magic: MAGIC.to_vec(),
            workspace_id: self.workspace_id,
            project_id: self.project_id,
            mission_id: self.mission_id,
            resident_id: self.resident_id,
            configuration_id: self.configuration_id,
            bootstrap_tag: 0x01,
            bootstrap_reason: "The Owner selected this Resident to request this one Proposal."
                .to_owned(),
            charter_revision: 1,
            policy_revision: 1,
            repository_revision: 1,
            base_revision: BASE_REVISION.to_owned(),
            summary: "Add a regression test for the invariant checker.".to_owned(),
            context: Context::Declared {
                entries: vec![("src/lib.rs".to_owned(), [0x66; 32])],
                manifest: None,
            },
            targets: vec!["src/lib.rs".to_owned(), "tests/invariants.rs".to_owned()],
            mutations: vec![
                (0x02, "src/lib.rs".to_owned()),
                (0x01, "tests/invariants.rs".to_owned()),
            ],
            check_runs: vec!["cargo-test".to_owned()],
            checks: vec![(
                "cargo-test".to_owned(),
                vec![
                    "cargo".to_owned(),
                    "test".to_owned(),
                    "--workspace".to_owned(),
                ],
                ".".to_owned(),
            )],
            requirements: vec![Constraint::NotDeclared; 7],
            bounds: [300_000, 240, 2 << 30, 256 << 20, 1 << 20, 0],
            budget_tag: 0x01,
            rollback_tag: 0x01,
        }
    }

    fn rebind_repository(
        &self,
        prior: u32,
        source_kind: RepositorySourceKind,
        source: &str,
        base_revision: &str,
    ) -> u32 {
        ProjectRepositoryService::new(Arc::clone(&self.accounts), self.workspace_id)
            .bind(
                self.project_id,
                1,
                Some(prior),
                RepositoryDraft {
                    source_kind,
                    source: source.to_owned(),
                    base_revision: base_revision.to_owned(),
                    declared_checks: declared_checks(),
                },
            )
            .unwrap()
            .revision
    }

    fn count(&self, sql: &str) -> i64 {
        self.store
            .connect()
            .query_row(sql, [], |row| row.get::<_, i64>(0))
            .unwrap()
    }
}

fn evaluation_draft(fixture: &Fixture, proposal_digest: &str) -> EvaluationDraft {
    EvaluationDraft {
        subject: EvaluationSubjectDraft::Proposal(proposal_digest.to_owned()),
        // The OTHER Resident: the author cannot evaluate their own Proposal, and slice 2 makes
        // that structural. Slice 1 uses a non-author because the fixture has one.
        evaluator: Evaluator {
            resident_id: fixture.other_resident_id,
            configuration_id: fixture.other_configuration_id,
        },
        rubric: RubricDeclaration {
            id: "security-review-v1".to_owned(),
            version: 1,
            canonicalizer: "beacon.rubric.v1".to_owned(),
            sha256: HEXLOWER.encode(&[0x20_u8; 32]),
        },
        cited_runs: Vec::new(),
        verdict: Verdict::Uncertain,
        findings: vec![Finding {
            severity: FindingSeverity::Note,
            summary: "Read the exact bytes.".to_owned(),
        }],
        conflicts: Conflicts::None,
        rationale: "Independent evaluator reviewed the exact Proposal bytes.".to_owned(),
        supersedes: EvaluationSupersedes::None,
    }
}

fn counts(fixture: &Fixture) -> (i64, i64, i64) {
    (
        fixture.count("SELECT count(*) FROM runs"),
        fixture.count("SELECT count(*) FROM receipts"),
        fixture.count("SELECT count(*) FROM events"),
    )
}

#[test]
fn an_evaluation_binds_to_the_exact_proposal_bytes_and_reads_back_verified() {
    let fixture = admitted_mission("evaluation-records");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("the Evaluation records");

    // The copied envelope is the bound Proposal's OWN, not this module's constants restated.
    assert_eq!(
        evaluation.payload().proposal_subject(),
        Some(proposal.digest())
    );
    assert_eq!(evaluation.digest().canonicalizer, "beacon.evaluation.v1");
    assert_eq!(evaluation.digest().algorithm, "sha256");
    assert_eq!(
        HEXLOWER.encode(&sha256(evaluation.canonical_bytes())),
        evaluation.digest().value,
        "the digest is not the hash of the stored bytes"
    );

    // And a read re-derives all of it rather than trusting the row.
    let reread = evaluations.get(&evaluation.digest().value).unwrap();
    assert_eq!(reread, evaluation);
}

#[test]
fn the_copied_proposal_envelope_equals_the_bound_proposals_own() {
    let fixture = admitted_mission("evaluation-envelope");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let connection = fixture.store.connect();
    let (value, algorithm, canonicalizer): (String, String, String) = connection
        .query_row(
            "SELECT proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer
             FROM evaluations WHERE digest_value = ?1",
            [evaluation.digest().value.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(value, proposal.digest().value);
    assert_eq!(algorithm, proposal.digest().algorithm);
    assert_eq!(canonicalizer, proposal.digest().canonicalizer);
}

#[test]
fn an_evaluation_of_a_digest_that_names_no_stored_proposal_writes_neither_row_nor_event() {
    let fixture = admitted_mission("evaluation-unknown-proposal");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let before = counts(&fixture);

    let error = evaluations
        .record(evaluation_draft(&fixture, &HEXLOWER.encode(&[0x5a_u8; 32])))
        .expect_err("a digest naming nothing must not record");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::ProposalNotFound)
        ),
        "{error:?}"
    );

    assert_eq!(
        fixture.count("SELECT count(*) FROM evaluations"),
        0,
        "a refused Evaluation left a row"
    );
    assert_eq!(
        counts(&fixture),
        before,
        "a refused Evaluation left an Event"
    );
}

#[test]
fn an_unauthenticated_caller_receives_only_the_authentication_refusal() {
    let fixture = admitted_mission("evaluation-locked");
    let record = fixture.record();
    fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    fixture.accounts.lock().unwrap();

    // The request is ALSO malformed -- a digest naming no Proposal -- so if any other check ran
    // first this would report that instead, and the authentication boundary would be decorative.
    let error = evaluations
        .record(evaluation_draft(&fixture, &HEXLOWER.encode(&[0x5a_u8; 32])))
        .expect_err("a locked session must refuse");
    assert!(
        matches!(error, EvaluationError::Account(_)),
        "authentication did not come first: {error:?}"
    );
}

#[test]
fn recording_an_evaluation_leaves_run_receipt_worktree_process_and_network_counts_unchanged() {
    let fixture = admitted_mission("evaluation-inert");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let (runs_before, receipts_before, events_before) = counts(&fixture);

    evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let (runs_after, receipts_after, events_after) = counts(&fixture);
    assert_eq!(runs_before, runs_after, "an Evaluation opened a Run");
    assert_eq!(
        receipts_before, receipts_after,
        "an Evaluation recorded a Receipt"
    );
    // Exactly one Event: the audit record, and nothing else.
    assert_eq!(
        events_after,
        events_before + 1,
        "an Evaluation recorded more than its own Event"
    );
    let connection = fixture.store.connect();
    let kind: String = connection
        .query_row(
            "SELECT kind FROM events ORDER BY sequence DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "evaluation.recorded");
}

#[test]
fn a_recorded_evaluation_is_immutable_and_survives_startup_verification() {
    let fixture = admitted_mission("evaluation-startup");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let connection = fixture.store.connect();
    for statement in [
        "UPDATE evaluations SET rationale = 'rewritten'",
        "DELETE FROM evaluations",
        "UPDATE evaluation_findings SET summary = 'rewritten'",
        "DELETE FROM evaluation_findings",
    ] {
        assert!(
            connection.execute(statement, []).is_err(),
            "the store accepted `{statement}`"
        );
    }
    drop(connection);

    let report = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("startup verifies the Evaluation");
    assert_eq!(report.diagnostics.schema_version, 34);
    assert_eq!(
        evaluations.get(&evaluation.digest().value).unwrap(),
        evaluation
    );
}

/// gpt2's P2: the audit Event must bind the EXACT canonical bytes, not merely quote a digest that
/// the row it names would have answered for anyway. Each case below is a way the Event and the
/// record can disagree, and startup must refuse every one of them.
#[test]
fn the_audit_event_binds_the_exact_bytes_and_refuses_every_way_it_can_disagree() {
    let fixture = admitted_mission("evaluation-event-binding");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let restart = |fixture: &Fixture| {
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
    };
    assert!(restart(&fixture).is_ok(), "the untouched store must start");

    let original: String = fixture
        .store
        .connect()
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // The Event really does carry the bytes, not just their name.
    assert!(
        original.contains(&BASE64.encode(evaluation.canonical_bytes())),
        "the Event does not carry the canonical bytes"
    );

    let set_payload = |payload: &str| {
        fixture
            .store
            .connect()
            .execute(
                "UPDATE events SET payload_json = ?1 WHERE kind = 'evaluation.recorded'",
                [payload],
            )
            .unwrap();
    };

    // 1. Different bytes in the Event than in the row. What this actually measures is the Event's
    //    SELF-consistency -- its bytes no longer hash to the digest it names -- because both sides
    //    are digest-verified, so a byte disagreement cannot survive to reach the row comparison.
    //    Named for what it checks rather than for the check it looks like it exercises.
    let mut other = evaluation.canonical_bytes().to_vec();
    let last = other.len() - 1;
    other[last] ^= 0x01;
    let forged = original.replace(
        &BASE64.encode(evaluation.canonical_bytes()),
        &BASE64.encode(&other),
    );
    set_payload(&forged);
    assert!(restart(&fixture).is_err(), "changed bytes started");

    // 2. An extra field. A closed payload refuses it rather than ignoring it.
    let extra = original.replace("{\"evaluation\":{", "{\"evaluation\":{\"note\":\"extra\",");
    assert_ne!(extra, original, "the probe did not modify the payload");
    set_payload(&extra);
    assert!(restart(&fixture).is_err(), "an extra Event field started");

    // 3. A renamed field is the same defect from the other side.
    let renamed = original.replace("\"verdict\"", "\"verdict_\"");
    assert_ne!(renamed, original);
    set_payload(&renamed);
    assert!(restart(&fixture).is_err(), "a renamed Event field started");

    set_payload(&original);
    assert!(restart(&fixture).is_ok(), "the restored Event must start");

    // 4. A duplicate Event. One record, one Event -- two is not "extra evidence".
    fixture
        .store
        .connect()
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, payload_json, occurred_at_ms FROM events
             WHERE kind = 'evaluation.recorded'",
            [],
        )
        .unwrap();
    assert!(restart(&fixture).is_err(), "a duplicate Event started");

    // 5. A missing Event: the record exists and nothing witnesses it.
    fixture
        .store
        .connect()
        .execute("DELETE FROM events WHERE kind = 'evaluation.recorded'", [])
        .unwrap();
    assert!(restart(&fixture).is_err(), "a missing Event started");
}

/// An Event naming an Evaluation that is not stored is not a harmless leftover: it asserts a
/// judgment nothing recorded.
#[test]
fn an_event_naming_no_stored_evaluation_refuses_startup() {
    let fixture = admitted_mission("evaluation-orphan-event");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let connection = fixture.store.connect();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Same shape, a digest nothing stores.
    let orphan = payload.replace(&evaluation.digest().value, &HEXLOWER.encode(&[0x33_u8; 32]));
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'evaluation.recorded', ?2, 7)",
            rusqlite::params![fixture.workspace_id.to_string(), orphan],
        )
        .unwrap();
    drop(connection);

    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "an orphan Event started"
    );
}

/// Raw SQL, the real migration ladder, and the truthful forbidden case gpt2 recorded as passing:
/// bytes and row both naming Proposal A and evaluator A, where A authored A. This is the write the
/// trigger exists for, and the decoy-projection control cannot substitute for it, because nothing
/// about this row disagrees with anything.
#[test]
fn an_evaluator_who_authored_the_bound_proposal_is_refused_by_the_store() {
    let fixture = admitted_mission("evaluation-self-store");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let connection = fixture.store.connect();

    let insert = |resident: Uuid, configuration: Uuid, digest: &str| {
        connection.execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             ) VALUES (
                 ?1, 'sha256', 'beacon.evaluation.v1', ?2, ?3, 'proposal', ?4, ?4, 'sha256',
                 'beacon.proposal.v1',
                 ?5, ?6, ?7, 'r', 1, 'beacon.rubric.v1', ?8, 'uncertain', 0, NULL, 'why', 7
             )",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                digest,
                vec![0_u8; 32],
                proposal.digest().value,
                fixture.mission_id.to_string(),
                resident.to_string(),
                configuration.to_string(),
                HEXLOWER.encode(&[0x20_u8; 32]),
            ],
        )
    };

    // SQLite says WHICH constraint refused, so the trigger's own rule is assertable rather than
    // inferred from "an error happened". 1811 is SQLITE_CONSTRAINT_TRIGGER; 787 is
    // SQLITE_CONSTRAINT_FOREIGNKEY.
    let code = |result: rusqlite::Result<usize>| match result {
        Err(rusqlite::Error::SqliteFailure(error, _)) => error.extended_code,
        other => panic!("expected a constraint failure, got {other:?}"),
    };

    // The author, under the configuration they authored with.
    assert_eq!(
        code(insert(
            fixture.resident_id,
            fixture.configuration_id,
            &HEXLOWER.encode(&[0x01_u8; 32]),
        )),
        1811,
        "raw SQL did not hit the non-authorship trigger"
    );

    // gpt2's E1, and the correction it forced: the author under ANOTHER configuration. My C38
    // report claimed the Resident-versus-pair distinction was unobservable at this layer, because
    // the membership foreign key refuses the row either way. That was too strong -- the two
    // refusals are told apart by their CAUSE. With the Resident-only trigger this is the trigger's
    // refusal (1811); a pair-keyed trigger would let the row past and it would become the
    // membership foreign key's (787).
    assert_eq!(
        code(insert(
            fixture.resident_id,
            fixture.other_configuration_id,
            &HEXLOWER.encode(&[0x03_u8; 32]),
        )),
        1811,
        "the trigger compares the configuration as well as the Resident"
    );

    // The positive control. Without it the trigger and a blanket refusal are indistinguishable.
    let permitted = insert(
        fixture.other_resident_id,
        fixture.other_configuration_id,
        &HEXLOWER.encode(&[0x02_u8; 32]),
    );
    assert!(
        permitted.is_ok(),
        "the trigger refuses a non-author too: {permitted:?}"
    );
}

/// Two configurations of one Resident are two evaluators for configuration purposes and ONE author
/// for this one. Keying the refusal on the pair would let an author re-evaluate by reconfiguring.
#[test]
fn a_second_configuration_of_the_author_is_still_the_author() {
    let fixture = admitted_mission("evaluation-self-reconfigured");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        // NOT the configuration the Proposal was authored under.
        configuration_id: fixture.other_configuration_id,
    };
    let error = evaluations
        .record(draft)
        .expect_err("the author under another configuration must be refused");
    // The COMMAND's arm, not a store error. This is the layer where Resident-only is observable:
    // a pair-keyed comparison would let this through, and the membership foreign key would then
    // refuse it for a different reason, which this assertion tells apart.
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheProposal)
        ),
        "{error:?}"
    );

    // And the fact that makes the same distinction unobservable at the trigger:
    // `mission_admission_members` is UNIQUE per (Workspace, Mission, Resident), so the author is
    // admitted under exactly one configuration and no row can name them under another.
    assert_eq!(
        fixture.count(&format!(
            "SELECT count(*) FROM mission_admission_members WHERE resident_id = '{}'",
            fixture.resident_id
        )),
        1,
        "the author is admitted more than once, and the trigger's key would then matter"
    );
}

/// The Command refuses before writing, and says WHY -- a caller gets the relation that is wrong
/// rather than a database abort.
#[test]
fn the_command_refuses_a_self_evaluation_before_it_writes_anything() {
    let fixture = admitted_mission("evaluation-self-command");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let before = counts(&fixture);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        configuration_id: fixture.configuration_id,
    };
    let error = evaluations
        .record(draft)
        .expect_err("a self-evaluation must be refused");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheProposal)
        ),
        "{error:?}"
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM evaluations"),
        0,
        "a refused self-evaluation left a row"
    );
    assert_eq!(
        counts(&fixture),
        before,
        "a refused self-evaluation left an Event"
    );
}

/// The verified read and startup re-derive non-authorship from the canonical BYTES, so a row that
/// reached the store some other way -- an older binary, a manual repair, a future migration bug --
/// is refused on the way out as well as on the way in.
///
/// Reaching that layer takes work, and the work is the point. Every ordinary path is already
/// closed: the Command refuses, the trigger refuses a truthful projection, a decoy projection is
/// refused earlier as `ColumnDisagreesWithBytes`, and Proposals are immutable so the author cannot
/// be changed underneath a valid Evaluation. So the trigger is dropped, a TRUTHFUL self-evaluation
/// is written -- bytes and projection agreeing, digest recomputed over the spliced bytes -- and the
/// trigger is restored, leaving the schema signature intact and the store holding a row nothing
/// should have been able to write.
#[test]
fn a_self_evaluation_written_past_the_command_refuses_startup() {
    let fixture = admitted_mission("evaluation-self-startup");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    // ADR 0038's field table puts the evaluator Resident at 94: magic 21, subject tag 1, Proposal
    // digest 32, then the two length-framed envelope strings (8+6 and 8+18).
    const EVALUATOR_RESIDENT_OFFSET: usize = 94;
    let mut bytes = evaluation.canonical_bytes().to_vec();
    assert_eq!(
        &bytes[EVALUATOR_RESIDENT_OFFSET..EVALUATOR_RESIDENT_OFFSET + 16],
        fixture.other_resident_id.as_bytes(),
        "the published offset is not where the evaluator Resident lives"
    );
    bytes[EVALUATOR_RESIDENT_OFFSET..EVALUATOR_RESIDENT_OFFSET + 16]
        .copy_from_slice(fixture.resident_id.as_bytes());
    // The configuration follows it, and must be one the author was admitted under, or the
    // membership foreign key refuses the row before the re-derivation is reached.
    bytes[EVALUATOR_RESIDENT_OFFSET + 16..EVALUATOR_RESIDENT_OFFSET + 32]
        .copy_from_slice(fixture.configuration_id.as_bytes());
    let digest = HEXLOWER.encode(&sha256(&bytes));

    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER evaluations_no_self_evaluation;")
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id, ?3,
                    ?4, rubric_id, rubric_version, rubric_canonicalizer,
                    rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                    evaluated_at_ms
             FROM evaluations WHERE digest_value = ?5",
            rusqlite::params![
                digest,
                bytes,
                fixture.resident_id.to_string(),
                fixture.configuration_id.to_string(),
                evaluation.digest().value,
            ],
        )
        .unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER evaluations_no_self_evaluation
             BEFORE INSERT ON evaluations
             WHEN EXISTS (
                 SELECT 1 FROM proposals
                 WHERE workspace_id = NEW.workspace_id
                   AND digest_value = NEW.proposal_digest
                   AND requester_resident_id = NEW.evaluator_resident_id
             )
             BEGIN
                 SELECT RAISE(ABORT, 'An Evaluation may not name the author of the Proposal it evaluates');
             END;",
        )
        .unwrap();
    // Its findings, cited Runs and Event, so nothing else refuses it first.
    connection
        .execute(
            "INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE evaluation_digest = ?2",
            rusqlite::params![digest, evaluation.digest().value],
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event = event
        .replace(evaluation.digest().value.as_str(), &digest)
        .replace(
            &BASE64.encode(evaluation.canonical_bytes()),
            &BASE64.encode(&bytes),
        );
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, ?1, occurred_at_ms FROM events
             WHERE kind = 'evaluation.recorded' LIMIT 1",
            [event],
        )
        .unwrap();
    drop(connection);

    // The row is truthful: bytes and projection agree, and the digest is the hash of those bytes.
    // Only a re-derivation against the Proposal can see what is wrong with it.
    let error = evaluations
        .get(&digest)
        .expect_err("the verified read must refuse it");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheProposal)
        ),
        "refused for the wrong reason: {error:?}"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a self-evaluation"
    );
}

/// Membership is structural: an evaluator who is not an admitted member of the bound Proposal's
/// Mission under the exact configuration named has no parent row to point at.
#[test]
fn an_evaluator_not_admitted_to_the_bound_proposals_mission_is_unstorable() {
    let fixture = admitted_mission("evaluation-not-admitted");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.evaluator = Evaluator {
        resident_id: Uuid::new_v4(),
        configuration_id: Uuid::new_v4(),
    };
    assert!(
        evaluations.record(draft).is_err(),
        "an unadmitted evaluator recorded"
    );

    // And the same Resident under a configuration they were not admitted with.
    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.evaluator = Evaluator {
        resident_id: fixture.other_resident_id,
        configuration_id: Uuid::new_v4(),
    };
    assert!(
        evaluations.record(draft).is_err(),
        "an unadmitted configuration recorded"
    );
    assert_eq!(fixture.count("SELECT count(*) FROM evaluations"), 0);
}

/// A cited Run must have recorded a Receipt. `evaluation_cited_runs` foreign-keys to `receipts`
/// rather than to `runs`, so this is refused by the shape of the store rather than by a check the
/// Command could forget.
///
/// gpt2's E2: citing a fresh UUID proves nothing here, because it names neither a Run nor a
/// Receipt -- pointing the foreign key at `runs` instead left that version of this test green, so
/// it could not tell the ruled contract from the wrong schema it warns against. The Run below is
/// structurally real and simply has no Receipt, which is the only shape that distinguishes them.
#[test]
fn a_cited_run_without_a_receipt_is_refused() {
    let fixture = admitted_mission("evaluation-cited-run");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    // A real Run row, in this Workspace and Project, that recorded no Receipt.
    let run_id = Uuid::new_v4();
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'unconfirmed', 9, 5, NULL)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                run_id.to_string(),
                fixture.project_id.to_string(),
            ],
        )
        .expect("the Run inserts");
    assert_eq!(
        fixture.count(&format!("SELECT count(*) FROM runs WHERE id = '{run_id}'")),
        1,
        "the fixture Run was not created"
    );
    assert_eq!(
        fixture.count(&format!(
            "SELECT count(*) FROM receipts WHERE run_id = '{run_id}'"
        )),
        0,
        "the fixture Run has a Receipt"
    );
    drop(connection);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.cited_runs = vec![run_id];
    assert!(
        evaluations.record(draft).is_err(),
        "a real Run with no Receipt was cited"
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM evaluations"),
        0,
        "a refused citation left the Evaluation behind"
    );

    // And a Run that DID record a Receipt is citable, so the refusal above is the Receipt and not
    // citation in general.
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO receipts (
                 workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
             ) VALUES (?1, ?2, 1, 'verified', '{}', 6)",
            rusqlite::params![fixture.workspace_id.to_string(), run_id.to_string()],
        )
        .expect("the Receipt inserts");
    drop(connection);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.cited_runs = vec![run_id];
    let evaluation = evaluations
        .record(draft)
        .expect("a receipted Run is citable");
    assert_eq!(evaluation.payload().cited_runs, vec![run_id]);
}

#[test]
fn astra_migration_0033_copies_populated_parent_and_child_with_constraints() {
    let fixture = admitted_mission("evaluation-cited-run");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    // A real Run row, in this Workspace and Project, that recorded no Receipt.
    let run_id = Uuid::new_v4();
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO runs (
                 workspace_id, id, project_id, kind, repository_revision, charter_revision,
                 state, lease_expires_at_ms, started_at_ms, ended_at_ms
             ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'unconfirmed', 9, 5, NULL)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                run_id.to_string(),
                fixture.project_id.to_string(),
            ],
        )
        .expect("the Run inserts");
    assert_eq!(
        fixture.count(&format!("SELECT count(*) FROM runs WHERE id = '{run_id}'")),
        1,
        "the fixture Run was not created"
    );
    assert_eq!(
        fixture.count(&format!(
            "SELECT count(*) FROM receipts WHERE run_id = '{run_id}'"
        )),
        0,
        "the fixture Run has a Receipt"
    );
    drop(connection);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.cited_runs = vec![run_id];
    assert!(
        evaluations.record(draft).is_err(),
        "a real Run with no Receipt was cited"
    );
    assert_eq!(
        fixture.count("SELECT count(*) FROM evaluations"),
        0,
        "a refused citation left the Evaluation behind"
    );

    // And a Run that DID record a Receipt is citable, so the refusal above is the Receipt and not
    // citation in general.
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO receipts (
                 workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
             ) VALUES (?1, ?2, 1, 'verified', '{}', 6)",
            rusqlite::params![fixture.workspace_id.to_string(), run_id.to_string()],
        )
        .expect("the Receipt inserts");
    drop(connection);

    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.cited_runs = vec![run_id];
    let evaluation = evaluations
        .record(draft)
        .expect("a receipted Run is citable");
    assert_eq!(evaluation.payload().cited_runs, vec![run_id]);
    // Structural migration witness: this existing fixture deliberately plants its Run/Receipt.
    // Its owner-invalid '{}' JSON is NOT represented as a startup-valid Receipt.
    let connection = fixture.store.connect();
    let second = Uuid::new_v4().to_string();
    connection.execute("INSERT INTO runs SELECT workspace_id,?1,project_id,kind,repository_revision,charter_revision,state,lease_expires_at_ms,started_at_ms,ended_at_ms FROM runs WHERE id=?2", rusqlite::params![second,run_id.to_string()]).unwrap();
    connection.execute("INSERT INTO receipts SELECT workspace_id,?1,2,'completed',?2,23 FROM receipts WHERE run_id=?3", rusqlite::params![second,"{ \"migrationSpecimen\": \"retained bytes\" }\n",run_id.to_string()]).unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_cited_runs VALUES (?1,?2,?3)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                evaluation.digest().value,
                second
            ],
        )
        .unwrap();

    let row_snapshot = |c: &Connection, sql: &str| {
        let mut statement = c.prepare(sql).unwrap();
        let n = statement.column_count();
        statement
            .query_map([], |r| {
                (0..n)
                    .map(|i| r.get::<_, rusqlite::types::Value>(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let receipt_query = "SELECT workspace_id,run_id,version,outcome,receipt_json,recorded_at_ms FROM receipts ORDER BY workspace_id,run_id";
    let child_query = "SELECT workspace_id,evaluation_digest,run_id FROM evaluation_cited_runs ORDER BY workspace_id,evaluation_digest,run_id";
    let objects_query = "SELECT type,name,tbl_name,sql FROM sqlite_master WHERE type IN ('trigger','index') ORDER BY type,name";
    let parent_before = row_snapshot(&connection, receipt_query);
    let child_before = row_snapshot(&connection, child_query);
    let objects_before = row_snapshot(&connection, objects_query);
    assert_eq!(parent_before.len(), 2, "R46.MIGRATION.TWO_PARENT_ROWS");
    assert_eq!(child_before.len(), 2, "R46.MIGRATION.TWO_CHILD_ROWS");
    let migration = include_str!("../migrations/0033_receipt_pre_image_unreadable.sql");
    let old = include_str!("../migrations/0022_candidates.sql");
    let old_table = &old[old.find("CREATE TABLE receipts (").unwrap()..];
    let old_create = &old_table[..old_table.find(") STRICT;").unwrap() + 9];
    let parent_triggers = &migration[migration.find("CREATE TRIGGER receipts_no_update").unwrap()
        ..migration.find("UPDATE schema_meta").unwrap()];
    let child_table = &migration[migration
        .find("CREATE TABLE evaluation_cited_runs (")
        .unwrap()..];
    let child_create = &child_table[..child_table.find(") STRICT;").unwrap() + 9];
    let child_triggers = &migration[migration
        .find("CREATE TRIGGER evaluation_cited_runs_no_update")
        .unwrap()
        ..migration.find("-- Recreated byte-exactly").unwrap()];
    // Reconstruct the exact v32 definitions around the populated relation, in the same FK-on shape.
    connection.execute_batch("BEGIN; DROP TRIGGER evaluation_cited_runs_no_update; DROP TRIGGER evaluation_cited_runs_no_delete; DROP TRIGGER receipts_no_update; DROP TRIGGER receipts_no_delete; ALTER TABLE evaluation_cited_runs RENAME TO child_fixture_33; ALTER TABLE receipts RENAME TO parent_fixture_33;").unwrap();
    connection.execute_batch(old_create).unwrap();
    connection
        .execute_batch("INSERT INTO receipts SELECT * FROM parent_fixture_33;")
        .unwrap();
    connection.execute_batch(child_create).unwrap();
    connection.execute_batch("INSERT INTO evaluation_cited_runs SELECT * FROM child_fixture_33; DROP TABLE child_fixture_33; DROP TABLE parent_fixture_33;").unwrap();
    connection.execute_batch(parent_triggers).unwrap();
    connection.execute_batch(child_triggers).unwrap();
    connection
        .execute_batch("UPDATE schema_meta SET version=32 WHERE singleton=1; COMMIT;")
        .unwrap();
    assert!(
        row_snapshot(&connection, "PRAGMA foreign_key_check").is_empty(),
        "R46.MIGRATION.FK_BEFORE"
    );
    assert_eq!(
        row_snapshot(&connection, receipt_query),
        parent_before,
        "R46.MIGRATION.FIXTURE_PARENT_BYTES"
    );
    assert_eq!(
        row_snapshot(&connection, child_query),
        child_before,
        "R46.MIGRATION.FIXTURE_CHILD_BYTES"
    );
    assert_eq!(
        row_snapshot(&connection, objects_query),
        objects_before,
        "R46.MIGRATION.FIXTURE_OBJECT_NAMES"
    );
    assert_eq!(
        connection
            .query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1,
        "R46.MIGRATION.FOREIGN_KEYS_ON"
    );
    connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
    connection
        .execute_batch(migration)
        .expect("R46.MIGRATION.0033_EXECUTES");
    connection.execute_batch("COMMIT;").unwrap();
    assert_eq!(
        row_snapshot(&connection, receipt_query),
        parent_before,
        "R46.MIGRATION.ALL_SIX_PARENT_COLUMNS_EXACT"
    );
    assert_eq!(
        row_snapshot(&connection, child_query),
        child_before,
        "R46.MIGRATION.ALL_THREE_CHILD_COLUMNS_EXACT"
    );
    assert_eq!(
        row_snapshot(&connection, objects_query),
        objects_before,
        "R46.MIGRATION.ALL_INDEX_TRIGGER_NAMES_AND_SQL_EXACT"
    );
    assert!(
        row_snapshot(&connection, "PRAGMA foreign_key_check").is_empty(),
        "R46.MIGRATION.FK_AFTER"
    );
    let receipts_ddl_before: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'receipts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let membership_start = receipts_ddl_before
        .find(" CHECK (outcome IN (")
        .expect("0033 still has the SQL membership vocabulary");
    let receipt_json_start = receipts_ddl_before
        .find("\n    receipt_json TEXT")
        .expect("receipt_json follows the outcome membership CHECK");
    let receipts_ddl_without_membership = format!(
        "{},{}",
        &receipts_ddl_before[..membership_start],
        &receipts_ddl_before[receipt_json_start..]
    );
    let migration_0034 = include_str!("../migrations/0034_receipt_outcome_typed_owner.sql");
    connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
    connection
        .execute_batch(migration_0034)
        .expect("R48.MIGRATION.0034_EXECUTES");
    connection.execute_batch("COMMIT;").unwrap();
    let receipts_ddl_after: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'receipts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        receipts_ddl_after, receipts_ddl_without_membership,
        "R48.MIGRATION.ONLY_OUTCOME_MEMBERSHIP_CHECK_MOVED"
    );
    assert_eq!(
        row_snapshot(&connection, receipt_query),
        parent_before,
        "R48.MIGRATION.ALL_SIX_PARENT_COLUMNS_EXACT"
    );
    assert_eq!(
        row_snapshot(&connection, child_query),
        child_before,
        "R48.MIGRATION.ALL_THREE_CHILD_COLUMNS_EXACT"
    );
    assert_eq!(
        row_snapshot(&connection, objects_query),
        objects_before,
        "R48.MIGRATION.ALL_INDEX_TRIGGER_NAMES_AND_SQL_EXACT"
    );
    assert!(
        row_snapshot(&connection, "PRAGMA foreign_key_check").is_empty(),
        "R48.MIGRATION.FK_AFTER"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT version FROM schema_meta WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        34,
        "R48.MIGRATION.VERSION_34"
    );
    for (sql, required) in [
        (
            "UPDATE receipts SET recorded_at_ms=recorded_at_ms",
            "Receipts are immutable",
        ),
        ("DELETE FROM receipts", "Receipts are immutable"),
        (
            "UPDATE evaluation_cited_runs SET run_id=run_id",
            "Evaluation cited Runs are immutable",
        ),
        (
            "DELETE FROM evaluation_cited_runs",
            "Evaluation cited Runs are immutable",
        ),
    ] {
        let error = connection.execute(sql, []).unwrap_err().to_string();
        assert!(
            error.contains(required),
            "R46.MIGRATION.TRIGGER_NAME {} {}",
            sql,
            error
        );
    }
    println!(
        "R46.MIGRATION parent_rows=2 columns=6 child_rows=2 columns=3 schema_objects={}",
        objects_before.len()
    );
    println!(
        "R46.MIGRATION_OBJECTS {:?}",
        row_snapshot(
            &connection,
            "SELECT type,name FROM sqlite_master WHERE tbl_name IN ('receipts','evaluation_cited_runs') AND type IN ('index','trigger') ORDER BY type,name"
        )
    );
    println!(
        "R48.MIGRATION parent_rows=2 columns=6 child_rows=2 columns=3 schema_objects={}",
        objects_before.len()
    );
}

#[test]
fn r48_receipt_outcomes_are_not_a_sql_membership_vocabulary() {
    let fixture = admitted_mission("receipt-outcome-owner");
    let connection = fixture.store.connect();
    let receipts_sql: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'receipts'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    assert!(
        !receipts_sql.contains("CHECK (outcome IN ("),
        "R48.OUTCOME_VOCABULARY_TYPED_NOT_SQL: {receipts_sql}"
    );
}

/// gpt2's E1: the two-layer proof needs BOTH controls committed, and only the trigger half was.
///
/// This is the other half — the reachable decoy. The canonical bytes name one Proposal; the
/// relational column names another, while the digest still genuinely hashes those bytes. Only
/// `columns_match`'s Proposal-digest comparison can see that, which is the point: my first attempt
/// at this test inserted a fabricated digest, so `DigestMismatch` fired first and the comparison
/// this test is named for was never reached — gpt2's mutant survived it.
#[test]
fn a_projection_that_names_another_proposal_than_the_bytes_is_refused_at_the_read() {
    let fixture = admitted_mission("evaluation-decoy-projection");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let mut second = fixture.record();
    second.summary = "A second Proposal, so the decoy has somewhere to point.".to_owned();
    let decoy = fixture.proposals.submit(&second.bytes()).unwrap();
    assert_ne!(proposal.digest().value, decoy.digest().value);

    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    // Repoint the column only. The bytes, and therefore the digest, do not move -- so every other
    // check still passes and exactly one thing is wrong with this row.
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER evaluations_no_update;")
        .unwrap();
    connection
        .execute(
            // Both projections move together: the store's own CHECK requires `subject_digest`
            // to be the populated arm's digest, so a decoy that moved only one of them would be
            // refused by the schema rather than by the reader -- and this test is about the READER
            // catching a projection that disagrees with the BYTES.
            "UPDATE evaluations SET proposal_digest = ?1, subject_digest = ?1
             WHERE digest_value = ?2",
            rusqlite::params![decoy.digest().value, evaluation.digest().value],
        )
        .expect("the store admits the decoy: SQL cannot read the bytes");
    connection
        .execute_batch(
            "CREATE TRIGGER evaluations_no_update
             BEFORE UPDATE ON evaluations
             BEGIN
                 SELECT RAISE(ABORT, 'Evaluations are immutable');
             END;",
        )
        .unwrap();
    drop(connection);

    let error = evaluations
        .get(&evaluation.digest().value)
        .expect_err("the verified read must refuse the decoy");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::ColumnDisagreesWithBytes)
        ),
        "refused for the wrong reason, so this test does not measure the projection: {error:?}"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a decoy projection"
    );
}

/// gpt2's E3: the total-bound test proved the codec's bound and never the store's, so lowering the
/// SQL ceiling to 32,768 left it green. This exercises the migrated column itself, at both sides of
/// the exact number the encoder owns.
#[test]
fn the_migrated_column_repeats_the_codecs_owned_byte_bound() {
    const MAX_CANONICAL_BYTES: usize = 65_536;
    let fixture = admitted_mission("evaluation-store-bound");
    let connection = fixture.store.connect();

    let attempt = |length: usize, digest: &str| {
        connection.execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             ) VALUES (
                 ?1, 'sha256', 'beacon.evaluation.v1', ?2, ?3, 'proposal', ?4, ?4, 'sha256',
                 'beacon.proposal.v1',
                 ?5, ?6, ?7, 'r', 1, 'beacon.rubric.v1', ?8, 'uncertain', 0, NULL, 'why', 7
             )",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                digest,
                vec![0_u8; length],
                HEXLOWER.encode(&[0x60_u8; 32]),
                fixture.mission_id.to_string(),
                fixture.other_resident_id.to_string(),
                fixture.other_configuration_id.to_string(),
                HEXLOWER.encode(&[0x20_u8; 32]),
            ],
        )
    };

    // One byte over the codec's bound must be refused by the COLUMN, so the two numbers are the
    // same number. The foreign key to `proposals` is not satisfied here, so a row that passes the
    // length CHECK still fails -- the two failures are told apart by their SQLite error code.
    let over = attempt(MAX_CANONICAL_BYTES + 1, &HEXLOWER.encode(&[0x61_u8; 32]));
    let at_bound = attempt(MAX_CANONICAL_BYTES, &HEXLOWER.encode(&[0x62_u8; 32]));
    let code = |result: &rusqlite::Result<usize>| match result {
        Err(rusqlite::Error::SqliteFailure(error, _)) => error.extended_code,
        other => panic!("expected a constraint failure, got {other:?}"),
    };
    // 275 is SQLITE_CONSTRAINT_CHECK; 787 is SQLITE_CONSTRAINT_FOREIGNKEY.
    assert_eq!(
        code(&over),
        275,
        "the column accepted one byte over the bound"
    );
    assert_eq!(
        code(&at_bound),
        787,
        "the column refused a blob AT the bound, so its limit is not the codec's"
    );
}

/// An evaluator changes their mind. Both records stay readable, because an Evaluation is a true
/// account of what somebody judged about exact bytes and no later record makes that untrue -- and
/// the earlier one now reports, as a fact rather than a verdict, that it was superseded.
#[test]
fn a_second_evaluation_supersedes_the_first_and_both_stay_readable() {
    let fixture = admitted_mission("evaluation-supersedes");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();
    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.verdict = Verdict::Meets;
    draft.rationale = "On a second reading the criteria are met.".to_owned();
    draft.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    let second = evaluations.record(draft).unwrap();

    let first = evaluations.get(&first.digest().value).unwrap();
    assert_eq!(
        first
            .force()
            .superseded_by()
            .map(|digest| digest.value.as_str()),
        Some(second.digest().value.as_str()),
        "the superseded Evaluation does not name its successor"
    );
    assert!(
        second.force().superseded_by().is_none(),
        "the current tail claims to be superseded"
    );
    // Both readable, and the first still says exactly what it said.
    assert_eq!(first.payload().verdict, Verdict::Uncertain);
    assert_eq!(second.payload().verdict, Verdict::Meets);
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_ok(),
        "a superseded Evaluation must not fail startup"
    );
}

/// Superseding anything but the current tail would fork the chain, and a fork has no last link, so
/// "the effective Evaluation" would stop having an answer. Each wrong digest gets its own refusal,
/// because they lead a caller to different corrections.
#[test]
fn superseding_any_link_but_the_current_tail_is_refused() {
    let fixture = admitted_mission("evaluation-tail");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let mut second_record = fixture.record();
    second_record.summary = "A second Proposal, with its own chain.".to_owned();
    let other_proposal = fixture.proposals.submit(&second_record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();
    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    let second = evaluations.record(draft).unwrap();
    let elsewhere = evaluations
        .record(evaluation_draft(&fixture, &other_proposal.digest().value))
        .unwrap();

    let refuse = |supersedes: EvaluationSupersedes| {
        let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
        draft.supersedes = supersedes;
        evaluations.record(draft).expect_err("must be refused")
    };

    // An already-superseded link: this is the fork.
    assert!(
        matches!(
            refuse(EvaluationSupersedes::Evaluation(
                first.digest().value.clone()
            )),
            EvaluationError::Inconsistent(InconsistentEvaluation::NotTheCurrentTail)
        ),
        "superseding a superseded link was not refused as a non-tail"
    );
    // A digest naming nothing.
    assert!(
        matches!(
            refuse(EvaluationSupersedes::Evaluation(
                HEXLOWER.encode(&[0x44_u8; 32])
            )),
            EvaluationError::Inconsistent(InconsistentEvaluation::SupersedesNotRecorded)
        ),
        "a digest naming nothing was not told apart from a non-tail"
    );
    // A real Evaluation of ANOTHER Proposal: a different mistake again.
    assert!(
        matches!(
            refuse(EvaluationSupersedes::Evaluation(
                elsewhere.digest().value.clone()
            )),
            EvaluationError::Inconsistent(InconsistentEvaluation::SupersedesOtherSubject)
        ),
        "another Proposal's Evaluation was not told apart"
    );
    // And declaring NO predecessor when a chain already exists is the same fork from the other end.
    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.supersedes = EvaluationSupersedes::None;
    assert!(
        matches!(
            evaluations.record(draft).expect_err("must be refused"),
            EvaluationError::Inconsistent(InconsistentEvaluation::NotTheCurrentTail)
        ),
        "a second first-link was admitted"
    );
    assert_eq!(
        second.payload().supersedes,
        EvaluationSupersedes::Evaluation(first.digest().value.clone())
    );
}

/// Time along the chain is non-decreasing, and EQUAL times are allowed: two records can land in the
/// same millisecond, and refusing that would invent a precision the clock does not have.
///
/// gpt2's E4: the first version of this test moved only the projected `evaluated_at_ms`, so startup
/// failed at column-versus-byte reconciliation and never reached the chain-time comparison —
/// disabling that comparison left the test green. The successor below is built self-consistently
/// instead: its bytes carry the earlier time AND declare the supersession, its digest is recomputed
/// over those bytes, its column matches, and its Event carries them. The only wrong fact in the
/// store is that the chain runs backwards.
#[test]
fn an_evaluated_time_earlier_than_the_link_it_supersedes_is_refused() {
    let fixture = admitted_mission("evaluation-chain-time");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    // The record ends with `supersedes` (one byte for `none`) and `evaluated_at_ms` (eight): nine
    // bytes, per ADR 0038's field table. Replace the tail with `0x01` + the predecessor digest + an
    // EARLIER time, and everything before it is byte-identical to a record the Command produced.
    let tail = first.canonical_bytes().len() - 9;
    assert_eq!(
        first.canonical_bytes()[tail],
        0x00,
        "the field table does not put `supersedes = none` nine bytes from the end"
    );
    let earlier = first.payload().evaluated_at_ms - 1_000;
    let mut bytes = first.canonical_bytes()[..tail].to_vec();
    bytes.push(0x01);
    bytes.extend_from_slice(
        &HEXLOWER
            .decode(first.digest().value.as_bytes())
            .expect("the predecessor digest is canonical hex"),
    );
    bytes.extend_from_slice(&earlier.to_be_bytes());
    let digest = HEXLOWER.encode(&sha256(&bytes));

    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id,
                    evaluator_resident_id, evaluator_configuration_id, rubric_id, rubric_version,
                    rubric_canonicalizer, rubric_sha256, verdict, conflicts_declared,
                    conflicts_text, rationale, ?3
             FROM evaluations WHERE digest_value = ?4",
            rusqlite::params![
                digest,
                bytes,
                i64::try_from(earlier).unwrap(),
                first.digest().value
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE evaluation_digest = ?2",
            rusqlite::params![digest, first.digest().value],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_supersessions (
                 workspace_id, subject_digest, evaluation_digest, supersedes_evaluation_digest
             ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                proposal.digest().value,
                digest,
                first.digest().value,
            ],
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event = event
        .replace(first.digest().value.as_str(), &digest)
        .replace(
            &BASE64.encode(first.canonical_bytes()),
            &BASE64.encode(&bytes),
        )
        .replace(
            &format!("\"evaluated_at_ms\":{}", first.payload().evaluated_at_ms),
            &format!("\"evaluated_at_ms\":{earlier}"),
        );
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'evaluation.recorded', ?2, ?3)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                event,
                i64::try_from(earlier).unwrap()
            ],
        )
        .unwrap();
    drop(connection);

    // Every other check passes: the read succeeds, so the only thing left is the chain.
    assert!(
        evaluations.get(&digest).is_ok(),
        "the successor row is not self-consistent, so this test measures something else"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a chain whose times decrease"
    );
}

/// The positive control for the same comparison: two links at the SAME millisecond are accepted,
/// because refusing equal times would invent a precision the clock does not have. Without it, `<`
/// and `<=` are indistinguishable.
///
/// Built exactly as the decreasing case is, with `equal` in place of `earlier` and no trigger
/// surgery, so the two tests differ in one value and nothing else.
#[test]
fn two_links_recorded_in_the_same_millisecond_are_accepted() {
    let fixture = admitted_mission("evaluation-chain-equal-time");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    let equal = first.payload().evaluated_at_ms;
    let tail = first.canonical_bytes().len() - 9;
    let mut bytes = first.canonical_bytes()[..tail].to_vec();
    bytes.push(0x01);
    bytes.extend_from_slice(&HEXLOWER.decode(first.digest().value.as_bytes()).unwrap());
    bytes.extend_from_slice(&equal.to_be_bytes());
    let digest = HEXLOWER.encode(&sha256(&bytes));

    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id,
                    evaluator_resident_id, evaluator_configuration_id, rubric_id, rubric_version,
                    rubric_canonicalizer, rubric_sha256, verdict, conflicts_declared,
                    conflicts_text, rationale, evaluated_at_ms
             FROM evaluations WHERE digest_value = ?3",
            rusqlite::params![digest, bytes, first.digest().value],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE evaluation_digest = ?2",
            rusqlite::params![digest, first.digest().value],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_supersessions (
                 workspace_id, subject_digest, evaluation_digest, supersedes_evaluation_digest
             ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                proposal.digest().value,
                digest,
                first.digest().value,
            ],
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event = event
        .replace(first.digest().value.as_str(), &digest)
        .replace(
            &BASE64.encode(first.canonical_bytes()),
            &BASE64.encode(&bytes),
        );
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             VALUES (?1, 'evaluation.recorded', ?2, ?3)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                event,
                i64::try_from(equal).unwrap()
            ],
        )
        .unwrap();
    drop(connection);

    let successor = evaluations.get(&digest).expect("the equal-time link reads");
    assert_eq!(successor.payload().evaluated_at_ms, equal);
    assert_eq!(
        successor.payload().evaluated_at_ms,
        first.payload().evaluated_at_ms,
        "the two links are not at the same millisecond, so this proves nothing"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_ok(),
        "startup refused two links recorded in the same millisecond"
    );
}

/// Moving a bound revision puts the Evaluation out of force NAMING that revision, and only that
/// one. The Evaluation stays readable and startup still succeeds: drift is a fact about the world
/// having moved, not a defect in the record.
#[test]
fn moving_a_bound_revision_puts_the_evaluation_out_of_force_naming_that_revision_and_only_that_one()
{
    let fixture = admitted_mission("evaluation-drift");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();
    assert!(
        evaluation.force().drift().is_empty(),
        "a fresh Evaluation already reports drift: {:?}",
        evaluation.force().drift()
    );

    ProjectService::new(Arc::clone(&fixture.accounts), fixture.workspace_id)
        .revise_charter(
            fixture.project_id,
            1,
            ProjectCharterDraft {
                purpose: "Prove the local agent office, revised.".to_owned(),
                desired_outcomes: vec!["Complete one attributable Mission".to_owned()],
                constraints: vec!["Keep authority explicit".to_owned()],
                prohibitions: vec!["Do not deploy".to_owned()],
                autonomy_profile: AutonomyProfile::Supervised,
            },
        )
        .unwrap();

    let drifted = evaluations.get(&evaluation.digest().value).unwrap();
    assert_eq!(
        drifted.force().drift(),
        [UnmetPrecondition::CharterRevisionMoved],
        "the Charter move named more than the Charter"
    );
    assert!(
        drifted.force().superseded_by().is_none(),
        "drift was reported as supersession"
    );
    // Still readable, still the same bytes, and startup still succeeds.
    assert_eq!(drifted.canonical_bytes(), evaluation.canonical_bytes());
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_ok(),
        "a drifted Evaluation failed startup"
    );
}

/// A chain with two heads has no last link, so "the effective Evaluation" stops having an answer.
/// The relation's own constraints already forbid a predecessor cited twice and an Evaluation citing
/// two predecessors; what they cannot forbid is TOPOLOGY -- a second, disconnected chain -- and that
/// is what the walk's length comparison refuses.
///
/// A cycle is the same branch from the other side: every Evaluation cited, so no head exists. It is
/// unreachable through any committed path, because a relation row that disagrees with an
/// Evaluation's `supersedes` byte tag is refused at the verified read before the walk is reached.
#[test]
fn an_evaluation_chain_that_forks_refuses_startup() {
    let fixture = admitted_mission("evaluation-fork");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();
    let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
    draft.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    evaluations.record(draft).unwrap();

    // A third Evaluation of the same Proposal with NO relation row: two heads, one link, three
    // records. Its bytes say `supersedes = none`, so the relation and the tag still agree.
    //
    // ADR 0038's field table puts the rubric digest at 180: magic 21, subject 1, Proposal digest
    // 32, two envelope strings (14 + 26), two UUIDs (32), rubric id (8+18), version 4, rubric
    // canonicalizer (8+16).
    const RUBRIC_DIGEST_OFFSET: usize = 180;
    let mut bytes = first.canonical_bytes().to_vec();
    assert_eq!(
        HEXLOWER.encode(&bytes[RUBRIC_DIGEST_OFFSET..RUBRIC_DIGEST_OFFSET + 32]),
        first.payload().rubric.sha256,
        "the published offset is not where the rubric digest lives"
    );
    bytes[RUBRIC_DIGEST_OFFSET] ^= 0x01;
    let digest = HEXLOWER.encode(&sha256(&bytes));
    let rubric = HEXLOWER.encode(&bytes[RUBRIC_DIGEST_OFFSET..RUBRIC_DIGEST_OFFSET + 32]);

    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id,
                    evaluator_resident_id, evaluator_configuration_id, rubric_id, rubric_version,
                    rubric_canonicalizer, ?3, verdict, conflicts_declared, conflicts_text,
                    rationale, evaluated_at_ms
             FROM evaluations WHERE digest_value = ?4",
            rusqlite::params![digest, bytes, rubric, first.digest().value],
        )
        .expect("the store admits a disconnected Evaluation");
    connection
        .execute(
            "INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE evaluation_digest = ?2",
            rusqlite::params![digest, first.digest().value],
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'
             ORDER BY sequence LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event = event
        .replace(first.digest().value.as_str(), &digest)
        .replace(
            &BASE64.encode(first.canonical_bytes()),
            &BASE64.encode(&bytes),
        );
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, ?1, occurred_at_ms FROM events
             WHERE kind = 'evaluation.recorded' ORDER BY sequence LIMIT 1",
            [event],
        )
        .unwrap();
    drop(connection);

    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a forked chain"
    );
}

/// Nothing in this module answers "is this Evaluation acceptable" with a bit. Supersession and
/// drift are two separate facts and stay two, because collapsing them would answer the reader's
/// question for them -- and because a boolean is exactly what a later dispatch would be tempted to
/// branch on, which is `BLUEPRINT:445-447` in a field.
#[test]
fn no_accessor_reports_force_or_acceptability_as_a_boolean() {
    let source = include_str!("../src/evaluation.rs");
    let offenders: Vec<&str> = source
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub fn ") || line.starts_with("pub const fn "))
        .filter(|line| line.contains("-> bool"))
        .collect();
    assert!(
        offenders.is_empty(),
        "a public accessor answers with a boolean: {offenders:?}"
    );

    // gpt2's E5: scanning accessors is not the contract. ADR 0038 says no boolean ANYWHERE on the
    // type, and adding a private `current: bool` field left the accessor scan green. The struct
    // body itself is read now, so a field is as visible as a method.
    let start = source
        .find("pub struct EvaluationForce {")
        .expect("EvaluationForce is declared");
    let body = &source[start..start + source[start..].find("\n}").expect("the struct closes")];
    assert!(
        !body.contains("bool"),
        "EvaluationForce grew a boolean field: {body}"
    );
    assert!(
        !source.contains("fn is_in_force")
            && !source.contains("fn is_acceptable")
            && !source.contains("fn is_current"),
        "an acceptability predicate was added"
    );
}

/// gpt2's S1: a malformed digest and a well-formed one naming nothing are different mistakes, and
/// the codec has had a distinct arm for the first all along. Querying the store with an unvalidated
/// string reported both as "not recorded".
#[test]
fn a_malformed_supersedes_digest_is_told_apart_from_one_that_names_nothing() {
    let fixture = admitted_mission("evaluation-supersedes-malformed");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let refuse = |digest: &str| {
        let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
        draft.supersedes = EvaluationSupersedes::Evaluation(digest.to_owned());
        evaluations.record(draft).expect_err("must be refused")
    };

    assert!(
        matches!(
            refuse("not-a-digest"),
            EvaluationError::Malformed(MalformedEvaluation::SupersedesDigest)
        ),
        "a malformed digest was reported as merely absent"
    );
    // Uppercase is malformed too: HEXLOWER accepts only the canonical spelling, so two spellings of
    // one digest can never both encode. `0xab` rather than `0x44` because the first probe I wrote
    // encoded to digits only, so `to_uppercase` was a no-op and the assertion measured nothing.
    let shouted = HEXLOWER.encode(&[0xab_u8; 32]).to_uppercase();
    assert!(
        shouted
            .chars()
            .any(|character| character.is_ascii_uppercase()),
        "the probe is not actually uppercase"
    );
    assert!(
        matches!(
            refuse(&shouted),
            EvaluationError::Malformed(MalformedEvaluation::SupersedesDigest)
        ),
        "an uppercase digest was not refused by the codec's rule"
    );
    // And the well-formed digest that names nothing keeps its own, different arm.
    assert!(
        matches!(
            refuse(&HEXLOWER.encode(&[0x44_u8; 32])),
            EvaluationError::Inconsistent(InconsistentEvaluation::SupersedesNotRecorded)
        ),
        "a well-formed absent digest lost its own refusal"
    );
}

/// Builds a store holding two Evaluations of one Proposal whose bytes BOTH say
/// `supersedes = none`, joined by a supersession relation that is structurally valid in every way
/// the relation's own constraints can see. Both are self-consistent and both are witnessed by their
/// own exact-byte Event, so the only wrong fact in the store is the disagreement itself.
///
/// The Event matters: gpt2's final review found the startup assertion passing on its absence, so
/// removing the relation check left it green and the startup half proved nothing.
fn store_with_a_relation_the_bytes_do_not_declare(
    label: &str,
) -> (Fixture, EvaluationService, String) {
    let fixture = admitted_mission(label);
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();

    // A second Evaluation of the same Proposal whose bytes also say `none`: splice the rubric
    // digest so it is a different record.
    const RUBRIC_DIGEST_OFFSET: usize = 180;
    let mut bytes = first.canonical_bytes().to_vec();
    bytes[RUBRIC_DIGEST_OFFSET] ^= 0x01;
    let digest = HEXLOWER.encode(&sha256(&bytes));
    let rubric = HEXLOWER.encode(&bytes[RUBRIC_DIGEST_OFFSET..RUBRIC_DIGEST_OFFSET + 32]);
    let connection = fixture.store.connect();
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id,
                    evaluator_resident_id, evaluator_configuration_id, rubric_id, rubric_version,
                    rubric_canonicalizer, ?3, verdict, conflicts_declared, conflicts_text,
                    rationale, evaluated_at_ms
             FROM evaluations WHERE digest_value = ?4",
            rusqlite::params![digest, bytes, rubric, first.digest().value],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
             SELECT workspace_id, ?1, ordinal, severity, summary FROM evaluation_findings
             WHERE evaluation_digest = ?2",
            rusqlite::params![digest, first.digest().value],
        )
        .unwrap();
    let event: String = connection
        .query_row(
            "SELECT payload_json FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let event = event
        .replace(first.digest().value.as_str(), &digest)
        .replace(
            &BASE64.encode(first.canonical_bytes()),
            &BASE64.encode(&bytes),
        );
    connection
        .execute(
            "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
             SELECT workspace_id, kind, ?1, occurred_at_ms FROM events
             WHERE kind = 'evaluation.recorded' LIMIT 1",
            [event],
        )
        .unwrap();
    // The relation. With it present the chain is one path of two links with equal times, so every
    // other startup rule is satisfied and only the relation-versus-bytes comparison can refuse.
    connection
        .execute(
            "INSERT INTO evaluation_supersessions (
                 workspace_id, subject_digest, evaluation_digest, supersedes_evaluation_digest
             ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                proposal.digest().value,
                digest,
                first.digest().value,
            ],
        )
        .expect("the relation's constraints admit this row");
    drop(connection);

    (fixture, evaluations, digest)
}

/// gpt2's E2, read half: the verified read refuses a relation the bytes do not declare, as exactly
/// that and not as anything else.
#[test]
fn a_relation_the_bytes_do_not_declare_is_refused_at_the_read() {
    let (_fixture, evaluations, digest) =
        store_with_a_relation_the_bytes_do_not_declare("evaluation-relation-disagrees-read");

    let error = evaluations
        .get(&digest)
        .expect_err("a relation the bytes do not declare must be refused");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::SupersessionDisagreesWithBytes)
        ),
        "refused for the wrong reason: {error:?}"
    );
}

/// gpt2's E2, startup half — a SEPARATE test on purpose.
///
/// When the read assertion and the startup assertion shared one test, removing the production check
/// made the read assertion fail first and the startup assertion was never evaluated, so it could
/// not be told from a store that refuses for some other reason. Standing alone, this one goes red
/// on its own when the check is removed, which is what "load-bearing" has to mean.
#[test]
fn a_relation_the_bytes_do_not_declare_refuses_startup() {
    let (fixture, _evaluations, _digest) =
        store_with_a_relation_the_bytes_do_not_declare("evaluation-relation-disagrees-startup");

    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a relation the bytes do not declare"
    );
}

/// gpt2's E3: both composite foreign keys are scoped by `(workspace_id, proposal_digest)`, and
/// neither was proved. Each end is exercised on its own, so a weakened scope on either side is
/// visible rather than masked by the other.
#[test]
fn each_composite_foreign_key_independently_refuses_a_cross_proposal_relation() {
    let fixture = admitted_mission("evaluation-relation-scope");
    let record = fixture.record();
    let here = fixture.proposals.submit(&record.bytes()).unwrap();
    let mut other_record = fixture.record();
    other_record.summary = "A second Proposal, with its own Evaluation.".to_owned();
    let elsewhere = fixture.proposals.submit(&other_record.bytes()).unwrap();

    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let mine = evaluations
        .record(evaluation_draft(&fixture, &here.digest().value))
        .unwrap();
    let theirs = evaluations
        .record(evaluation_draft(&fixture, &elsewhere.digest().value))
        .unwrap();

    let connection = fixture.store.connect();
    let link = |proposal: &str, successor: &str, predecessor: &str| {
        connection.execute(
            "INSERT INTO evaluation_supersessions (
                 workspace_id, subject_digest, evaluation_digest, supersedes_evaluation_digest
             ) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                fixture.workspace_id.to_string(),
                proposal,
                successor,
                predecessor
            ],
        )
    };

    // The SUCCESSOR key: the row is scoped to this Proposal and names an Evaluation of the other.
    assert!(
        link(
            &here.digest().value,
            &theirs.digest().value,
            &mine.digest().value
        )
        .is_err(),
        "the successor foreign key admitted another Proposal's Evaluation"
    );
    // The PREDECESSOR key: the same row the other way round.
    assert!(
        link(
            &here.digest().value,
            &mine.digest().value,
            &theirs.digest().value
        )
        .is_err(),
        "the predecessor foreign key admitted another Proposal's Evaluation"
    );
    // The positive control: both ends inside one Proposal is admitted by the keys, so the two
    // refusals above are the scope and not the relation being unwritable.
    const RUBRIC_DIGEST_OFFSET: usize = 180;
    let mut bytes = mine.canonical_bytes().to_vec();
    bytes[RUBRIC_DIGEST_OFFSET] ^= 0x01;
    let sibling = HEXLOWER.encode(&sha256(&bytes));
    let rubric = HEXLOWER.encode(&bytes[RUBRIC_DIGEST_OFFSET..RUBRIC_DIGEST_OFFSET + 32]);
    connection
        .execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                    subject_kind, subject_digest, proposal_digest,
                    proposal_digest_algorithm, proposal_digest_canonicalizer, mission_id,
                    evaluator_resident_id, evaluator_configuration_id, rubric_id, rubric_version,
                    rubric_canonicalizer, ?3, verdict, conflicts_declared, conflicts_text,
                    rationale, evaluated_at_ms
             FROM evaluations WHERE digest_value = ?4",
            rusqlite::params![sibling, bytes, rubric, mine.digest().value],
        )
        .unwrap();
    assert!(
        link(&here.digest().value, &sibling, &mine.digest().value).is_ok(),
        "the keys refuse a relation inside one Proposal too"
    );
}

/// ADR 0038 promises that a Repository rebind keeping the same pinned commit reports only the
/// revision as moved — one drift, not two — and `derive_force` implements that branch. gpt2's E5:
/// no test reached it, so the branch and a version that always reported both were the same.
#[test]
fn a_rebind_keeping_the_same_pinned_commit_reports_only_the_revision_as_moved() {
    let fixture = admitted_mission("evaluation-rebind-drift");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .unwrap();
    assert!(evaluation.force().drift().is_empty());

    // A new Repository revision at the SAME pinned commit.
    fixture.rebind_repository(
        1,
        RepositorySourceKind::LocalPath,
        "/home/operator/projects/sample-tracker-moved",
        BASE_REVISION,
    );

    let drifted = evaluations.get(&evaluation.digest().value).unwrap();
    assert_eq!(
        drifted.force().drift(),
        [UnmetPrecondition::RepositoryRevisionMoved],
        "a rebind at the same commit reported the base as moved too"
    );

    // And a rebind that also moves the base reports both, so the branch above is a distinction and
    // not a blanket answer.
    fixture.rebind_repository(
        2,
        RepositorySourceKind::LocalPath,
        "/home/operator/projects/sample-tracker-moved",
        "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
    );
    let drifted = evaluations.get(&evaluation.digest().value).unwrap();
    assert_eq!(
        drifted.force().drift(),
        [
            UnmetPrecondition::RepositoryRevisionMoved,
            UnmetPrecondition::BaseRevisionMoved
        ],
        "a rebind that moved the base did not report both facts"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_ok(),
        "drift failed startup"
    );
}

// ---------------------------------------------------------------------------------------------
// Slice E2 — the Artifact subject arm in the store. One test per design bullet.
// ---------------------------------------------------------------------------------------------

impl Fixture {
    fn artifacts(&self) -> ArtifactService {
        ArtifactService::new(Arc::clone(&self.accounts), self.workspace_id)
    }

    /// One Mission-scoped Artifact, authored by `resident_id` -- so `other_resident_id` is the
    /// permitted evaluator and the author is the refused one.
    fn record_artifact(&self, content: &[u8]) -> beacond::VerifiedArtifactRecord {
        self.artifacts()
            .record(
                ArtifactKind::OutcomeDocument,
                content,
                ArtifactContext::Mission {
                    project_id: self.project_id,
                    mission_id: self.mission_id,
                },
                self.resident_id,
                self.configuration_id,
                None,
            )
            .expect("the Artifact records")
    }

    fn artifact_evaluation_draft(&self, artifact_digest: &str) -> EvaluationDraft {
        EvaluationDraft {
            subject: EvaluationSubjectDraft::Artifact(artifact_digest.to_owned()),
            evaluator: Evaluator {
                resident_id: self.other_resident_id,
                configuration_id: self.other_configuration_id,
            },
            rubric: RubricDeclaration {
                id: "artifact-review-v1".to_owned(),
                version: 1,
                canonicalizer: "beacon.rubric.v1".to_owned(),
                sha256: HEXLOWER.encode(&[0x31_u8; 32]),
            },
            cited_runs: vec![],
            verdict: Verdict::Meets,
            findings: vec![],
            conflicts: Conflicts::None,
            rationale: "The document meets the declared criteria.".to_owned(),
            supersedes: EvaluationSupersedes::None,
        }
    }
}

/// Bullet 1. An Artifact digest nothing stored writes no row and no Event.
#[test]
fn an_evaluation_of_an_artifact_that_names_no_stored_artifact_writes_neither_row_nor_event() {
    let fixture = admitted_mission("artifact-subject-absent");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let error = evaluations
        .record(fixture.artifact_evaluation_draft(&HEXLOWER.encode(&[0x99_u8; 32])))
        .expect_err("an absent Artifact is refused");
    assert!(matches!(
        error,
        EvaluationError::Inconsistent(InconsistentEvaluation::ArtifactNotFound)
    ));

    let connection = fixture.store.connect();
    let rows: i64 = connection
        .query_row("SELECT count(*) FROM evaluations", [], |row| row.get(0))
        .unwrap();
    let events: i64 = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'evaluation.recorded'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (rows, events),
        (0, 0),
        "a refused write left something behind"
    );
}

/// Bullet 2. Exactly one subject column is populated, in both directions.
#[test]
fn exactly_one_subject_column_is_populated() {
    let fixture = admitted_mission("one-subject-column");
    let artifact = fixture.record_artifact(b"# Outcome\n\nThe work is described here.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("an Artifact-subject Evaluation records");

    let connection = fixture.store.connect();
    let (kind, subject, proposal, artifact_column): (
        String,
        String,
        Option<String>,
        Option<String>,
    ) = connection
        .query_row(
            "SELECT subject_kind, subject_digest, proposal_digest, artifact_digest
                 FROM evaluations WHERE digest_value = ?1",
            [&recorded.digest().value],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(kind, "artifact");
    assert_eq!(subject, artifact.digest().value);
    assert_eq!(proposal, None, "the other arm's column is populated");
    assert_eq!(
        artifact_column.as_deref(),
        Some(artifact.digest().value.as_str())
    );

    // ...and the CHECK refuses a row that populates both, or one whose `subject_digest` names
    // neither. Written with the immutability trigger down, which is the only way to build one.
    connection
        .execute_batch("DROP TRIGGER evaluations_no_update;")
        .unwrap();
    let both = connection.execute(
        "UPDATE evaluations SET proposal_digest = ?1, proposal_digest_algorithm = 'sha256',
             proposal_digest_canonicalizer = 'beacon.proposal.v1' WHERE digest_value = ?2",
        rusqlite::params![artifact.digest().value, recorded.digest().value],
    );
    assert!(both.is_err(), "a row with both arms populated was admitted");
    let neither = connection.execute(
        "UPDATE evaluations SET subject_digest = ?1 WHERE digest_value = ?2",
        rusqlite::params![HEXLOWER.encode(&[0x77_u8; 32]), recorded.digest().value],
    );
    assert!(
        neither.is_err(),
        "a subject_digest naming neither arm's record was admitted"
    );
}

/// Bullet 3. The Artifact's accountable Resident may not evaluate it -- with the permitted
/// non-author beside it, because a trigger and a blanket refusal are otherwise indistinguishable.
#[test]
fn an_evaluator_who_is_the_artifacts_accountable_resident_is_refused_by_the_store() {
    let fixture = admitted_mission("artifact-self-evaluation");
    let artifact = fixture.record_artifact(b"# Outcome\n\nAuthored by the first Resident.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let mut author = fixture.artifact_evaluation_draft(&artifact.digest().value);
    author.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        configuration_id: fixture.configuration_id,
    };
    assert!(matches!(
        evaluations
            .record(author)
            .expect_err("the author is refused"),
        EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheArtifact)
    ));

    // The control: the other Resident, same Artifact, records.
    evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("a non-author evaluator records");
}

/// Bullet 5. An Artifact with no Mission scope is not an Evaluation subject in v1.
#[test]
fn an_artifact_without_mission_scope_cannot_be_an_evaluation_subject() {
    let fixture = admitted_mission("artifact-no-scope");
    let unscoped = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nNo Mission declared.\n",
            ArtifactContext::None,
            fixture.resident_id,
            fixture.configuration_id,
            None,
        )
        .expect("a Workspace-scoped Artifact is still a valid Artifact");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    assert!(matches!(
        evaluations
            .record(fixture.artifact_evaluation_draft(&unscoped.digest().value))
            .expect_err("there is no roster to check the evaluator against"),
        EvaluationError::Inconsistent(InconsistentEvaluation::ArtifactHasNoMissionScope)
    ));
}

/// Bullet 6. The derived Mission is re-derived against the Artifact at every read and at startup.
#[test]
fn the_derived_mission_is_rederived_against_the_artifact_at_every_read_and_at_startup() {
    let fixture = admitted_mission("artifact-mission-derived");
    let artifact = fixture.record_artifact(b"# Outcome\n\nScoped to the Mission.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("it records");
    assert!(evaluations.get(&recorded.digest().value).is_ok());

    // A Mission column that no longer matches the Artifact's own bytes: the trigger and the CHECK
    // both allow it (the Mission is a projection, and this Mission exists), so only the
    // re-derivation can catch it.
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER evaluations_no_update;")
        .unwrap();
    let other_mission = Uuid::new_v4();
    connection
        .execute(
            // Every column copied from the Mission beside it, so the decoy differs in exactly one
            // thing: which Mission the Evaluation's column names.
            "INSERT INTO missions SELECT ?1, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json, prohibitions_json,
                    lifecycle, admitted_by, admitted_at_ms
               FROM missions WHERE id = ?2",
            rusqlite::params![other_mission.to_string(), fixture.mission_id.to_string()],
        )
        .expect("a second Mission row for the same Project");
    connection
        .execute(
            "INSERT INTO mission_admission_members
               SELECT workspace_id, ?1, team_id, position, resident_id, configuration_id
               FROM mission_admission_members WHERE mission_id = ?2",
            rusqlite::params![other_mission.to_string(), fixture.mission_id.to_string()],
        )
        .expect("the same roster");
    connection
        .execute(
            "UPDATE evaluations SET mission_id = ?1 WHERE digest_value = ?2",
            rusqlite::params![other_mission.to_string(), recorded.digest().value],
        )
        .expect("the store admits the decoy: SQL cannot read the bytes");
    drop(connection);

    assert!(
        matches!(
            evaluations.get(&recorded.digest().value),
            Err(EvaluationError::Inconsistent(
                InconsistentEvaluation::MissionDisagreesWithProposal
            ))
        ),
        "the read accepted a Mission the subject Artifact does not declare"
    );
    assert!(
        Supervisor::bootstrap(StartupConfig {
            store_path: fixture.store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        })
        .is_err(),
        "startup accepted a Mission the subject Artifact does not declare"
    );
}

/// Bullet 8. Chains are keyed by subject: an Artifact chain and a Proposal chain never mix.
#[test]
fn supersession_chains_are_keyed_by_subject_and_never_mix_kinds_or_versions() {
    let fixture = admitted_mission("chains-by-subject");
    let artifact = fixture.record_artifact(b"# Outcome\n\nFirst version.\n");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    let on_artifact = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("the Artifact-subject Evaluation records");
    let on_proposal = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("the Proposal-subject Evaluation records");

    // A supersession that names the OTHER subject's Evaluation is refused: chains do not cross.
    let mut crossing = fixture.artifact_evaluation_draft(&artifact.digest().value);
    crossing.rationale = "A second reading of the same document.".to_owned();
    crossing.supersedes = EvaluationSupersedes::Evaluation(on_proposal.digest().value.clone());
    assert!(matches!(
        evaluations
            .record(crossing)
            .expect_err("chains do not cross subjects"),
        EvaluationError::Inconsistent(InconsistentEvaluation::SupersedesOtherSubject)
    ));

    // Within the subject it works, and the link is keyed on the Artifact digest.
    let mut successor = fixture.artifact_evaluation_draft(&artifact.digest().value);
    successor.rationale = "A second reading of the same document.".to_owned();
    successor.supersedes = EvaluationSupersedes::Evaluation(on_artifact.digest().value.clone());
    let second = evaluations.record(successor).expect("the chain extends");

    let connection = fixture.store.connect();
    let keyed_on: String = connection
        .query_row(
            "SELECT subject_digest FROM evaluation_supersessions WHERE evaluation_digest = ?1",
            [&second.digest().value],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(keyed_on, artifact.digest().value);
}

/// Bullet 9. The rekey carried Proposal-subject records and their chains across unchanged.
#[test]
fn proposal_subject_records_keep_their_proposal_digest_and_chain_after_the_rekey() {
    let fixture = admitted_mission("proposal-rekey");
    let record = fixture.record();
    let proposal = fixture.proposals.submit(&record.bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("it records");
    let mut successor = evaluation_draft(&fixture, &proposal.digest().value);
    successor.verdict = Verdict::Meets;
    successor.rationale = "On a second reading the criteria are met.".to_owned();
    successor.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    let second = evaluations.record(successor).expect("the chain extends");

    let connection = fixture.store.connect();
    let (kind, subject, proposal_column): (String, String, Option<String>) = connection
        .query_row(
            "SELECT subject_kind, subject_digest, proposal_digest FROM evaluations
             WHERE digest_value = ?1",
            [&first.digest().value],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(kind, "proposal");
    assert_eq!(subject, proposal.digest().value);
    assert_eq!(
        proposal_column.as_deref(),
        Some(proposal.digest().value.as_str()),
        "the Proposal arm kept its own column through the rekey"
    );

    let keyed_on: String = connection
        .query_row(
            "SELECT subject_digest FROM evaluation_supersessions WHERE evaluation_digest = ?1",
            [&second.digest().value],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(keyed_on, proposal.digest().value);
    assert!(evaluations.get(&first.digest().value).is_ok());
    assert!(evaluations.get(&second.digest().value).is_ok());
}

/// Bullet 4. Non-authorship is keyed on the RESIDENT, never on the (Resident, Configuration) pair.
///
/// An author who reconfigures is still the author. If the comparison used the pair, changing
/// configuration would be a way to evaluate your own work, which is the whole rule undone by a
/// field nobody would think to look at.
#[test]
fn a_second_configuration_of_the_accountable_resident_is_still_that_resident() {
    let fixture = admitted_mission("author-reconfigured");
    let artifact = fixture.record_artifact(b"# Outcome\n\nAuthored under one configuration.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);

    // The author, under a DIFFERENT configuration than the one the Artifact names.
    let mut author = fixture.artifact_evaluation_draft(&artifact.digest().value);
    author.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        configuration_id: fixture.other_configuration_id,
    };
    assert_ne!(fixture.other_configuration_id, fixture.configuration_id);
    let error = evaluations
        .record(author)
        .expect_err("a reconfigured author is still the author");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheArtifact)
        ),
        "refused for the wrong reason: {error}"
    );
}

/// Bullet 7, and the store half of bullet 3. The truthful forbidden row: bytes and columns that
/// AGREE with each other and name the author as the evaluator.
///
/// The first version of this test spliced only the evaluator COLUMN, which `columns_match` catches
/// first -- so it went green whether or not the non-authorship re-derivation existed. My own mutant
/// table found that (M5 was killed by the Proposal-arm test, never by this one). The row below is
/// built from re-encoded bytes, so every other check passes and the re-derivation is the only thing
/// left that can refuse it.
#[test]
fn an_artifact_subject_evaluation_written_past_the_command_refuses_startup() {
    let fixture = admitted_mission("artifact-forbidden-row");
    let artifact = fixture.record_artifact(b"# Outcome\n\nAuthored by the first Resident.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let honest = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("the honest Evaluation records");

    // The same record with the AUTHOR as evaluator, encoded by the codec -- which has no opinion
    // about authorship, and should not: it cannot see the Artifact.
    let mut payload = honest.payload().clone();
    payload.evaluator = Evaluator {
        resident_id: fixture.resident_id,
        configuration_id: fixture.configuration_id,
    };
    let bytes = payload
        .canonical_bytes()
        .expect("the forbidden payload encodes");
    let digest = HEXLOWER.encode(&Sha256::digest(&bytes));

    let connection = fixture.store.connect();
    let insert = |connection: &Connection| {
        connection.execute(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, subject_kind, subject_digest,
                 artifact_digest, artifact_digest_algorithm, artifact_digest_canonicalizer,
                 artifact_project_id, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             ) SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2,
                      subject_kind, subject_digest, artifact_digest, artifact_digest_algorithm,
                      artifact_digest_canonicalizer, artifact_project_id, mission_id, ?3, ?4,
                      rubric_id, rubric_version, rubric_canonicalizer, rubric_sha256, verdict,
                      conflicts_declared, conflicts_text, rationale, evaluated_at_ms
                FROM evaluations WHERE digest_value = ?5",
            rusqlite::params![
                digest,
                bytes,
                fixture.resident_id.to_string(),
                fixture.configuration_id.to_string(),
                honest.digest().value
            ],
        )
    };

    // The STORE refuses it first, by its own trigger, and says which rule refused.
    let refused = insert(&connection).expect_err("the trigger admits the author as evaluator");
    assert!(
        refused
            .to_string()
            .contains("accountable Resident of the subject Artifact"),
        "refused by something other than the non-authorship trigger: {refused}"
    );

    // The trigger keys on the RESIDENT, asserted against its own text -- read BEFORE the drop
    // below, because a test that reads a trigger it has already dropped asserts nothing.
    //
    // This one is a text assertion because the discriminating row cannot be built: to tell
    // "keyed on the Resident" from "keyed on the Configuration" behaviourally you need an
    // evaluator who shares one and not the other, and the roster foreign key
    // `(workspace, mission, resident, configuration)` refuses every such row before any trigger
    // runs. So the property is pinned where it lives. (My mutant table found this: M9 flips the
    // trigger to the configuration column and every behavioural test stays green.)
    let trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master
             WHERE type = 'trigger' AND name = 'evaluations_evaluator_is_not_the_artifact_author'",
            [],
            |row| row.get(0),
        )
        .expect("the trigger exists");
    assert!(
        trigger.contains("artifacts.accountable_resident_id = NEW.evaluator_resident_id"),
        "the non-authorship trigger no longer keys on the Resident: {trigger}"
    );
    assert!(
        !trigger.contains("accountable_configuration_id"),
        "the non-authorship trigger reads a configuration: an author who reconfigures is still \
         the author"
    );

    // With that one trigger down -- and every other check intact -- the row lands, and the
    // re-derivation from the BYTES is the only thing left to catch it.
    connection
        .execute_batch("DROP TRIGGER evaluations_evaluator_is_not_the_artifact_author;")
        .unwrap();
    insert(&connection).expect("with the trigger down the row is storable");
    drop(connection);

    let error = evaluations
        .get(&digest)
        .expect_err("the read accepted an Evaluation whose evaluator authored the Artifact");
    assert!(
        matches!(
            error,
            EvaluationError::Inconsistent(InconsistentEvaluation::EvaluatorAuthoredTheArtifact)
        ),
        "refused for the wrong reason: {error}"
    );
}

/// sol1's P0 probe, committed as its test: an unchanged store holding one valid Artifact-subject
/// Evaluation must still start.
///
/// `verify_evaluation_state` enumerated chains with `SELECT DISTINCT workspace_id, proposal_digest`
/// read as `String`, and migration 0028 makes that column NULL for the Artifact arm -- so the first
/// valid Artifact Evaluation made the whole store refuse startup. A Proposal-only call site the arm
/// did not generalize. The positive control -- recording and reading before the restart -- is what
/// makes the restart assertion mean something.
#[test]
fn a_valid_artifact_subject_evaluation_survives_restart() {
    let fixture = admitted_mission("artifact-subject-restart-positive");
    let artifact = fixture
        .record_artifact(b"# Outcome\n\nAuthored by one Resident and evaluated by the other.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("the valid Artifact-subject Evaluation records");
    evaluations
        .get(&recorded.digest().value)
        .expect("the positive control reads before restart");

    let restart = Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        restart.is_ok(),
        "an unchanged store holding a valid Artifact-subject Evaluation refused restart: \
         {restart:?}"
    );
}

/// sol1's both-links probe, committed as its test (R12.10 applied twice).
///
/// A surface showing an Evaluation of an Artifact shows the cited Evaluation digest with its chain
/// tail AND the cited Artifact digest with its supersession tail -- **beside, never instead**. E2
/// made Artifact-subject Evaluations readable through the public service, so deferring this field
/// left a temporarily lying surface rather than an uncalled seam.
#[test]
fn an_artifact_evaluation_surface_reports_the_subject_artifacts_successor() {
    let fixture = admitted_mission("artifact-subject-both-links");
    let artifact = fixture.record_artifact(b"# Outcome\n\nFirst version.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let evaluation = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("the Artifact-subject Evaluation records");
    assert_eq!(
        evaluation.subject_superseded_by(),
        None,
        "nothing has superseded the subject yet"
    );

    let predecessor = HEXLOWER.decode(artifact.digest().value.as_bytes()).unwrap();
    let mut predecessor_digest = [0_u8; 32];
    predecessor_digest.copy_from_slice(&predecessor);
    let successor = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nSecond version.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            Some(predecessor_digest),
        )
        .expect("the subject Artifact is superseded");

    let reread = evaluations.get(&evaluation.digest().value).unwrap();
    assert_eq!(
        reread.subject_superseded_by(),
        Some(successor.digest().value.as_str()),
        "the Evaluation surface omits the cited Artifact's read-time successor"
    );
    // BESIDE, never instead: the citation still names the exact bytes evaluated, and the
    // Evaluation's own chain tail is a different fact again.
    assert_eq!(
        reread.payload().subject.digest_value(),
        artifact.digest().value,
        "the cited digest was resolved to the successor"
    );
    assert_eq!(reread.force().superseded_by(), None);
}

/// sol1's rekey probe, committed as its test: a v1 Evaluation chain recorded BEFORE the migration
/// survives the real 27 -> 28 startup byte-identically.
///
/// `proposal_subject_records_keep_their_proposal_digest_and_chain_after_the_rekey` builds its
/// fixture at the CURRENT schema, so it checks the new schema's behaviour and never crosses the
/// migration. This one reconstructs the exact 0023 Evaluation objects, stamps version 27, and lets
/// the real ladder run -- and sol1 proved it reaches the migration by making the copied
/// `canonical_bytes` equal-length zeroes and watching it go RED.

#[test]
fn migration_0028_preserves_preexisting_proposal_evaluation_bytes_and_chain() {
    let fixture = admitted_mission("proposal-evaluation-rekey-migration");
    let proposal = fixture.proposals.submit(&fixture.record().bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("the pre-migration Evaluation records");
    let mut successor = evaluation_draft(&fixture, &proposal.digest().value);
    successor.verdict = Verdict::Meets;
    successor.rationale = "A second reading now meets the rubric.".to_owned();
    successor.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    let second = evaluations
        .record(successor)
        .expect("the pre-migration chain extends");
    let pinned = [
        (
            first.digest().value.clone(),
            first.canonical_bytes().to_vec(),
        ),
        (
            second.digest().value.clone(),
            second.canonical_bytes().to_vec(),
        ),
    ];

    let connection = fixture.store.connect();
    let events_before = {
        let mut statement = connection
            .prepare(
                "SELECT sequence, payload_json, occurred_at_ms FROM events
                 WHERE kind = 'evaluation.recorded' ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TEMP TABLE evaluations_snapshot AS SELECT * FROM evaluations;
             CREATE TEMP TABLE findings_snapshot AS SELECT * FROM evaluation_findings;
             CREATE TEMP TABLE cited_runs_snapshot AS SELECT * FROM evaluation_cited_runs;
             CREATE TEMP TABLE supersessions_snapshot AS SELECT * FROM evaluation_supersessions;
             CREATE TEMP TABLE classifications_snapshot AS
                 SELECT * FROM mission_impact_classifications;
             -- A store at 27 does not have migration 0029's Outcome manifest store either, and
             -- startup verifies the schema a store CLAIMS before it migrates it: leaving a later
             -- migration's objects behind under a 27 stamp is a mis-stamped store, which is
             -- refused rather than migrated. Newest migration off first.
             DROP TRIGGER outcome_publications_no_update;
             DROP TRIGGER outcome_publications_no_delete;
             DROP INDEX outcome_publications_by_mission;
             DROP TABLE outcome_publications;
             DROP TRIGGER outcome_manifests_no_update;
             DROP TRIGGER outcome_manifests_no_delete;
             DROP INDEX outcome_manifests_by_mission;
             DROP TABLE outcome_manifests;
             DROP TRIGGER mission_impact_classifications_no_update;
             DROP TRIGGER mission_impact_classifications_no_delete;
             DROP TABLE mission_impact_classifications;
             DROP TABLE evaluation_supersessions;
             DROP TABLE evaluation_cited_runs;
             DROP TABLE evaluation_findings;
             DROP TABLE evaluations;
             -- R46's 0033 adds the last SQL outcome and R48's 0034 removes that membership CHECK;
             -- both rebuild `receipts` and rewrite `evaluation_cited_runs` with it (the FK follows
             -- a rename). This fixture rolls the store back to 27, so it has to roll both back too
             -- -- before R46 they were untouched since 0022 and this snapshot did not name them.
             DROP TRIGGER receipts_no_update;
             DROP TRIGGER receipts_no_delete;
             ALTER TABLE receipts RENAME TO receipts_r46;",
        )
        .unwrap();
    // `receipts` at its 0022 shape, sliced out of 0022's own text for the same reason the
    // evaluation slice below gives: a rollback that retyped the DDL would be testing the test.
    // Its two triggers come with it -- 0022's block ends at the next table.
    let migration_0022 = include_str!("../migrations/0022_candidates.sql");
    // The CREATE and the two triggers, but NOT 0022's own carry-over INSERT from `receipts_v21`:
    // that table belongs to 0022's rebuild, not to this one.
    let receipt_table = &migration_0022[migration_0022.find("CREATE TABLE receipts (").unwrap()
        ..migration_0022
            .find("CREATE TABLE dispatch_preparations (")
            .unwrap()];
    let receipt_create =
        &receipt_table[..receipt_table.find(") STRICT;").unwrap() + ") STRICT;".len()];
    let receipt_triggers = &receipt_table[receipt_table
        .find("CREATE TRIGGER receipts_no_update")
        .unwrap()..];
    connection.execute_batch(receipt_create).unwrap();
    connection.execute_batch(receipt_triggers).unwrap();
    connection
        .execute_batch(
            "INSERT INTO receipts SELECT * FROM receipts_r46;
             DROP TABLE receipts_r46;",
        )
        .unwrap();

    let migration_0023 = include_str!("../migrations/0023_evaluations.sql");
    let evaluation_objects =
        &migration_0023[migration_0023.find("CREATE TABLE evaluations (").unwrap()
            ..migration_0023.find("UPDATE schema_meta").unwrap()];
    connection.execute_batch(evaluation_objects).unwrap();
    // 0032 rebuilt `mission_impact_classifications` to bind a Mission to its Project, so a store
    // rolled back below it must carry 0025's shape or the version-27 signature will not match.
    // Sliced out of 0025's own text for the reason the evaluation slice above gives: a rollback
    // that retyped the DDL would be testing the test.
    let migration_0025 = include_str!("../migrations/0025_spend_and_impact.sql");
    let classification_objects = &migration_0025[migration_0025
        .find("CREATE TABLE mission_impact_classifications (")
        .unwrap()
        ..migration_0025.find("UPDATE schema_meta").unwrap()];
    connection.execute_batch(classification_objects).unwrap();
    connection
        .execute_batch(
            "INSERT INTO mission_impact_classifications
                 SELECT * FROM classifications_snapshot;",
        )
        .unwrap();
    connection
        .execute_batch(
            "INSERT INTO evaluations (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                    canonical_bytes, proposal_digest, proposal_digest_algorithm,
                    proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                    evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                    rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                    evaluated_at_ms
             FROM evaluations_snapshot ORDER BY digest_value;
             INSERT INTO evaluation_findings SELECT * FROM findings_snapshot;
             INSERT INTO evaluation_cited_runs SELECT * FROM cited_runs_snapshot;
             INSERT INTO evaluation_supersessions (
                 workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
             )
             SELECT workspace_id, subject_digest, evaluation_digest,
                    supersedes_evaluation_digest
             FROM supersessions_snapshot;
             UPDATE schema_meta SET version = 27 WHERE singleton = 1 AND version = 34;
             COMMIT;",
        )
        .unwrap();
    // The stamp above is guarded on the version the store actually starts at, and a guard that
    // matches nothing is a silent no-op: when the ladder grew 0029 this UPDATE stopped firing and
    // the failure surfaced downstream as a `SchemaMismatch` from a store that still claimed the
    // current version. Assert the rollback landed, so the next migration lane is told HERE.
    assert_eq!(
        connection
            .query_row(
                "SELECT version FROM schema_meta WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        27,
        "the rollback did not stamp the store at 27 -- update its guard for the new ladder head"
    );
    drop(connection);

    Supervisor::bootstrap(StartupConfig {
        store_path: fixture.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect("the real 27-to-28 startup migration succeeds");

    let connection = fixture.store.connect();
    for (digest, bytes) in &pinned {
        let (stored_digest, stored_bytes): (String, Vec<u8>) = connection
            .query_row(
                "SELECT digest_value, canonical_bytes FROM evaluations WHERE digest_value = ?1",
                [digest],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            &stored_digest, digest,
            "the rekey changed Evaluation identity"
        );
        assert_eq!(&stored_bytes, bytes, "the rekey changed canonical bytes");
        evaluations
            .get(digest)
            .expect("the migrated Evaluation still verifies");
    }
    let subject_key: String = connection
        .query_row(
            "SELECT subject_digest FROM evaluation_supersessions WHERE evaluation_digest = ?1",
            [&second.digest().value],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(subject_key, proposal.digest().value);
    let events_after = {
        let mut statement = connection
            .prepare(
                "SELECT sequence, payload_json, occurred_at_ms FROM events
                 WHERE kind = 'evaluation.recorded' ORDER BY sequence",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        events_after, events_before,
        "the migration changed Event bytes"
    );
}

// ---------------------------------------------------------------------------------------------
// Slice E3 — the verified reader's seam contract and the both-links surface.
// ---------------------------------------------------------------------------------------------

/// E3 bullet 1. Every read re-verifies the SUBJECT Artifact's bytes and digest, not only the
/// Evaluation's own.
///
/// The Evaluation's bytes can be perfect while the record they cite has rotted: the reader reaches
/// the Artifact through its owning module, so a subject whose stored bytes no longer hash to its
/// digest takes the Evaluation with it rather than being read past.
#[test]
fn the_verified_reader_reverifies_the_subject_artifacts_bytes_and_digest_on_every_read() {
    let fixture = admitted_mission("e3-subject-reverified");
    let artifact = fixture.record_artifact(b"# Outcome\n\nThe subject document.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("it records");
    evaluations
        .get(&recorded.digest().value)
        .expect("the positive control reads");

    // Tamper with the SUBJECT's bytes only. The Evaluation's own row is untouched and still hashes
    // to its own digest, so only a reader that follows the citation can notice.
    let connection = fixture.store.connect();
    connection
        .execute_batch("DROP TRIGGER artifacts_no_update;")
        .unwrap();
    let mut rotted: Vec<u8> = connection
        .query_row(
            "SELECT canonical_bytes FROM artifacts WHERE digest_value = ?1",
            [&artifact.digest().value],
            |row| row.get(0),
        )
        .unwrap();
    let last = rotted.len() - 1;
    rotted[last] ^= 0xff;
    connection
        .execute(
            "UPDATE artifacts SET canonical_bytes = ?1 WHERE digest_value = ?2",
            rusqlite::params![rotted, artifact.digest().value],
        )
        .expect("the store admits it: SQL cannot hash");
    drop(connection);

    assert!(
        evaluations.get(&recorded.digest().value).is_err(),
        "the read accepted an Evaluation whose subject Artifact no longer verifies"
    );
}

/// E3 bullet 3. Drift is empty for an Artifact subject, and subject supersession is NOT folded into
/// it.
///
/// Two absences that mean different things: an Artifact record carries no Charter, Policy,
/// Repository or base revision, so nothing could have moved under it -- while its successor is a
/// real fact about the world, reported in its own field. Folding the second into the first would
/// make "the document was replaced" read as "a precondition moved".
#[test]
fn drift_is_empty_for_artifact_subjects_and_subject_supersession_is_not_folded_into_it() {
    let fixture = admitted_mission("e3-drift-empty");
    let artifact = fixture.record_artifact(b"# Outcome\n\nFirst version.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("it records");
    assert!(recorded.force().drift().is_empty());

    let mut predecessor_digest = [0_u8; 32];
    predecessor_digest
        .copy_from_slice(&HEXLOWER.decode(artifact.digest().value.as_bytes()).unwrap());
    let successor = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nSecond version.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            Some(predecessor_digest),
        )
        .expect("the subject is superseded");

    let reread = evaluations.get(&recorded.digest().value).unwrap();
    assert!(
        reread.force().drift().is_empty(),
        "the subject's supersession was reported as drift"
    );
    assert_eq!(
        reread.subject_superseded_by(),
        Some(successor.digest().value.as_str()),
        "the successor is reported in its own field"
    );
}

/// E3 bullet 4. A citation of a superseded Artifact shows BOTH links at read time.
///
/// The manifest-shaped question: what a surface must be able to say about a published Evaluation of
/// a document that has since moved on. Both facts, at once, neither replacing the other.
#[test]
fn a_manifest_citation_of_a_superseded_artifact_shows_both_links_at_read_time() {
    let fixture = admitted_mission("e3-both-links-manifest");
    let artifact = fixture.record_artifact(b"# Outcome\n\nThe evaluated version.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("it records");

    // The Evaluation chain moves...
    let mut successor_draft = fixture.artifact_evaluation_draft(&artifact.digest().value);
    successor_draft.rationale = "A second reading of the same bytes.".to_owned();
    successor_draft.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    let second = evaluations
        .record(successor_draft)
        .expect("the chain extends");

    // ...and so does the document.
    let mut predecessor_digest = [0_u8; 32];
    predecessor_digest
        .copy_from_slice(&HEXLOWER.decode(artifact.digest().value.as_bytes()).unwrap());
    let newer = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nA later version of the document.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            Some(predecessor_digest),
        )
        .expect("the subject is superseded");

    let citation = evaluations.get(&first.digest().value).unwrap();
    // The cited identities, unmoved: this is what the Evaluation is about.
    assert_eq!(citation.digest().value, first.digest().value);
    assert_eq!(
        citation.payload().subject.digest_value(),
        artifact.digest().value
    );
    // And both read-time tails, beside them.
    assert_eq!(
        citation.force().superseded_by().map(|d| d.value.as_str()),
        Some(second.digest().value.as_str()),
        "the Evaluation's own chain tail is missing"
    );
    assert_eq!(
        citation.subject_superseded_by(),
        Some(newer.digest().value.as_str()),
        "the subject Artifact's tail is missing"
    );
}

/// E3 bullet 5. The publication seam decides from the two records alone (§8, R4.3).
///
/// The relation is Outcome slice 3's; this exercises the contract it consumes, in the shape a
/// consumer would use it, and pins what does NOT cross: the candidacy carries two digests and a
/// Workspace, and no rendering of the verdict, the rationale or the evaluator.
#[test]
fn the_publication_seam_can_check_subject_verdict_and_nonauthorship_from_the_two_records_alone() {
    let fixture = admitted_mission("e3-publication-seam");
    let artifact = fixture.record_artifact(b"# Outcome\n\nThe candidate.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let meets = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("a `meets` Evaluation records");

    let connection = fixture.store.connect();
    let candidacy = publication_candidacy(
        &connection,
        fixture.workspace_id,
        meets.digest(),
        artifact.digest(),
        fixture.mission_id,
    )
    .expect("the pairing is publishable");
    assert_eq!(candidacy.evaluation_digest(), meets.digest());
    assert_eq!(candidacy.candidate_artifact_digest(), artifact.digest());
    // The two identities are distinct TYPES, so a caller cannot swap them: the line below is the
    // shape of that mistake, and it does not compile.
    //
    //   publication_candidacy(&connection, ws, artifact.digest(), meets.digest(), mission)
    //   ^ expected `&EvaluationDigest`, found `&ArtifactDigest`
    //
    // ...and a hand-built envelope with the right value and the wrong canonicalizer is refused,
    // because types stop a swap and not a forge.
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            &EvaluationDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.evaluation.v2".to_owned(),
                value: meets.digest().value.clone(),
            },
            artifact.digest(),
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::CitedEnvelopeDisagrees)
    );
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            meets.digest(),
            &ArtifactDigest {
                algorithm: "sha512".to_owned(),
                canonicalizer: "beacon.artifact.v1".to_owned(),
                value: artifact.digest().value.clone(),
            },
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::CitedEnvelopeDisagrees)
    );
    assert_eq!(candidacy.workspace_id(), fixture.workspace_id);

    // A verdict that is not `meets` is not a soft yes.
    let mut uncertain_draft = fixture.artifact_evaluation_draft(&artifact.digest().value);
    uncertain_draft.verdict = Verdict::Uncertain;
    uncertain_draft.rationale = "The criteria could not be assessed from the document.".to_owned();
    uncertain_draft.supersedes = EvaluationSupersedes::Evaluation(meets.digest().value.clone());
    let uncertain = evaluations.record(uncertain_draft).expect("it records");
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            uncertain.digest(),
            artifact.digest(),
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::VerdictIsNotMeets)
    );

    // A Proposal-subject Evaluation is not a candidate for publication at all.
    let proposal = fixture.proposals.submit(&fixture.record().bytes()).unwrap();
    let on_proposal = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("it records");
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            on_proposal.digest(),
            artifact.digest(),
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::SubjectIsNotAnArtifact)
    );

    // An Evaluation about DIFFERENT bytes than the candidate named.
    let other = fixture.record_artifact(b"# Outcome\n\nA different document.\n");
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            meets.digest(),
            other.digest(),
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::SubjectIsNotTheCandidate)
    );

    // A different Outcome Mission, and absent records on both sides.
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            meets.digest(),
            artifact.digest(),
            Uuid::new_v4(),
        ),
        Err(PublicationSeamRefusal::MissionDiffers)
    );
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            &EvaluationDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.evaluation.v1".to_owned(),
                value: HEXLOWER.encode(&[0x5a_u8; 32]),
            },
            artifact.digest(),
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::EvaluationAbsent)
    );
    assert_eq!(
        publication_candidacy(
            &connection,
            fixture.workspace_id,
            meets.digest(),
            &ArtifactDigest {
                algorithm: "sha256".to_owned(),
                canonicalizer: "beacon.artifact.v1".to_owned(),
                value: HEXLOWER.encode(&[0x5b_u8; 32]),
            },
            fixture.mission_id,
        ),
        Err(PublicationSeamRefusal::CandidateAbsent)
    );
}

/// Fable's mutant, closed: **both** public spellings of the second link are read here.
///
/// `Evaluation::subject_superseded_by` and `EvaluationForce::subject_superseded_by` are two public
/// accessors for one fact, and only the first was read by any test — so forcing the second to
/// `None` left all 44 green while a consumer reading the force type saw "nothing has superseded
/// this" about a document that had. Two surfaces for one fact need both read, or the untested one
/// is free to lie.
///
/// The seam's whole currency story rests on this: `publication_seam` deliberately does NOT refuse a
/// superseded candidate, because surfacing it is the read-time rule. If the read-time rule can
/// silently return `None`, that decision becomes "nobody is told" rather than "the reader is told".
#[test]
fn both_public_spellings_of_the_subject_link_report_the_successor_beside_the_cited_digest() {
    let fixture = admitted_mission("e3-both-accessors");
    let artifact = fixture.record_artifact(b"# Outcome\n\nThe evaluated bytes.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&artifact.digest().value))
        .expect("it records");

    // Before: nothing has superseded it, through both spellings.
    assert_eq!(recorded.subject_superseded_by(), None);
    assert_eq!(recorded.force().subject_superseded_by(), None);

    let mut predecessor_digest = [0_u8; 32];
    predecessor_digest
        .copy_from_slice(&HEXLOWER.decode(artifact.digest().value.as_bytes()).unwrap());
    let successor = fixture
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Outcome\n\nThe version that replaced it.\n",
            ArtifactContext::Mission {
                project_id: fixture.project_id,
                mission_id: fixture.mission_id,
            },
            fixture.resident_id,
            fixture.configuration_id,
            Some(predecessor_digest),
        )
        .expect("the subject is superseded");

    let reread = evaluations.get(&recorded.digest().value).unwrap();

    // The cited digest, unmoved: what the Evaluation is about.
    assert_eq!(
        reread.payload().subject.digest_value(),
        artifact.digest().value
    );
    // ...and the tail, through BOTH accessors, carrying its full envelope on the force type.
    assert_eq!(
        reread.subject_superseded_by(),
        Some(successor.digest().value.as_str()),
        "Evaluation::subject_superseded_by reports no successor"
    );
    let on_force = reread
        .force()
        .subject_superseded_by()
        .expect("EvaluationForce::subject_superseded_by reports no successor");
    assert_eq!(on_force.value, successor.digest().value);
    assert_eq!(on_force.canonicalizer, "beacon.artifact.v1");
    assert_eq!(on_force.algorithm, "sha256");
    // The two spellings agree, which is the property that makes either safe to read.
    assert_eq!(
        Some(on_force.value.as_str()),
        reread.subject_superseded_by()
    );
}

/// sol1's P0, on the SUBJECT side: with `v1 -> v2 -> v3` the successor and the tail are different
/// records, and R12.10 asks for the tail.
///
/// The committed test had one hop, where the two are indistinguishable — so a reader that stopped
/// at the first link passed it while reporting a version that had itself been replaced. That is the
/// failure the rule exists to prevent: a surface saying "superseded by v2" about a document now at
/// v3 is worse than saying nothing, because it reads as current.
#[test]
fn the_subject_link_reports_the_tail_of_the_artifact_chain_not_the_next_version() {
    let fixture = admitted_mission("e3-subject-tail");
    let v1 = fixture.record_artifact(b"# Outcome\n\nVersion one.\n");
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let recorded = evaluations
        .record(fixture.artifact_evaluation_draft(&v1.digest().value))
        .expect("it records");

    let supersede = |predecessor: &str, content: &[u8]| {
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&HEXLOWER.decode(predecessor.as_bytes()).unwrap());
        fixture
            .artifacts()
            .record(
                ArtifactKind::OutcomeDocument,
                content,
                ArtifactContext::Mission {
                    project_id: fixture.project_id,
                    mission_id: fixture.mission_id,
                },
                fixture.resident_id,
                fixture.configuration_id,
                Some(digest),
            )
            .expect("the version records")
    };
    let v2 = supersede(&v1.digest().value, b"# Outcome\n\nVersion two.\n");
    let v3 = supersede(&v2.digest().value, b"# Outcome\n\nVersion three.\n");
    assert_ne!(v2.digest().value, v3.digest().value);

    let reread = evaluations.get(&recorded.digest().value).unwrap();
    assert_eq!(
        reread.subject_superseded_by(),
        Some(v3.digest().value.as_str()),
        "the subject link reported the next version instead of the tail"
    );
    assert_eq!(
        reread
            .force()
            .subject_superseded_by()
            .map(|d| d.value.as_str()),
        Some(v3.digest().value.as_str())
    );
    // The cited digest is still v1: the tail is reported beside it, never in place of it.
    assert_eq!(reread.payload().subject.digest_value(), v1.digest().value);
}

/// sol1's P0, on the EVALUATION side: the same two-hop shape for the Evaluation's own chain.
///
/// Both arms take this path — the Proposal arm's `derive_force` had its own one-hop query and its
/// own synthesized envelope, a third spelling of one rule — so this is asserted for a Proposal
/// subject, where the third site lived.
#[test]
fn the_evaluation_chain_link_reports_the_tail_not_the_next_evaluation() {
    let fixture = admitted_mission("e3-chain-tail");
    let proposal = fixture.proposals.submit(&fixture.record().bytes()).unwrap();
    let evaluations = EvaluationService::new(Arc::clone(&fixture.accounts), fixture.workspace_id);
    let first = evaluations
        .record(evaluation_draft(&fixture, &proposal.digest().value))
        .expect("the first records");

    let extend = |predecessor: &str, rationale: &str| {
        let mut draft = evaluation_draft(&fixture, &proposal.digest().value);
        draft.rationale = rationale.to_owned();
        draft.supersedes = EvaluationSupersedes::Evaluation(predecessor.to_owned());
        evaluations.record(draft).expect("the chain extends")
    };
    let second = extend(&first.digest().value, "A second reading of the same bytes.");
    let third = extend(&second.digest().value, "A third reading of the same bytes.");
    assert_ne!(second.digest().value, third.digest().value);

    let reread = evaluations.get(&first.digest().value).unwrap();
    assert_eq!(
        reread.force().superseded_by().map(|d| d.value.as_str()),
        Some(third.digest().value.as_str()),
        "the chain link reported the next Evaluation instead of the tail"
    );
    // ...and the tail's own envelope comes from reading that record, not from constants here.
    let tail = reread.force().superseded_by().unwrap();
    assert_eq!(tail.canonicalizer, "beacon.evaluation.v1");
    assert_eq!(tail.algorithm, "sha256");
    // The middle link still reports the same tail, and the tail reports none.
    assert_eq!(
        evaluations
            .get(&second.digest().value)
            .unwrap()
            .force()
            .superseded_by()
            .map(|d| d.value.as_str()),
        Some(third.digest().value.as_str())
    );
    assert_eq!(
        evaluations
            .get(&third.digest().value)
            .unwrap()
            .force()
            .superseded_by(),
        None
    );
}
// Astra seam audit at 572a92ff. Temporary-store corruption only; production code is unchanged.
fn astra_allow_update(conn: &Connection, trigger: &str) -> String {
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
            [trigger],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch(&format!("DROP TRIGGER {trigger};"))
        .unwrap();
    sql
}

fn astra_restart(f: &Fixture) -> Result<beacond::StartupReport, beacond::SupervisorError> {
    Supervisor::bootstrap(StartupConfig {
        store_path: f.store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
}

fn astra_other_mission(f: &Fixture) -> Uuid {
    let missions = MissionService::new(Arc::clone(&f.accounts), f.workspace_id);
    let current = missions.get(f.mission_id).unwrap();
    missions
        .admit(MissionDraft {
            project_id: current.project_id,
            expected_charter_revision: current.charter_revision,
            team_id: current.team_id,
            desired_outcome: "A distinct Mission for a seam scope control.".to_owned(),
            success_conditions: vec!["A different result".to_owned()],
            constraints: vec![],
            prohibitions: vec![],
            members: current
                .members
                .iter()
                .map(|m| MissionMemberDraft {
                    resident_id: m.resident_id,
                    expected_configuration_id: m.admission_configuration_id,
                })
                .collect(),
        })
        .unwrap()
        .id
}

fn astra_successor(f: &Fixture, previous: &str, content: &[u8]) -> beacond::VerifiedArtifactRecord {
    let digest: [u8; 32] = HEXLOWER
        .decode(previous.as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    f.artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            content,
            ArtifactContext::Mission {
                project_id: f.project_id,
                mission_id: f.mission_id,
            },
            f.resident_id,
            f.configuration_id,
            Some(digest),
        )
        .unwrap()
}

fn astra_pair(
    f: &Fixture,
) -> (
    beacond::VerifiedArtifactRecord,
    EvaluationService,
    beacond::Evaluation,
) {
    let a = f.record_artifact(b"# Outcome\n\nOriginal cited bytes.\n");
    let es = EvaluationService::new(Arc::clone(&f.accounts), f.workspace_id);
    let e = es
        .record(f.artifact_evaluation_draft(&a.digest().value))
        .unwrap();
    (a, es, e)
}

fn astra_seam(
    f: &Fixture,
    e: &beacond::Evaluation,
    a: &beacond::VerifiedArtifactRecord,
) -> Result<beacond::PublicationCandidacy, PublicationSeamRefusal> {
    publication_candidacy(
        &f.store.connect(),
        f.workspace_id,
        e.digest(),
        a.digest(),
        f.mission_id,
    )
}

#[test]
fn astra_control_valid_chain_restart_and_exact_collector_citation() {
    let f = admitted_mission("astra-valid-chain");
    let (a, es, e) = astra_pair(&f);
    let b = astra_successor(&f, &a.digest().value, b"# Second\n");
    let c = astra_successor(&f, &b.digest().value, b"# Third\n");
    assert_eq!(
        es.get(&e.digest().value)
            .unwrap()
            .force()
            .subject_superseded_by(),
        Some(c.digest())
    );
    assert!(astra_restart(&f).is_ok());
    assert!(astra_seam(&f, &e, &a).is_ok());
    let conn = f.store.connect();
    let cited = beacond::Collector::new(&conn, f.workspace_id)
        .artifact(&a.digest().value, f.mission_id)
        .unwrap();
    assert_eq!(cited.digest_value, a.digest().value);
}

#[test]
fn astra_s1_subject_tail_cannot_jump_to_another_missions_unrelated_artifact() {
    let f = admitted_mission("astra-foreign-mission-tail");
    let other_mission = astra_other_mission(&f);
    let (a, es, e) = astra_pair(&f);
    let b = f
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Other Mission\n",
            ArtifactContext::Mission {
                project_id: f.project_id,
                mission_id: other_mission,
            },
            f.resident_id,
            f.configuration_id,
            None,
        )
        .unwrap();
    assert!(astra_restart(&f).is_ok());
    assert!(astra_seam(&f, &e, &a).is_ok());
    let conn = f.store.connect();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    conn.execute(
        "INSERT INTO artifact_supersessions (workspace_id, artifact_digest, supersedes_digest)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![
            f.workspace_id.to_string(),
            b.digest().value,
            a.digest().value
        ],
    )
    .unwrap();
    let read = es.get(&e.digest().value);
    let seam = astra_seam(&f, &e, &a);
    eprintln!(
        "S1 foreign_mission={} tail={:?} seam={:?}",
        other_mission,
        read.as_ref().map(|x| x.force().subject_superseded_by()),
        seam
    );
    // The known Artifact startup ladder refuses this row. The new question is the READ consumer.
    assert!(astra_restart(&f).is_err());
    assert!(
        read.is_err() && seam.is_err(),
        "an unrelated other-Mission Artifact became a verified subject tail"
    );
}

#[test]
fn astra_control_foreign_workspace_tail_is_refused() {
    let f = admitted_mission("astra-foreign-ws-tail");
    let other = admitted_mission("astra-foreign-ws-target");
    let (a, es, e) = astra_pair(&f);
    let b = other.record_artifact(b"# Elsewhere\n");
    // Disable FKs only on this temporary-store test connection: emulate a bad restore.
    let conn = f.store.connect();
    conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
    conn.execute(
        "INSERT INTO artifact_supersessions (workspace_id, artifact_digest, supersedes_digest)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![
            f.workspace_id.to_string(),
            b.digest().value,
            a.digest().value
        ],
    )
    .unwrap();
    assert!(es.get(&e.digest().value).is_err());
    assert!(astra_seam(&f, &e, &a).is_err());
    assert!(astra_restart(&f).is_err());
}

#[test]
fn astra_s2_subject_tail_must_verify_the_middle_artifacts_content() {
    let f = admitted_mission("astra-middle-artifact");
    let (a, es, e) = astra_pair(&f);
    let b = astra_successor(&f, &a.digest().value, b"# Second\n");
    let c = astra_successor(&f, &b.digest().value, b"# Third\n");
    assert!(astra_restart(&f).is_ok());
    assert_eq!(
        es.get(&e.digest().value)
            .unwrap()
            .force()
            .subject_superseded_by(),
        Some(c.digest())
    );
    let conn = f.store.connect();
    let trigger = astra_allow_update(&conn, "artifacts_no_update");
    conn.execute(
        "UPDATE artifacts SET content_inline = x'00' WHERE digest_value = ?1",
        [&b.digest().value],
    )
    .unwrap();
    conn.execute_batch(&trigger).unwrap();
    assert!(f.artifacts().get(&b.digest().value).is_err());
    let read = es.get(&e.digest().value);
    let seam = astra_seam(&f, &e, &a);
    eprintln!(
        "S2 corrupt_middle={} tail={:?} seam={:?}",
        b.digest().value,
        read.as_ref().map(|x| x.force().subject_superseded_by()),
        seam
    );
    assert!(astra_restart(&f).is_err());
    assert!(
        read.is_err() && seam.is_err(),
        "a corrupt middle Artifact was skipped by the supposedly verified tail walk"
    );
}

#[test]
fn astra_s3_evaluation_tail_must_verify_the_middle_evaluation() {
    let f = admitted_mission("astra-middle-evaluation");
    let (a, es, first) = astra_pair(&f);
    let mut second_draft = f.artifact_evaluation_draft(&a.digest().value);
    second_draft.supersedes = EvaluationSupersedes::Evaluation(first.digest().value.clone());
    second_draft.rationale = "Second reading".to_owned();
    let second = es.record(second_draft).unwrap();
    let mut third_draft = f.artifact_evaluation_draft(&a.digest().value);
    third_draft.supersedes = EvaluationSupersedes::Evaluation(second.digest().value.clone());
    third_draft.rationale = "Third reading".to_owned();
    let third = es.record(third_draft).unwrap();
    assert!(astra_restart(&f).is_ok());
    assert_eq!(
        es.get(&first.digest().value)
            .unwrap()
            .force()
            .superseded_by(),
        Some(third.digest())
    );
    let conn = f.store.connect();
    let trigger = astra_allow_update(&conn, "evaluations_no_update");
    conn.execute(
        "UPDATE evaluations SET rationale = 'decoy' WHERE digest_value = ?1",
        [&second.digest().value],
    )
    .unwrap();
    conn.execute_batch(&trigger).unwrap();
    assert!(es.get(&second.digest().value).is_err());
    let read = es.get(&first.digest().value);
    let seam = astra_seam(&f, &first, &a);
    eprintln!(
        "S3 corrupt_middle={} tail={:?} seam={:?}",
        second.digest().value,
        read.as_ref().map(|x| x.force().superseded_by()),
        seam
    );
    assert!(astra_restart(&f).is_err());
    assert!(
        read.is_err() && seam.is_err(),
        "a corrupt middle Evaluation was skipped by the supposedly verified tail walk"
    );
}

#[test]
fn astra_control_mission_comes_from_arm03_and_not_its_projection() {
    let f = admitted_mission("astra-mission-arm");
    let other = astra_other_mission(&f);
    let (a, es, e) = astra_pair(&f);
    assert!(astra_seam(&f, &e, &a).is_ok());
    assert_eq!(
        publication_candidacy(
            &f.store.connect(),
            f.workspace_id,
            e.digest(),
            a.digest(),
            other
        ),
        Err(PublicationSeamRefusal::MissionDiffers)
    );
    let conn = f.store.connect();
    let trigger = astra_allow_update(&conn, "evaluations_no_update");
    conn.execute(
        "UPDATE evaluations SET mission_id = ?1 WHERE digest_value = ?2",
        rusqlite::params![other.to_string(), e.digest().value],
    )
    .unwrap();
    conn.execute_batch(&trigger).unwrap();
    assert!(es.get(&e.digest().value).is_err());
    assert!(publication_candidacy(&conn, f.workspace_id, e.digest(), a.digest(), other).is_err());
    assert!(astra_restart(&f).is_err());
}

#[test]
fn astra_control_forged_subject_event_is_refused_after_restart() {
    let f = admitted_mission("astra-artifact-event");
    let (a, es, e) = astra_pair(&f);
    assert!(astra_restart(&f).is_ok());
    assert!(astra_seam(&f, &e, &a).is_ok());
    let conn = f.store.connect();
    conn.execute(
        "UPDATE events SET payload_json = json_set(payload_json, '$.artifact.digest.canonicalizer', 'wrong.v1')
         WHERE kind = 'artifact.recorded' AND json_extract(payload_json, '$.artifact.digest.value') = ?1",
        [&a.digest().value],
    ).unwrap();
    assert!(es.get(&e.digest().value).is_err());
    assert!(astra_seam(&f, &e, &a).is_err());
    assert!(
        beacond::Collector::new(&conn, f.workspace_id)
            .artifact(&a.digest().value, f.mission_id)
            .is_err()
    );
    assert!(astra_restart(&f).is_err());
}

#[test]
fn astra_s4_reordered_subject_creation_cannot_follow_its_evaluation() {
    let f = admitted_mission("astra-reordered-artifact-event");
    let (a, es, e) = astra_pair(&f);
    assert!(astra_restart(&f).is_ok());
    let conn = f.store.connect();
    let eval_seq: i64 = conn
        .query_row(
            "SELECT sequence FROM events WHERE kind = 'evaluation.recorded'
         AND json_extract(payload_json, '$.evaluation.digest.value') = ?1",
            [&e.digest().value],
            |r| r.get(0),
        )
        .unwrap();
    assert!(a.creation_event_seq() < eval_seq as u64);
    let scratch: i64 = conn
        .query_row("SELECT MAX(sequence)+1 FROM events", [], |r| r.get(0))
        .unwrap();
    let moved = eval_seq;
    conn.execute(
        "UPDATE events SET sequence = ?1 WHERE sequence = ?2",
        rusqlite::params![scratch, eval_seq],
    )
    .unwrap();
    conn.execute(
        "UPDATE events SET sequence = ?1 WHERE sequence = ?2",
        rusqlite::params![moved, i64::try_from(a.creation_event_seq()).unwrap()],
    )
    .unwrap();
    conn.execute(
        "UPDATE events SET sequence = ?1 WHERE sequence = ?2",
        rusqlite::params![i64::try_from(a.creation_event_seq()).unwrap(), scratch],
    )
    .unwrap();
    // A mismatching cursor on its own is refused: positive detector control.
    assert!(es.get(&e.digest().value).is_err());
    let trigger = astra_allow_update(&conn, "artifacts_no_update");
    conn.execute(
        "UPDATE artifacts SET creation_event_seq = ?1 WHERE digest_value = ?2",
        rusqlite::params![moved, a.digest().value],
    )
    .unwrap();
    conn.execute_batch(&trigger).unwrap();
    let restart = astra_restart(&f);
    let read = es.get(&e.digest().value);
    let seam = astra_seam(&f, &e, &a);
    eprintln!(
        "S4 artifact_seq={moved} evaluation_seq={} restart={restart:?} read_ok={} seam={seam:?}",
        a.creation_event_seq(),
        read.is_ok()
    );
    assert!(
        restart.is_err() && read.is_err() && seam.is_err(),
        "an Evaluation was accepted even though its subject entered the ledger after it"
    );
}

#[test]
fn astra_control_collected_identity_is_not_a_meets_verdict() {
    let f = admitted_mission("astra-collected-not-meets");
    let (a, es, meets) = astra_pair(&f);
    let mut draft = f.artifact_evaluation_draft(&a.digest().value);
    draft.verdict = Verdict::Uncertain; // There is NO not_evaluated Evaluation verdict in v1.
    draft.rationale = "No definite assessment can be made.".to_owned();
    draft.supersedes = EvaluationSupersedes::Evaluation(meets.digest().value.clone());
    let uncertain = es.record(draft).unwrap();
    let conn = f.store.connect();
    let collected = beacond::Collector::new(&conn, f.workspace_id)
        .artifact(&a.digest().value, f.mission_id)
        .unwrap();
    assert_eq!(collected.digest_value, a.digest().value);
    assert_eq!(
        es.get(&uncertain.digest().value).unwrap().payload().verdict,
        Verdict::Uncertain
    );
    assert_eq!(
        astra_seam(&f, &uncertain, &a),
        Err(PublicationSeamRefusal::VerdictIsNotMeets)
    );
    // Historical Meets remains usable under K-2 §8; the later Evaluation is shown beside it.
    assert_eq!(
        es.get(&meets.digest().value)
            .unwrap()
            .force()
            .superseded_by(),
        Some(uncertain.digest())
    );
    assert!(astra_seam(&f, &meets, &a).is_ok());
}

#[test]
fn astra_s1_canonical_cross_mission_successor_is_not_a_verified_subject_tail() {
    let f = admitted_mission("astra-canonical-cross-mission");
    let other = astra_other_mission(&f);
    let (a, es, e) = astra_pair(&f);
    let b = f
        .artifacts()
        .record(
            ArtifactKind::OutcomeDocument,
            b"# Other Mission\n",
            ArtifactContext::Mission {
                project_id: f.project_id,
                mission_id: other,
            },
            f.resident_id,
            f.configuration_id,
            None,
        )
        .unwrap();
    assert!(astra_restart(&f).is_ok());
    // Build a byte-consistent restore image: B truthfully declares predecessor A but has another Mission.
    // The writer refuses WrongContext; this probe is exclusively about consumers reading persisted data.
    let conn = f.store.connect();
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    let mut bytes: Vec<u8> = conn
        .query_row(
            "SELECT canonical_bytes FROM artifacts WHERE digest_value = ?1",
            [&b.digest().value],
            |r| r.get(0),
        )
        .unwrap();
    let predecessor_tag = bytes.len() - 9;
    assert_eq!(bytes[predecessor_tag], 0x00);
    bytes[predecessor_tag] = 0x01;
    let predecessor: [u8; 32] = HEXLOWER
        .decode(a.digest().value.as_bytes())
        .unwrap()
        .try_into()
        .unwrap();
    bytes.splice(predecessor_tag + 1..predecessor_tag + 1, predecessor);
    let parsed = beacond::Artifact::from_canonical_bytes(&bytes).unwrap();
    assert_eq!(parsed.payload().supersedes, Some(predecessor));
    assert_eq!(
        parsed.payload().context,
        ArtifactContext::Mission {
            project_id: f.project_id,
            mission_id: other
        }
    );
    let replacement = parsed.digest().value.clone();
    let trigger = astra_allow_update(&conn, "artifacts_no_update");
    conn.execute(
        "UPDATE artifacts SET digest_value=?1, canonical_bytes=?2, has_predecessor=1, supersedes_digest=?3
         WHERE digest_value=?4",
        rusqlite::params![replacement, bytes, a.digest().value, b.digest().value],
    ).unwrap();
    conn.execute_batch(&trigger).unwrap();
    conn.execute(
        "UPDATE events SET payload_json=json_set(payload_json, '$.artifact.digest.value', ?1,
             '$.artifact.canonicalBytesBase64', ?2)
         WHERE kind='artifact.recorded' AND json_extract(payload_json, '$.artifact.digest.value')=?3",
        rusqlite::params![replacement, BASE64.encode(&bytes), b.digest().value],
    ).unwrap();
    conn.execute(
        "INSERT INTO artifact_supersessions (workspace_id, artifact_digest, supersedes_digest)
         VALUES (?1, ?2, ?3)",
        rusqlite::params![f.workspace_id.to_string(), replacement, a.digest().value],
    )
    .unwrap();
    // The canonical bytes, projections, Event and predecessor relation all agree.
    assert!(f.artifacts().get(&replacement).is_ok());
    let restart = astra_restart(&f);
    let read = es.get(&e.digest().value);
    let seam = astra_seam(&f, &e, &a);
    eprintln!(
        "S1b cross_mission={other} tail={:?} restart_ok={} seam={seam:?}",
        read.as_ref().map(|x| x.force().subject_superseded_by()),
        restart.is_ok()
    );
    assert!(
        read.is_err() && seam.is_err(),
        "the Evaluation reader blessed a cross-Mission subject chain after a byte-consistent restore"
    );
}
