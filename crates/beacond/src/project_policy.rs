use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::current_charter_revision;
use crate::{BeaconCapability, OperatorAccountError, OperatorAccountService, ProjectError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackRecordEvidenceKind {
    MissionOutcome,
    Evaluation,
    PolicyCompliance,
    Spend,
    Receipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrackRecordRequirement {
    pub evidence_kind: TrackRecordEvidenceKind,
    pub minimum_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamLeadAvailabilityRule {
    CurrentTeamEligibility,
}

impl TeamLeadAvailabilityRule {
    fn as_str(self) -> &'static str {
        match self {
            Self::CurrentTeamEligibility => "current_team_eligibility",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectPolicyError> {
        match value {
            "current_team_eligibility" => Ok(Self::CurrentTeamEligibility),
            _ => Err(ProjectPolicyError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamLeadConflictRule {
    RequireNoDeclaredConflict,
}

impl TeamLeadConflictRule {
    fn as_str(self) -> &'static str {
        match self {
            Self::RequireNoDeclaredConflict => "require_no_declared_conflict",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectPolicyError> {
        match value {
            "require_no_declared_conflict" => Ok(Self::RequireNoDeclaredConflict),
            _ => Err(ProjectPolicyError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamLeadAmbiguityRule {
    RequireOperatorDecision,
}

impl TeamLeadAmbiguityRule {
    fn as_str(self) -> &'static str {
        match self {
            Self::RequireOperatorDecision => "require_operator_decision",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectPolicyError> {
        match value {
            "require_operator_decision" => Ok(Self::RequireOperatorDecision),
            _ => Err(ProjectPolicyError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TeamLeadSelectionPolicy {
    pub required_capabilities: Vec<BeaconCapability>,
    pub availability_rule: TeamLeadAvailabilityRule,
    pub track_record_requirements: Vec<TrackRecordRequirement>,
    pub conflict_rule: TeamLeadConflictRule,
    pub ambiguity_rule: TeamLeadAmbiguityRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectPolicyDraft {
    pub team_lead_selection: TeamLeadSelectionPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAuthor {
    Operator,
}

impl PolicyAuthor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectPolicyError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(ProjectPolicyError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectPolicy {
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub revision: u32,
    pub charter_revision: u32,
    pub team_lead_selection: TeamLeadSelectionPolicy,
    pub authored_by: PolicyAuthor,
    pub authored_at_ms: u64,
    pub applies_to_current_charter: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredProjectPolicy {
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
    charter_revision: u32,
    team_lead_selection: TeamLeadSelectionPolicy,
    authored_by: PolicyAuthor,
    authored_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectPolicyPublishedPayload {
    policy: StoredProjectPolicy,
}

/// Which fault the ProjectPolicy startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum ProjectPolicyStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One ProjectPolicy did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        project_id: Uuid,
        cause: ProjectPolicyError,
    },
    // NO `Totals` ARM. Like the repository pass, this one walks the Projects that have policy
    // revisions and checks each one's own history and Events; no whole-store count is left over.
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and
    /// the Workspace pass run first.
    Identity {
        workspace_id: String,
        project_id: String,
    },
    /// A stored revision outside the positive `u32` domain written by the service.
    Revision {
        workspace_id: String,
        project_id: String,
        revision: i64,
    },
}

impl ProjectPolicyError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ProjectNotFound => "ProjectNotFound",
            Self::StaleCharterRevision { .. } => "StaleCharterRevision",
            Self::StalePolicyRevision => "StalePolicyRevision",
            Self::InvalidRequiredCapabilities => "InvalidRequiredCapabilities",
            Self::InvalidTrackRecordRequirements => "InvalidTrackRecordRequirements",
            Self::UnchangedPolicy => "UnchangedPolicy",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for ProjectPolicyStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the ProjectPolicys could not be read"),
            Self::Row {
                workspace_id,
                project_id,
                cause,
            } => write!(
                formatter,
                "ProjectPolicy {project_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                project_id,
            } => write!(
                formatter,
                "a ProjectPolicy row's Workspace is not readable: {workspace_id}, ProjectPolicy \
                 {project_id}"
            ),
            Self::Revision { .. } => {
                formatter.write_str("a ProjectPolicy revision is not readable")
            }
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ProjectPolicyStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                project_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("project_id", project_id)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                project_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("project_id", project_id)
                .finish(),
            Self::Revision {
                workspace_id,
                project_id,
                revision,
            } => formatter
                .debug_struct("Revision")
                .field("workspace_id", workspace_id)
                .field("project_id", project_id)
                .field("revision", revision)
                .finish(),
        }
    }
}

impl std::error::Error for ProjectPolicyStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } | Self::Revision { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ProjectPolicyError {
    Account(OperatorAccountError),
    ProjectNotFound,
    StaleCharterRevision { expected: u32, current: u32 },
    StalePolicyRevision,
    InvalidRequiredCapabilities,
    InvalidTrackRecordRequirements,
    UnchangedPolicy,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ProjectPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ProjectNotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::StaleCharterRevision { expected, current } => write!(
                formatter,
                "Project Charter changed from revision {expected} to {current} before Policy publication"
            ),
            Self::StalePolicyRevision => write!(formatter, "Project Policy revision is stale"),
            Self::InvalidRequiredCapabilities => write!(
                formatter,
                "Team Lead Policy must name distinct capabilities including work.coordinate_mission"
            ),
            Self::InvalidTrackRecordRequirements => write!(
                formatter,
                "Team Lead Policy must name distinct positive Track Record evidence thresholds"
            ),
            Self::UnchangedPolicy => write!(formatter, "Project Policy is unchanged"),
            Self::InvalidStore => write!(formatter, "Project Policy state is invalid"),
            Self::Store(error) => write!(formatter, "Project Policy store failed: {error}"),
            Self::Json(error) => write!(formatter, "Project Policy event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ProjectPolicyError {}

impl From<OperatorAccountError> for ProjectPolicyError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ProjectPolicyError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ProjectPolicyError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns authenticated, immutable Project Policy publication and history queries.
pub struct ProjectPolicyService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ProjectPolicyService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn current(&self, project_id: Uuid) -> Result<Option<ProjectPolicy>, ProjectPolicyError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let current_charter = require_project(connection, self.workspace_id, project_id)?;
            latest_stored_policy(connection, self.workspace_id, project_id)?
                .map(|policy| render_policy(policy, current_charter, true))
                .transpose()
        })
    }

    pub fn history(&self, project_id: Uuid) -> Result<Vec<ProjectPolicy>, ProjectPolicyError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let current_charter = require_project(connection, self.workspace_id, project_id)?;
            let policies = load_policy_history(connection, self.workspace_id, project_id)?;
            let latest_revision = policies.last().map(|policy| policy.revision);
            policies
                .into_iter()
                .map(|policy| {
                    let is_latest = Some(policy.revision) == latest_revision;
                    render_policy(policy, current_charter, is_latest)
                })
                .collect()
        })
    }

    pub fn publish(
        &self,
        project_id: Uuid,
        expected_charter_revision: u32,
        expected_policy_revision: Option<u32>,
        mut draft: ProjectPolicyDraft,
    ) -> Result<ProjectPolicy, ProjectPolicyError> {
        self.accounts.with_authenticated_store(|connection, now| {
            normalize_selection(&mut draft.team_lead_selection)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current_charter = require_project(&transaction, self.workspace_id, project_id)?;
            if current_charter != expected_charter_revision {
                return Err(ProjectPolicyError::StaleCharterRevision {
                    expected: expected_charter_revision,
                    current: current_charter,
                });
            }
            let previous = latest_stored_policy(&transaction, self.workspace_id, project_id)?;
            if previous.as_ref().map(|policy| policy.revision) != expected_policy_revision {
                return Err(ProjectPolicyError::StalePolicyRevision);
            }
            if previous.as_ref().is_some_and(|policy| {
                policy.charter_revision == current_charter
                    && policy.team_lead_selection == draft.team_lead_selection
            }) {
                return Err(ProjectPolicyError::UnchangedPolicy);
            }
            let revision = match previous.as_ref() {
                Some(policy) => policy
                    .revision
                    .checked_add(1)
                    .ok_or(ProjectPolicyError::InvalidStore)?,
                None => 1,
            };
            let authored_at_ms = monotonic_time(now, previous.as_ref())?;
            let policy = StoredProjectPolicy {
                workspace_id: self.workspace_id,
                project_id,
                revision,
                charter_revision: current_charter,
                team_lead_selection: draft.team_lead_selection,
                authored_by: PolicyAuthor::Operator,
                authored_at_ms,
            };
            insert_policy(&transaction, &policy)?;
            let view = render_policy(policy, current_charter, true)?;
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn normalize_selection(selection: &mut TeamLeadSelectionPolicy) -> Result<(), ProjectPolicyError> {
    if selection.required_capabilities.is_empty()
        || selection.required_capabilities.len() > BeaconCapability::ALL.len()
    {
        return Err(ProjectPolicyError::InvalidRequiredCapabilities);
    }
    let original_capability_count = selection.required_capabilities.len();
    selection.required_capabilities.sort_unstable();
    selection.required_capabilities.dedup();
    if selection.required_capabilities.len() != original_capability_count
        || !selection
            .required_capabilities
            .contains(&BeaconCapability::WorkCoordinateMission)
    {
        return Err(ProjectPolicyError::InvalidRequiredCapabilities);
    }

    if selection.track_record_requirements.is_empty() {
        return Err(ProjectPolicyError::InvalidTrackRecordRequirements);
    }
    let mut evidence_kinds = HashSet::with_capacity(selection.track_record_requirements.len());
    if selection
        .track_record_requirements
        .iter()
        .any(|requirement| {
            requirement.minimum_count == 0 || !evidence_kinds.insert(requirement.evidence_kind)
        })
    {
        return Err(ProjectPolicyError::InvalidTrackRecordRequirements);
    }
    selection
        .track_record_requirements
        .sort_unstable_by_key(|requirement| requirement.evidence_kind);
    Ok(())
}

fn require_project(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<u32, ProjectPolicyError> {
    current_charter_revision(connection, workspace_id, project_id)
        .map_err(map_project_error)?
        .ok_or(ProjectPolicyError::ProjectNotFound)
}

fn insert_policy(
    connection: &Connection,
    policy: &StoredProjectPolicy,
) -> Result<(), ProjectPolicyError> {
    connection.execute(
        "INSERT INTO project_policy_revisions (
             workspace_id, project_id, revision, charter_revision,
             required_capabilities_json, availability_rule,
             track_record_requirements_json, conflict_rule, ambiguity_rule,
             authored_by, authored_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            policy.workspace_id.to_string(),
            policy.project_id.to_string(),
            i64::from(policy.revision),
            i64::from(policy.charter_revision),
            serde_json::to_string(&policy.team_lead_selection.required_capabilities)?,
            policy.team_lead_selection.availability_rule.as_str(),
            serde_json::to_string(&policy.team_lead_selection.track_record_requirements)?,
            policy.team_lead_selection.conflict_rule.as_str(),
            policy.team_lead_selection.ambiguity_rule.as_str(),
            policy.authored_by.as_str(),
            i64::try_from(policy.authored_at_ms).map_err(|_| ProjectPolicyError::InvalidStore)?,
        ],
    )?;
    let payload = serde_json::to_string(&ProjectPolicyPublishedPayload {
        policy: policy.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'project.policy_published', ?2, ?3)",
        params![
            policy.workspace_id.to_string(),
            payload,
            i64::try_from(policy.authored_at_ms).map_err(|_| ProjectPolicyError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn latest_stored_policy(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<StoredProjectPolicy>, ProjectPolicyError> {
    let revision = connection
        .query_row(
            "SELECT max(revision) FROM project_policy_revisions
             WHERE workspace_id = ?1 AND project_id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, Option<i64>>(0),
        )?
        .map(positive_u32)
        .transpose()?;
    revision
        .map(|revision| load_stored_policy(connection, workspace_id, project_id, revision))
        .transpose()?
        .flatten()
        .map_or(Ok(None), |policy| Ok(Some(policy)))
}

fn load_policy_history(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<StoredProjectPolicy>, ProjectPolicyError> {
    let mut statement = connection.prepare(
        "SELECT revision FROM project_policy_revisions
         WHERE workspace_id = ?1 AND project_id = ?2 ORDER BY revision",
    )?;
    let revisions = statement
        .query_map(
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    revisions
        .into_iter()
        .map(|revision| {
            let revision = positive_u32(revision)?;
            load_stored_policy(connection, workspace_id, project_id, revision)?
                .ok_or(ProjectPolicyError::InvalidStore)
        })
        .collect()
}

fn load_stored_policy(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
) -> Result<Option<StoredProjectPolicy>, ProjectPolicyError> {
    let row = connection
        .query_row(
            "SELECT workspace_id, project_id, revision, charter_revision,
                    required_capabilities_json, availability_rule,
                    track_record_requirements_json, conflict_rule, ambiguity_rule,
                    authored_by, authored_at_ms
             FROM project_policy_revisions
             WHERE workspace_id = ?1 AND project_id = ?2 AND revision = ?3",
            params![
                workspace_id.to_string(),
                project_id.to_string(),
                i64::from(revision)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()?;
    let Some((
        stored_workspace_id,
        stored_project_id,
        stored_revision,
        charter_revision,
        required_capabilities_json,
        availability_rule,
        track_record_requirements_json,
        conflict_rule,
        ambiguity_rule,
        authored_by,
        authored_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let mut selection = TeamLeadSelectionPolicy {
        required_capabilities: serde_json::from_str(&required_capabilities_json)?,
        availability_rule: TeamLeadAvailabilityRule::parse(&availability_rule)?,
        track_record_requirements: serde_json::from_str(&track_record_requirements_json)?,
        conflict_rule: TeamLeadConflictRule::parse(&conflict_rule)?,
        ambiguity_rule: TeamLeadAmbiguityRule::parse(&ambiguity_rule)?,
    };
    let before = selection.clone();
    normalize_selection(&mut selection).map_err(|_| ProjectPolicyError::InvalidStore)?;
    if selection != before {
        return Err(ProjectPolicyError::InvalidStore);
    }
    Ok(Some(StoredProjectPolicy {
        workspace_id: parse_uuid(stored_workspace_id)?,
        project_id: parse_uuid(stored_project_id)?,
        revision: positive_u32(stored_revision)?,
        charter_revision: positive_u32(charter_revision)?,
        team_lead_selection: selection,
        authored_by: PolicyAuthor::parse(&authored_by)?,
        authored_at_ms: nonnegative_u64(authored_at_ms)?,
    }))
}

fn render_policy(
    policy: StoredProjectPolicy,
    current_charter_revision: u32,
    is_latest: bool,
) -> Result<ProjectPolicy, ProjectPolicyError> {
    Ok(ProjectPolicy {
        workspace_id: policy.workspace_id,
        project_id: policy.project_id,
        revision: policy.revision,
        charter_revision: policy.charter_revision,
        team_lead_selection: policy.team_lead_selection,
        authored_by: policy.authored_by,
        authored_at_ms: policy.authored_at_ms,
        applies_to_current_charter: is_latest
            && policy.charter_revision == current_charter_revision,
    })
}

fn monotonic_time(
    now: i64,
    previous: Option<&StoredProjectPolicy>,
) -> Result<u64, ProjectPolicyError> {
    let now = nonnegative_u64(now)?;
    previous.map_or(Ok(now), |policy| {
        Ok(now.max(
            policy
                .authored_at_ms
                .checked_add(1)
                .ok_or(ProjectPolicyError::InvalidStore)?,
        ))
    })
}

/// What one stored Policy revision tells a caller that binds to it: the exact Charter
/// revision it interprets, and whether it is still this Project's terminal revision.
/// `None` when that Policy revision does not exist in this Project.
///
/// Binding validity and staleness are different questions — the first is decided
/// against the named immutable revision, the second is derived at read time — so this
/// reader answers both without letting a caller confuse them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PolicyRevisionFacts {
    pub(crate) charter_revision: u32,
    pub(crate) is_current: bool,
}

pub(crate) fn policy_revision_facts(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
) -> Result<Option<PolicyRevisionFacts>, ProjectPolicyError> {
    let Some(charter_revision) = connection
        .query_row(
            "SELECT charter_revision FROM project_policy_revisions
             WHERE workspace_id = ?1 AND project_id = ?2 AND revision = ?3",
            params![
                workspace_id.to_string(),
                project_id.to_string(),
                i64::from(revision)
            ],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let latest = connection.query_row(
        "SELECT max(revision) FROM project_policy_revisions
         WHERE workspace_id = ?1 AND project_id = ?2",
        params![workspace_id.to_string(), project_id.to_string()],
        |row| row.get::<_, Option<i64>>(0),
    )?;
    Ok(Some(PolicyRevisionFacts {
        charter_revision: positive_u32(charter_revision)?,
        is_current: latest == Some(i64::from(revision)),
    }))
}

/// The rows `verify_project_policy_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_policy_projects(
    connection: &Connection,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT workspace_id, project_id FROM project_policy_revisions
         ORDER BY workspace_id, project_id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_project_policy_state(
    connection: &Connection,
) -> Result<(), ProjectPolicyStateFault> {
    let invalid_revision = connection
        .query_row(
            "SELECT workspace_id, project_id, revision
             FROM project_policy_revisions
             WHERE revision < 1 OR revision > ?1
             ORDER BY workspace_id, project_id, revision
             LIMIT 1",
            [i64::from(u32::MAX)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(ProjectPolicyStateFault::Store)?;
    if let Some((workspace_id, project_id, revision)) = invalid_revision {
        return Err(ProjectPolicyStateFault::Revision {
            workspace_id,
            project_id,
            revision,
        });
    }

    let projects = enumerate_policy_projects(connection).map_err(ProjectPolicyStateFault::Store)?;
    for (workspace_id, project_id) in projects {
        let identity = || ProjectPolicyStateFault::Identity {
            workspace_id: workspace_id.clone(),
            project_id: project_id.clone(),
        };
        let parsed_workspace = Uuid::parse_str(&workspace_id).map_err(|_| identity())?;
        let parsed_project = Uuid::parse_str(&project_id).map_err(|_| identity())?;
        verify_one_project_policy(connection, parsed_workspace, parsed_project).map_err(
            |cause| ProjectPolicyStateFault::Row {
                workspace_id: parsed_workspace,
                project_id: parsed_project,
                cause,
            },
        )?;
    }
    Ok(())
}

/// One Project's policy history, re-proved. Unchanged from what the loop held inline.
fn verify_one_project_policy(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(), ProjectPolicyError> {
    {
        let policies = load_policy_history(connection, workspace_id, project_id)?;
        if policies.iter().enumerate().any(|(index, policy)| {
            usize::try_from(policy.revision) != Ok(index + 1)
                || index > 0 && policies[index - 1].authored_at_ms > policy.authored_at_ms
        }) {
            return Err(ProjectPolicyError::InvalidStore);
        }
        for policy in policies {
            let event_count: i64 = connection.query_row(
                "SELECT count(*) FROM events
                 WHERE workspace_id = ?1 AND kind = 'project.policy_published'
                   AND json_extract(payload_json, '$.policy.projectId') = ?2
                   AND json_extract(payload_json, '$.policy.revision') = ?3",
                params![
                    workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(policy.revision)
                ],
                |row| row.get(0),
            )?;
            if event_count != 1 {
                return Err(ProjectPolicyError::InvalidStore);
            }
        }
    }
    Ok(())
}

pub(crate) fn verify_project_policy_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ProjectPolicyError> {
    let payload: ProjectPolicyPublishedPayload = serde_json::from_str(payload_json)?;
    let policy = payload.policy;
    if policy.workspace_id != workspace_id || policy.authored_at_ms != occurred_at_ms {
        return Err(ProjectPolicyError::InvalidStore);
    }
    let stored = load_stored_policy(connection, workspace_id, policy.project_id, policy.revision)?
        .ok_or(ProjectPolicyError::InvalidStore)?;
    if stored != policy {
        return Err(ProjectPolicyError::InvalidStore);
    }
    Ok(())
}

fn map_project_error(error: ProjectError) -> ProjectPolicyError {
    match error {
        ProjectError::Store(error) => ProjectPolicyError::Store(error),
        ProjectError::Json(error) => ProjectPolicyError::Json(error),
        _ => ProjectPolicyError::InvalidStore,
    }
}

fn parse_uuid(value: String) -> Result<Uuid, ProjectPolicyError> {
    Uuid::parse_str(&value).map_err(|_| ProjectPolicyError::InvalidStore)
}

fn positive_u32(value: i64) -> Result<u32, ProjectPolicyError> {
    let value = u32::try_from(value).map_err(|_| ProjectPolicyError::InvalidStore)?;
    if value == 0 {
        return Err(ProjectPolicyError::InvalidStore);
    }
    Ok(value)
}

fn nonnegative_u64(value: i64) -> Result<u64, ProjectPolicyError> {
    u64::try_from(value).map_err(|_| ProjectPolicyError::InvalidStore)
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &ProjectPolicyError) -> &'static str {
        match error {
            ProjectPolicyError::Account(_) => "Account",
            ProjectPolicyError::ProjectNotFound => "ProjectNotFound",
            ProjectPolicyError::StaleCharterRevision { .. } => "StaleCharterRevision",
            ProjectPolicyError::StalePolicyRevision => "StalePolicyRevision",
            ProjectPolicyError::InvalidRequiredCapabilities => "InvalidRequiredCapabilities",
            ProjectPolicyError::InvalidTrackRecordRequirements => "InvalidTrackRecordRequirements",
            ProjectPolicyError::UnchangedPolicy => "UnchangedPolicy",
            ProjectPolicyError::InvalidStore => "InvalidStore",
            ProjectPolicyError::Store(_) => "Store",
            ProjectPolicyError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_project_policy_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            ProjectPolicyError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            ProjectPolicyError::ProjectNotFound,
            ProjectPolicyError::StaleCharterRevision {
                expected: 1,
                current: 2,
            },
            ProjectPolicyError::StalePolicyRevision,
            ProjectPolicyError::InvalidRequiredCapabilities,
            ProjectPolicyError::InvalidTrackRecordRequirements,
            ProjectPolicyError::UnchangedPolicy,
            ProjectPolicyError::InvalidStore,
            ProjectPolicyError::Store(rusqlite::Error::QueryReturnedNoRows),
            ProjectPolicyError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            10,
            "an arm was added to `ProjectPolicyError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(ProjectPolicyError::name).collect();
        assert_eq!(
            names.len(),
            10,
            "two arms share a startup name, so a swap between them is invisible: {names:?}"
        );
        for name in names {
            assert!(
                !name.contains(' ') && !name.contains('{'),
                "a name is a name, not a rendering: {name}"
            );
        }
    }
}
