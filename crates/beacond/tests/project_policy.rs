use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use beacond::{
    AutonomyProfile, BeaconCapability, OperatorAccountService, OperatorSessionPhase, PolicyAuthor,
    ProjectCharterDraft, ProjectDraft, ProjectPolicyDraft, ProjectPolicyError,
    ProjectPolicyService, ProjectService, SecretString, StartupConfig, Supervisor, SupervisorError,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
use rusqlite::Connection;
use uuid::Uuid;

struct TestStore {
    path: PathBuf,
}

impl TestStore {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "beacond-project-policy-{label}-{}.sqlite3",
                Uuid::new_v4()
            )),
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

fn enrolled_services(
    label: &str,
) -> (
    TestStore,
    Arc<OperatorAccountService>,
    ProjectService,
    ProjectPolicyService,
) {
    let store = TestStore::new(label);
    let report = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Personal".to_owned(),
    })
    .unwrap();
    let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
    let recovery = accounts
        .begin_password_setup(
            SecretString::new("a long local passphrase".to_owned()),
            SecretString::new("a long local passphrase".to_owned()),
        )
        .unwrap();
    assert!(!recovery.expose().is_empty());
    assert_eq!(
        accounts.complete_operator_setup(true).unwrap().phase,
        OperatorSessionPhase::Unlocked
    );
    (
        store,
        Arc::clone(&accounts),
        ProjectService::new(Arc::clone(&accounts), report.workspace.id),
        ProjectPolicyService::new(accounts, report.workspace.id),
    )
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

/// A second Project, so the startup bar has two rows to tell apart (R39.3).
fn other_project_draft() -> ProjectDraft {
    ProjectDraft {
        name: "Beacon Beta".to_owned(),
        ..project_draft()
    }
}

fn policy_draft() -> ProjectPolicyDraft {
    ProjectPolicyDraft {
        team_lead_selection: TeamLeadSelectionPolicy {
            required_capabilities: vec![
                BeaconCapability::WorkCoordinateMission,
                BeaconCapability::EvidenceDevelop,
            ],
            availability_rule: TeamLeadAvailabilityRule::CurrentTeamEligibility,
            track_record_requirements: vec![
                TrackRecordRequirement {
                    evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
                    minimum_count: 1,
                },
                TrackRecordRequirement {
                    evidence_kind: TrackRecordEvidenceKind::Evaluation,
                    minimum_count: 2,
                },
            ],
            conflict_rule: TeamLeadConflictRule::RequireNoDeclaredConflict,
            ambiguity_rule: TeamLeadAmbiguityRule::RequireOperatorDecision,
        },
    }
}

#[test]
fn operator_publishes_first_policy_revision_bound_to_current_charter() {
    let (_store, _accounts, projects, policies) = enrolled_services("publish");
    let project = projects.create(project_draft()).unwrap();

    let policy = policies
        .publish(project.id, project.charter.revision, None, policy_draft())
        .unwrap();

    assert_eq!(policy.workspace_id, project.workspace_id);
    assert_eq!(policy.project_id, project.id);
    assert_eq!(policy.revision, 1);
    assert_eq!(policy.charter_revision, project.charter.revision);
    assert_eq!(policy.authored_by, PolicyAuthor::Operator);
    assert!(policy.applies_to_current_charter);
    assert_eq!(
        policy.team_lead_selection,
        policy_draft().team_lead_selection
    );
    assert_eq!(policies.current(project.id).unwrap(), Some(policy.clone()));
    assert_eq!(policies.history(project.id).unwrap(), vec![policy]);
}

#[test]
fn policy_commands_authenticate_before_reading_or_validating_arguments() {
    let (_store, accounts, projects, policies) = enrolled_services("locked");
    let project = projects.create(project_draft()).unwrap();
    accounts.lock().unwrap();

    let mut malformed = policy_draft();
    malformed.team_lead_selection.required_capabilities.clear();
    assert!(matches!(
        policies.current(Uuid::nil()),
        Err(ProjectPolicyError::Account(_))
    ));
    assert!(matches!(
        policies.history(Uuid::nil()),
        Err(ProjectPolicyError::Account(_))
    ));
    assert!(matches!(
        policies.publish(project.id, 0, None, malformed),
        Err(ProjectPolicyError::Account(_))
    ));
}

