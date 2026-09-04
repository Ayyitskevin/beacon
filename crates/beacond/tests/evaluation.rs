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
    AutonomyProfile, BeaconCapability, Conflicts, DeclaredCheck, EvaluationDraft, EvaluationError,
    EvaluationService, EvaluationSupersedes, Evaluator, Finding, FindingSeverity,
    InconsistentEvaluation, LocalModelActivationService, LocalModelEndpoint, MalformedEvaluation,
    MissionDraft, MissionMemberDraft, MissionService, OperatorAccountService, OperatorSessionPhase,
    ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyService,
    ProjectRepositoryService, ProjectService, ProposalService, ProviderFeature, RepositoryDraft,
    RepositorySourceKind, ResidentConfigurationService, ResidentLifecycleService,
    RubricDeclaration, SecretString, StarterOrganizationService, StarterResidentDraft, StarterRole,
    StartupConfig, Supervisor, TeamDraft, TeamLeadAmbiguityRule, TeamLeadAvailabilityRule,
    TeamLeadConflictRule, TeamLeadSelectionPolicy, TeamMemberDraft, TeamService,
    TrackRecordEvidenceKind, TrackRecordRequirement, UnmetPrecondition, Verdict,
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
        proposal_digest: proposal_digest.to_owned(),
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
    assert_eq!(evaluation.payload().proposal, *proposal.digest());
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
    assert_eq!(report.diagnostics.schema_version, 27);
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             ) VALUES (
                 ?1, 'sha256', 'beacon.evaluation.v1', ?2, ?3, ?4, 'sha256', 'beacon.proposal.v1',
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
            "UPDATE evaluations SET proposal_digest = ?1 WHERE digest_value = ?2",
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             ) VALUES (
                 ?1, 'sha256', 'beacon.evaluation.v1', ?2, ?3, ?4, 'sha256', 'beacon.proposal.v1',
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
            EvaluationError::Inconsistent(InconsistentEvaluation::SupersedesOtherProposal)
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
                 workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
                 workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
                 workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
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
                 workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
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
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, mission_id, evaluator_resident_id,
                 evaluator_configuration_id, rubric_id, rubric_version, rubric_canonicalizer,
                 rubric_sha256, verdict, conflicts_declared, conflicts_text, rationale,
                 evaluated_at_ms
             )
             SELECT workspace_id, digest_algorithm, digest_canonicalizer, ?1, ?2, proposal_digest,
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