#[test]
fn invalid_or_stale_policy_drafts_are_refused_without_partial_state() {
    let (store, _accounts, projects, policies) = enrolled_services("refusals");
    let project = projects.create(project_draft()).unwrap();

    let mut missing_coordination = policy_draft();
    missing_coordination
        .team_lead_selection
        .required_capabilities = vec![BeaconCapability::EvidenceDevelop];
    assert!(matches!(
        policies.publish(project.id, 1, None, missing_coordination),
        Err(ProjectPolicyError::InvalidRequiredCapabilities)
    ));

    let mut duplicate_capability = policy_draft();
    duplicate_capability
        .team_lead_selection
        .required_capabilities
        .push(BeaconCapability::WorkCoordinateMission);
    assert!(matches!(
        policies.publish(project.id, 1, None, duplicate_capability),
        Err(ProjectPolicyError::InvalidRequiredCapabilities)
    ));

    let mut no_evidence = policy_draft();
    no_evidence
        .team_lead_selection
        .track_record_requirements
        .clear();
    assert!(matches!(
        policies.publish(project.id, 1, None, no_evidence),
        Err(ProjectPolicyError::InvalidTrackRecordRequirements)
    ));

    let mut zero_threshold = policy_draft();
    zero_threshold.team_lead_selection.track_record_requirements[0].minimum_count = 0;
    assert!(matches!(
        policies.publish(project.id, 1, None, zero_threshold),
        Err(ProjectPolicyError::InvalidTrackRecordRequirements)
    ));

    let mut duplicate_evidence = policy_draft();
    duplicate_evidence
        .team_lead_selection
        .track_record_requirements
        .push(TrackRecordRequirement {
            evidence_kind: TrackRecordEvidenceKind::MissionOutcome,
            minimum_count: 3,
        });
    assert!(matches!(
        policies.publish(project.id, 1, None, duplicate_evidence),
        Err(ProjectPolicyError::InvalidTrackRecordRequirements)
    ));

    assert!(matches!(
        policies.publish(Uuid::new_v4(), 1, None, policy_draft()),
        Err(ProjectPolicyError::ProjectNotFound)
    ));
    assert!(policies.current(project.id).unwrap().is_none());
    assert!(policies.history(project.id).unwrap().is_empty());
    let connection = Connection::open(&store.path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM events WHERE kind = 'project.policy_published'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn policy_revision_is_stale_safe_and_charter_changes_require_republication() {
    let (_store, _accounts, projects, policies) = enrolled_services("revision");
    let project = projects.create(project_draft()).unwrap();
    let first = policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();

    let revised_project = projects
        .revise_charter(
            project.id,
            1,
            ProjectCharterDraft {
                purpose: "Build a trustworthy local control plane.".to_owned(),
                ..project_draft().charter
            },
        )
        .unwrap();
    assert_eq!(revised_project.charter.revision, 2);
    assert!(
        !policies
            .current(project.id)
            .unwrap()
            .unwrap()
            .applies_to_current_charter
    );
    assert!(matches!(
        policies.publish(project.id, 1, Some(1), policy_draft()),
        Err(ProjectPolicyError::StaleCharterRevision {
            expected: 1,
            current: 2
        })
    ));

    let second = policies
        .publish(project.id, 2, Some(first.revision), policy_draft())
        .unwrap();
    assert_eq!(second.revision, 2);
    assert_eq!(second.charter_revision, 2);
    assert!(second.applies_to_current_charter);
    assert!(matches!(
        policies.publish(project.id, 2, Some(1), policy_draft()),
        Err(ProjectPolicyError::StalePolicyRevision)
    ));
    assert!(matches!(
        policies.publish(project.id, 2, Some(2), policy_draft()),
        Err(ProjectPolicyError::UnchangedPolicy)
    ));
    assert_eq!(policies.history(project.id).unwrap().len(), 2);
}

#[test]
fn concurrent_publications_from_the_same_revision_commit_exactly_once() {
    let (_store, accounts, projects, first_service) = enrolled_services("concurrent");
    let project = projects.create(project_draft()).unwrap();
    let second_service = ProjectPolicyService::new(Arc::clone(&accounts), project.workspace_id);
    let barrier = Arc::new(Barrier::new(3));

    let first_barrier = Arc::clone(&barrier);
    let project_id = project.id;
    let first = thread::spawn(move || {
        first_barrier.wait();
        first_service.publish(project_id, 1, None, policy_draft())
    });
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        second_service.publish(project_id, 1, None, policy_draft())
    });

    barrier.wait();
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(ProjectPolicyError::StalePolicyRevision)))
            .count(),
        1
    );
    let policies = ProjectPolicyService::new(accounts, project.workspace_id);
    assert_eq!(policies.history(project.id).unwrap().len(), 1);
}

#[test]
fn restart_refuses_policy_state_without_its_exact_event() {
    // R44: BOTH ENDS. Tampering only the last-sorting Project made "names the failing row"
    // and "names the last row" one assertion, and a Row arm naming `rows.last()` survived.
    for (end, order) in [("last", "DESC"), ("first", "ASC")] {
        let (store, accounts, projects, policies) =
            enrolled_services(&format!("missing-event-{end}"));
        // R39.3 (kimi's HOLD): TWO Projects carry policies, and the tamper hits whichever sorts LAST
        // under this pass's own `ORDER BY workspace_id, project_id`. The old fixture had ONE Project,
        // so `!project_id.is_nil()` was all the bar could assert -- naming ANY real Project satisfied
        // it, and a mutant naming `rows[0]` was unkillable by construction.
        let first = projects.create(project_draft()).unwrap();
        let second = projects.create(other_project_draft()).unwrap();
        for project in [&first, &second] {
            policies
                .publish(project.id, 1, None, policy_draft())
                .unwrap();
        }
        drop(policies);
        drop(projects);
        drop(accounts);
        let connection = Connection::open(&store.path).unwrap();
        // The pass's own order, read back from the store rather than assumed from the ids.
        let tampered: String = connection
            .query_row(
                &format!(
                    "SELECT DISTINCT project_id FROM project_policy_revisions
                 ORDER BY workspace_id {order}, project_id {order} LIMIT 1"
                ),
                [],
                |row| row.get(0),
            )
            .expect("both Projects have policy revisions");
        let removed = connection
            .execute(
                "DELETE FROM events
             WHERE kind = 'project.policy_published'
               AND payload_json LIKE '%' || ?1 || '%'",
                rusqlite::params![tampered],
            )
            .unwrap();
        assert_eq!(removed, 1, "the plant removed the wrong number of Events");
        drop(connection);

        let result = Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        });
        assert!(
            matches!(
                result,
                Err(SupervisorError::ProjectPolicyStateMismatch { .. })
            ),
            "unexpected bootstrap result: {result:?}"
        );
        // R28.1: the refusal names WHICH Project, and R28.4's line is the key plus the fault's NAME
        // with the reader's own sentence absent.
        let refusal = result.unwrap_err();
        assert!(
            matches!(
                &refusal,
                SupervisorError::ProjectPolicyStateMismatch { project_id, .. }
                    if project_id.to_string() == tampered
            ),
            "the refusal does not name the Project it refused: {refusal:?}"
        );
        assert!(
            refusal.to_string().contains(&tampered),
            "the line does not name the Project it refused: {refusal}"
        );
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
    }
}

#[test]
fn restart_refuses_a_policy_event_whose_evidence_no_longer_matches_state() {
    let (store, accounts, projects, policies) = enrolled_services("event-mismatch");
    let project = projects.create(project_draft()).unwrap();
    policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();
    drop(policies);
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute(
            "UPDATE events
             SET payload_json = json_set(payload_json, '$.policy.teamLeadSelection.trackRecordRequirements[0].minimumCount', 99)
             WHERE kind = 'project.policy_published'",
            [],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Supervisor::bootstrap(StartupConfig {
            store_path: store.path.clone(),
            initial_workspace_name: "Ignored".to_owned(),
        }),
        Err(SupervisorError::ProjectPolicyEventMismatch(_))
    ));
}

#[test]
fn restart_refuses_noncanonical_project_policy_state() {
    let (store, accounts, projects, policies) = enrolled_services("noncanonical-state");
    let project = projects.create(project_draft()).unwrap();
    policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();
    drop(policies);
    drop(projects);
    drop(accounts);
    let connection = Connection::open(&store.path).unwrap();
    connection
        .execute_batch("DROP TRIGGER project_policy_revisions_no_update")
        .unwrap();
    connection
        .execute(
            "UPDATE project_policy_revisions
             SET required_capabilities_json = '[\"evidence.develop\",\"work.coordinate_mission\"]'",
            [],
        )
        .unwrap();
    connection
        .execute_batch(concat!(
            "CREATE TRIGGER project_policy_revisions_no_update\n",
            "BEFORE UPDATE ON project_policy_revisions\n",
            "BEGIN\n",
            "    SELECT RAISE(ABORT, 'Project Policy revisions are immutable');\n",
            "END;"
        ))
        .unwrap();
    drop(connection);

    let result = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    });
    assert!(
        matches!(result, Err(SupervisorError::ProjectPolicyEventMismatch(_))),
        "unexpected bootstrap result: {result:?}"
    );
}

#[test]
fn only_the_latest_policy_revision_can_apply_to_the_current_charter() {
    let (_store, _accounts, projects, policies) = enrolled_services("current-revision");
    let project = projects.create(project_draft()).unwrap();
    policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();
    let mut revised = policy_draft();
    revised.team_lead_selection.track_record_requirements[0].minimum_count = 3;
    policies.publish(project.id, 1, Some(1), revised).unwrap();

    let history = policies.history(project.id).unwrap();
    assert_eq!(history.len(), 2);
    assert!(!history[0].applies_to_current_charter);
    assert!(history[1].applies_to_current_charter);
}

/// sol1 cold-read probe: the policy relation's own stored key includes its integer revision.
/// A revision outside u32 is reachable through SQL and refused by the reader, so R39.1 requires
/// the refusal to identify that stored revision instead of only the Project containing it.
#[test]
fn sol1_policy_startup_names_out_of_domain_stored_revision() {
    let (store, _accounts, projects, policies) = enrolled_services("sol1-version-edge");
    let project = projects.create(project_draft()).unwrap();
    policies
        .publish(project.id, 1, None, policy_draft())
        .unwrap();
    drop(policies);
    drop(projects);

    let connection = Connection::open(&store.path).unwrap();
    let columns: Vec<String> = connection
        .prepare("SELECT name FROM pragma_table_info('project_policy_revisions')")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let names = columns.join(", ");
    let values = columns
        .iter()
        .map(|column| {
            if column == "revision" {
                "?1".to_owned()
            } else {
                column.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!(
        "INSERT INTO project_policy_revisions ({names})
         SELECT {values} FROM project_policy_revisions WHERE project_id = ?2 AND revision = 1"
    );
    for rejected in [0_i64, -1, i64::MIN] {
        let error = connection
            .execute(&insert, rusqlite::params![rejected, project.id.to_string()])
            .unwrap_err();
        assert!(
            error.to_string().contains("CHECK constraint failed"),
            "revision {rejected} escaped the schema or failed under the wrong guard: {error}"
        );
    }
    let stored = i64::from(u32::MAX) + 1;
    let inserted = connection
        .execute(&insert, rusqlite::params![stored, project.id.to_string()])
        .unwrap();
    assert_eq!(inserted, 1, "the u32::MAX+1 revision did not plant");
    drop(connection);

    let error = Supervisor::bootstrap(StartupConfig {
        store_path: store.path.clone(),
        initial_workspace_name: "Ignored".to_owned(),
    })
    .expect_err("an out-of-domain policy revision reopened Ready");
    let line = error.to_string();
    assert!(
        line.contains(&stored.to_string()),
        "the refusal names only the Project, not stored revision {stored}: {line}"
    );
}
