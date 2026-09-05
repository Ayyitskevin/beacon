use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OperatorAccountError, OperatorAccountService};

const MAX_PROJECT_NAME_CHARS: usize = 120;
const MAX_PURPOSE_CHARS: usize = 2_000;
const MAX_CHARTER_ITEMS: usize = 20;
const MAX_CHARTER_ITEM_CHARS: usize = 300;
const FIRST_CHARTER_REVISION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutonomyProfile {
    Observe,
    Supervised,
}

impl AutonomyProfile {
    /// The inverse of [`AutonomyProfile::as_str`], for the columns that store it.
    fn from_str(value: &str) -> Option<Self> {
        match value {
            "observe" => Some(Self::Observe),
            "supervised" => Some(Self::Supervised),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Supervised => "supervised",
        }
    }

    fn parse(value: &str) -> Result<Self, ProjectError> {
        match value {
            "observe" => Ok(Self::Observe),
            "supervised" => Ok(Self::Supervised),
            _ => Err(ProjectError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetBoundary {
    NoSpend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalEffects {
    Prohibited,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectCharterDraft {
    pub purpose: String,
    pub desired_outcomes: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub autonomy_profile: AutonomyProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectDraft {
    pub name: String,
    pub charter: ProjectCharterDraft,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectCharter {
    pub revision: u32,
    pub purpose: String,
    pub desired_outcomes: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub autonomy_profile: AutonomyProfile,
    pub budget_boundary: BudgetBoundary,
    pub external_effects: ExternalEffects,
    pub authored_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub charter: ProjectCharter,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectSummary {
    pub id: Uuid,
    pub name: String,
    pub charter_revision: u32,
    pub autonomy_profile: AutonomyProfile,
    pub created_at_ms: u64,
}

/// Which fault the Project startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum ProjectStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Project did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    ///
    /// MEASURED LIMIT, the same one `mission` has: the Project's own record is compared against
    /// its `project.created` event by an earlier pass, so a tampered column reopens as
    /// `ProjectEventMismatch` and never reaches here. What this pass uniquely reaches -- a charter
    /// history whose terminal revision disagrees, a non-monotonic authored_at, a wrong event count
    /// -- is all `ProjectError::InvalidStore`.
    Row {
        workspace_id: Uuid,
        project_id: Uuid,
        cause: ProjectError,
    },
    /// Counts across the whole store: Projects against creation snapshots against events. No row
    /// to name, and naming one with a nil UUID would be an invented identity.
    Totals(ProjectError),
    /// A row whose identity columns are not UUIDs. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity {
        workspace_id: String,
        project_id: String,
    },
}

impl ProjectError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::InvalidName => "InvalidName",
            Self::InvalidPurpose => "InvalidPurpose",
            Self::InvalidDesiredOutcomes => "InvalidDesiredOutcomes",
            Self::InvalidConstraint => "InvalidConstraint",
            Self::InvalidProhibition => "InvalidProhibition",
            Self::DuplicateCharterEntry => "DuplicateCharterEntry",
            Self::StaleCharterRevision { .. } => "StaleCharterRevision",
            Self::UnchangedCharter => "UnchangedCharter",
            Self::NotFound => "NotFound",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for ProjectStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Projects could not be read"),
            Self::Row {
                workspace_id,
                project_id,
                cause,
            } => write!(
                formatter,
                "Project {project_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Totals(cause) => write!(
                formatter,
                "the Projects do not agree with the store as a whole: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                project_id,
            } => write!(
                formatter,
                "a Project row's identity is not readable: Workspace {workspace_id}, Project \
                 {project_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ProjectStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(cause) => formatter.debug_tuple("Totals").field(cause).finish(),
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
        }
    }
}

impl std::error::Error for ProjectStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Totals(cause) | Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ProjectError {
    Account(OperatorAccountError),
    InvalidName,
    InvalidPurpose,
    InvalidDesiredOutcomes,
    InvalidConstraint,
    InvalidProhibition,
    DuplicateCharterEntry,
    StaleCharterRevision { expected: u32, current: u32 },
    UnchangedCharter,
    NotFound,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::InvalidName => write!(formatter, "Project name is invalid"),
            Self::InvalidPurpose => write!(formatter, "Project purpose is invalid"),
            Self::InvalidDesiredOutcomes => {
                write!(formatter, "Project needs between 1 and 20 desired outcomes")
            }
            Self::InvalidConstraint => write!(formatter, "Project constraint is invalid"),
            Self::InvalidProhibition => write!(formatter, "Project prohibition is invalid"),
            Self::DuplicateCharterEntry => {
                write!(formatter, "Project Charter entries must be distinct")
            }
            Self::StaleCharterRevision { expected, current } => write!(
                formatter,
                "Project Charter revision {expected} is stale; current revision is {current}"
            ),
            Self::UnchangedCharter => write!(formatter, "Project Charter is unchanged"),
            Self::NotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::InvalidStore => write!(formatter, "Project state is invalid"),
            Self::Store(error) => write!(formatter, "Project store failed: {error}"),
            Self::Json(error) => write!(formatter, "Project event is invalid: {error}"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<OperatorAccountError> for ProjectError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ProjectError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ProjectError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectCreatedPayload {
    project: ProjectView,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectCharterRevisedPayload {
    project_id: Uuid,
    previous_revision: u32,
    charter: ProjectCharter,
}

/// Owns authenticated Project queries and Operator-authored Charter creation.
pub struct ProjectService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ProjectService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn list(&self) -> Result<Vec<ProjectSummary>, ProjectError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            list_projects(connection, self.workspace_id)
        })
    }

    pub fn get(&self, project_id: Uuid) -> Result<ProjectView, ProjectError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            load_project(connection, self.workspace_id, project_id)
        })
    }

    pub fn charter_history(&self, project_id: Uuid) -> Result<Vec<ProjectCharter>, ProjectError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            load_charter_history(connection, self.workspace_id, project_id)
        })
    }

    pub fn create(&self, draft: ProjectDraft) -> Result<ProjectView, ProjectError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let draft = normalize_draft(draft)?;
            let project_id = Uuid::new_v4();
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            transaction.execute(
                "INSERT INTO projects (
                    id, workspace_id, name, current_charter_revision, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![
                    project_id.to_string(),
                    self.workspace_id.to_string(),
                    draft.name,
                    i64::from(FIRST_CHARTER_REVISION),
                    now
                ],
            )?;
            transaction.execute(
                "INSERT INTO project_charter_revisions (
                    workspace_id, project_id, revision, purpose, desired_outcomes_json,
                    constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                    external_effects, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'no_spend', 'prohibited',
                           'owner', ?9)",
                params![
                    self.workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(FIRST_CHARTER_REVISION),
                    draft.charter.purpose,
                    serde_json::to_string(&draft.charter.desired_outcomes)?,
                    serde_json::to_string(&draft.charter.constraints)?,
                    serde_json::to_string(&draft.charter.prohibitions)?,
                    draft.charter.autonomy_profile.as_str(),
                    now
                ],
            )?;
            transaction.execute(
                "INSERT INTO project_creation_snapshots (
                    project_id, workspace_id, initial_name, initial_charter_revision, created_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    project_id.to_string(),
                    self.workspace_id.to_string(),
                    draft.name,
                    i64::from(FIRST_CHARTER_REVISION),
                    now
                ],
            )?;
            let project = load_creation_project(&transaction, self.workspace_id, project_id)?;
            let payload = serde_json::to_string(&ProjectCreatedPayload {
                project: project.clone(),
            })?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.created', ?2, ?3)",
                params![self.workspace_id.to_string(), payload, now],
            )?;
            transaction.commit()?;
            Ok(project)
        })
    }

    pub fn revise_charter(
        &self,
        project_id: Uuid,
        expected_current_revision: u32,
        draft: ProjectCharterDraft,
    ) -> Result<ProjectView, ProjectError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let draft = normalize_charter(draft)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_project(&transaction, self.workspace_id, project_id)?;
            if current.charter.revision != expected_current_revision {
                return Err(ProjectError::StaleCharterRevision {
                    expected: expected_current_revision,
                    current: current.charter.revision,
                });
            }
            if charter_matches_draft(&current.charter, &draft) {
                return Err(ProjectError::UnchangedCharter);
            }
            let next_revision = current
                .charter
                .revision
                .checked_add(1)
                .ok_or(ProjectError::InvalidStore)?;
            let authored_at_ms = now.max(
                i64::try_from(current.charter.authored_at_ms)
                    .map_err(|_| ProjectError::InvalidStore)?,
            );
            transaction.execute(
                "INSERT INTO project_charter_revisions (
                    workspace_id, project_id, revision, purpose, desired_outcomes_json,
                    constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                    external_effects, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'no_spend', 'prohibited',
                           'owner', ?9)",
                params![
                    self.workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(next_revision),
                    draft.purpose,
                    serde_json::to_string(&draft.desired_outcomes)?,
                    serde_json::to_string(&draft.constraints)?,
                    serde_json::to_string(&draft.prohibitions)?,
                    draft.autonomy_profile.as_str(),
                    authored_at_ms
                ],
            )?;
            let changed = transaction.execute(
                "UPDATE projects
                 SET current_charter_revision = ?1, updated_at_ms = ?2
                 WHERE workspace_id = ?3 AND id = ?4 AND current_charter_revision = ?5",
                params![
                    i64::from(next_revision),
                    authored_at_ms,
                    self.workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(expected_current_revision)
                ],
            )?;
            if changed != 1 {
                return Err(ProjectError::InvalidStore);
            }
            let project = load_project(&transaction, self.workspace_id, project_id)?;
            let payload = serde_json::to_string(&ProjectCharterRevisedPayload {
                project_id,
                previous_revision: expected_current_revision,
                charter: project.charter.clone(),
            })?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.charter_revised', ?2, ?3)",
                params![self.workspace_id.to_string(), payload, authored_at_ms],
            )?;
            transaction.commit()?;
            Ok(project)
        })
    }
}

fn normalize_draft(mut draft: ProjectDraft) -> Result<ProjectDraft, ProjectError> {
    draft.name = normalize_single_line(draft.name, MAX_PROJECT_NAME_CHARS)
        .ok_or(ProjectError::InvalidName)?;
    draft.charter = normalize_charter(draft.charter)?;
    Ok(draft)
}

fn normalize_charter(
    mut charter: ProjectCharterDraft,
) -> Result<ProjectCharterDraft, ProjectError> {
    charter.purpose = normalize_purpose(charter.purpose)?;
    charter.desired_outcomes = normalize_items(
        charter.desired_outcomes,
        true,
        ProjectError::InvalidDesiredOutcomes,
    )?;
    charter.constraints =
        normalize_items(charter.constraints, false, ProjectError::InvalidConstraint)?;
    charter.prohibitions = normalize_items(
        charter.prohibitions,
        false,
        ProjectError::InvalidProhibition,
    )?;
    Ok(charter)
}

fn charter_matches_draft(charter: &ProjectCharter, draft: &ProjectCharterDraft) -> bool {
    charter.purpose == draft.purpose
        && charter.desired_outcomes == draft.desired_outcomes
        && charter.constraints == draft.constraints
        && charter.prohibitions == draft.prohibitions
        && charter.autonomy_profile == draft.autonomy_profile
}

fn normalize_purpose(value: String) -> Result<String, ProjectError> {
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(ProjectError::InvalidPurpose);
    }
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_PURPOSE_CHARS {
        return Err(ProjectError::InvalidPurpose);
    }
    Ok(value.to_owned())
}

fn normalize_items(
    values: Vec<String>,
    required: bool,
    invalid_error: ProjectError,
) -> Result<Vec<String>, ProjectError> {
    if values.len() > MAX_CHARTER_ITEMS || (required && values.is_empty()) {
        return Err(invalid_error);
    }
    let mut normalized = Vec::with_capacity(values.len());
    let mut distinct = HashSet::with_capacity(values.len());
    for value in values {
        let value =
            normalize_single_line(value, MAX_CHARTER_ITEM_CHARS).ok_or_else(
                || match &invalid_error {
                    ProjectError::InvalidDesiredOutcomes => ProjectError::InvalidDesiredOutcomes,
                    ProjectError::InvalidConstraint => ProjectError::InvalidConstraint,
                    ProjectError::InvalidProhibition => ProjectError::InvalidProhibition,
                    _ => ProjectError::InvalidStore,
                },
            )?;
        if !distinct.insert(value.clone()) {
            return Err(ProjectError::DuplicateCharterEntry);
        }
        normalized.push(value);
    }
    Ok(normalized)
}

fn normalize_single_line(value: String, max_chars: usize) -> Option<String> {
    if value.chars().any(char::is_control) {
        return None;
    }
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max_chars {
        return None;
    }
    Some(value.to_owned())
}

fn list_projects(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
) -> Result<Vec<ProjectSummary>, ProjectError> {
    let mut statement = connection.prepare(
        "SELECT project.id, project.name, project.current_charter_revision,
                charter.autonomy_profile, project.created_at_ms
         FROM projects AS project
         JOIN project_charter_revisions AS charter
           ON charter.workspace_id = project.workspace_id
          AND charter.project_id = project.id
          AND charter.revision = project.current_charter_revision
         WHERE project.workspace_id = ?1
         ORDER BY project.created_at_ms DESC, project.id",
    )?;
    statement
        .query_map([workspace_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?
        .map(|row| {
            let (id, name, revision, autonomy_profile, created_at_ms) = row?;
            Ok(ProjectSummary {
                id: parse_uuid(id)?,
                name: validate_stored_single_line(name, MAX_PROJECT_NAME_CHARS)?,
                charter_revision: positive_u32(revision)?,
                autonomy_profile: AutonomyProfile::parse(&autonomy_profile)?,
                created_at_ms: nonnegative_u64(created_at_ms)?,
            })
        })
        .collect()
}

fn load_project(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<ProjectView, ProjectError> {
    let stored = connection
        .query_row(
            "SELECT name, current_charter_revision, created_at_ms
             FROM projects WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((name, revision, created_at_ms)) = stored else {
        return Err(ProjectError::NotFound);
    };
    Ok(ProjectView {
        id: project_id,
        workspace_id,
        name: validate_stored_single_line(name, MAX_PROJECT_NAME_CHARS)?,
        charter: load_charter(
            connection,
            workspace_id,
            project_id,
            positive_u32(revision)?,
        )?,
        created_at_ms: nonnegative_u64(created_at_ms)?,
    })
}

/// The autonomy profile of a Project's **current** Charter revision.
///
/// Owned here rather than joined by a consumer: "current" is this module's invariant -- the pointer
/// on `projects` must name the terminal revision -- and a write-authorizing gate that assembled its
/// own join would carry a second definition of it, which would drift the day this module's does
/// (gpt1's P0 on dispatch).
///
/// `Ok(None)` means no such Project. A Charter revision the pointer names that does not exist, or
/// one carrying a profile outside the closed vocabulary, is invalid state rather than a Project
/// with no profile.
pub(crate) fn current_autonomy_profile(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<AutonomyProfile>, ProjectError> {
    let Some(revision) = current_charter_revision(connection, workspace_id, project_id)? else {
        return Ok(None);
    };
    let profile: String = connection
        .query_row(
            "SELECT autonomy_profile FROM project_charter_revisions
             WHERE workspace_id = ?1 AND project_id = ?2 AND revision = ?3",
            params![
                workspace_id.to_string(),
                project_id.to_string(),
                i64::from(revision)
            ],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(ProjectError::InvalidStore)?;
    AutonomyProfile::from_str(&profile)
        .map(Some)
        .ok_or(ProjectError::InvalidStore)
}

pub(crate) fn current_charter_revision(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<u32>, ProjectError> {
    connection
        .query_row(
            "SELECT current_charter_revision
             FROM projects
             WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|revision| u32::try_from(revision).map_err(|_| ProjectError::InvalidStore))
        .transpose()
}

fn load_charter_history(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<ProjectCharter>, ProjectError> {
    let _current = load_project(connection, workspace_id, project_id)?;
    let mut statement = connection.prepare(
        "SELECT revision FROM project_charter_revisions
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
            load_charter(
                connection,
                workspace_id,
                project_id,
                positive_u32(revision)?,
            )
        })
        .collect()
}

fn load_creation_project(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<ProjectView, ProjectError> {
    let stored = connection
        .query_row(
            "SELECT initial_name, initial_charter_revision, created_at_ms
             FROM project_creation_snapshots
             WHERE workspace_id = ?1 AND project_id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((name, revision, created_at_ms)) = stored else {
        return Err(ProjectError::InvalidStore);
    };
    Ok(ProjectView {
        id: project_id,
        workspace_id,
        name: validate_stored_single_line(name, MAX_PROJECT_NAME_CHARS)?,
        charter: load_charter(
            connection,
            workspace_id,
            project_id,
            positive_u32(revision)?,
        )?,
        created_at_ms: nonnegative_u64(created_at_ms)?,
    })
}

fn load_charter(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
    revision: u32,
) -> Result<ProjectCharter, ProjectError> {
    let stored = connection
        .query_row(
            "SELECT purpose, desired_outcomes_json, constraints_json, prohibitions_json,
                    autonomy_profile, budget_boundary, external_effects, author_kind,
                    authored_at_ms
             FROM project_charter_revisions
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
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()?;
    let Some((
        purpose,
        desired_outcomes_json,
        constraints_json,
        prohibitions_json,
        autonomy_profile,
        budget_boundary,
        external_effects,
        author_kind,
        authored_at_ms,
    )) = stored
    else {
        return Err(ProjectError::InvalidStore);
    };
    if budget_boundary != "no_spend" || external_effects != "prohibited" || author_kind != "owner" {
        return Err(ProjectError::InvalidStore);
    }
    let charter = ProjectCharterDraft {
        purpose,
        desired_outcomes: serde_json::from_str(&desired_outcomes_json)?,
        constraints: serde_json::from_str(&constraints_json)?,
        prohibitions: serde_json::from_str(&prohibitions_json)?,
        autonomy_profile: AutonomyProfile::parse(&autonomy_profile)?,
    };
    let normalized = normalize_draft(ProjectDraft {
        name: "validation".to_owned(),
        charter,
    })?
    .charter;
    Ok(ProjectCharter {
        revision,
        purpose: normalized.purpose,
        desired_outcomes: normalized.desired_outcomes,
        constraints: normalized.constraints,
        prohibitions: normalized.prohibitions,
        autonomy_profile: normalized.autonomy_profile,
        budget_boundary: BudgetBoundary::NoSpend,
        external_effects: ExternalEffects::Prohibited,
        authored_at_ms: nonnegative_u64(authored_at_ms)?,
    })
}

/// Does this Workspace hold this Project, proved against its own `project.created` Event?
///
/// The mirror of `mission::verified_mission_project`, and added for the same finding: a completion
/// names a Project in its canonical bytes and nothing established that the Project exists in the
/// Workspace at all, let alone that it owns the Mission.
pub(crate) fn verified_project_exists(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<bool, ProjectError> {
    let mut statement = connection.prepare(
        "SELECT payload_json, occurred_at_ms FROM events
         WHERE workspace_id = ?1 AND kind = 'project.created'
           AND json_extract(payload_json, '$.project.id') = ?2",
    )?;
    let events: Vec<(String, i64)> = statement
        .query_map(
            rusqlite::params![workspace_id.to_string(), project_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    match events.as_slice() {
        [] => Ok(false),
        [(payload_json, occurred_at_ms)] => {
            verify_project_event(
                connection,
                workspace_id,
                nonnegative_u64(*occurred_at_ms)?,
                payload_json,
            )?;
            Ok(true)
        }
        _ => Err(ProjectError::InvalidStore),
    }
}

pub(crate) fn verify_project_event(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ProjectError> {
    let payload: ProjectCreatedPayload = serde_json::from_str(payload_json)?;
    if payload.project.workspace_id != workspace_id
        || payload.project.created_at_ms != occurred_at_ms
        || payload.project != load_creation_project(connection, workspace_id, payload.project.id)?
    {
        return Err(ProjectError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_project_charter_revised_event(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), ProjectError> {
    let payload: ProjectCharterRevisedPayload = serde_json::from_str(payload_json)?;
    if payload.charter.revision <= FIRST_CHARTER_REVISION
        || payload.previous_revision.checked_add(1) != Some(payload.charter.revision)
        || payload.charter.authored_at_ms != occurred_at_ms
        || payload.charter
            != load_charter(
                connection,
                workspace_id,
                payload.project_id,
                payload.charter.revision,
            )?
    {
        return Err(ProjectError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_project_state(
    connection: &rusqlite::Connection,
) -> Result<(), ProjectStateFault> {
    let mut statement = connection
        .prepare("SELECT workspace_id, id FROM projects ORDER BY workspace_id, id")
        .map_err(ProjectStateFault::Store)?;
    let projects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(ProjectStateFault::Store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(ProjectStateFault::Store)?;

    for (workspace_id, project_id) in &projects {
        let identity = || ProjectStateFault::Identity {
            workspace_id: workspace_id.clone(),
            project_id: project_id.clone(),
        };
        let parsed_workspace = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        let parsed_project = Uuid::parse_str(project_id).map_err(|_| identity())?;
        verify_one_project(connection, parsed_workspace, parsed_project).map_err(|cause| {
            ProjectStateFault::Row {
                workspace_id: parsed_workspace,
                project_id: parsed_project,
                cause,
            }
        })?;
    }
    verify_project_totals(connection).map_err(|cause| match cause {
        ProjectError::Store(error) => ProjectStateFault::Store(error),
        other => ProjectStateFault::Totals(other),
    })?;
    Ok(())
}

/// One stored Project, re-proved. Unchanged from what the loop held inline.
fn verify_one_project(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(), ProjectError> {
    {
        let current = load_project(connection, workspace_id, project_id)?;
        let creation = load_creation_project(connection, workspace_id, project_id)?;
        let updated_at_ms = connection.query_row(
            "SELECT updated_at_ms FROM projects WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        if creation.charter.revision != FIRST_CHARTER_REVISION
            || current.created_at_ms != creation.created_at_ms
            || creation.charter.authored_at_ms != creation.created_at_ms
            || nonnegative_u64(updated_at_ms)? != current.charter.authored_at_ms
        {
            return Err(ProjectError::InvalidStore);
        }
        let revisions = load_charter_history(connection, workspace_id, project_id)?;
        let terminal_revision = verify_all_charter_revisions(&revisions)?;
        if current.charter.revision != terminal_revision {
            return Err(ProjectError::InvalidStore);
        }
        for pair in revisions.windows(2) {
            if pair[0].authored_at_ms > pair[1].authored_at_ms {
                return Err(ProjectError::InvalidStore);
            }
        }
        let event_count: i64 = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'project.created'
               AND json_extract(payload_json, '$.project.id') = ?2",
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get(0),
        )?;
        if event_count != 1 {
            return Err(ProjectError::InvalidStore);
        }
        for revision in (FIRST_CHARTER_REVISION + 1)..=terminal_revision {
            let revision_event_count: i64 = connection.query_row(
                "SELECT count(*) FROM events
                 WHERE workspace_id = ?1 AND kind = 'project.charter_revised'
                   AND json_extract(payload_json, '$.project_id') = ?2
                   AND json_extract(payload_json, '$.charter.revision') = ?3",
                params![
                    workspace_id.to_string(),
                    project_id.to_string(),
                    i64::from(revision)
                ],
                |row| row.get(0),
            )?;
            if revision_event_count != 1 {
                return Err(ProjectError::InvalidStore);
            }
        }
    }
    Ok(())
}

/// The checks that are about the STORE rather than one Project.
fn verify_project_totals(connection: &rusqlite::Connection) -> Result<(), ProjectError> {
    let project_count: i64 =
        connection.query_row("SELECT count(*) FROM projects", [], |row| row.get(0))?;
    let snapshot_count: i64 = connection.query_row(
        "SELECT count(*) FROM project_creation_snapshots",
        [],
        |row| row.get(0),
    )?;
    let project_event_count: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind = 'project.created'",
        [],
        |row| row.get(0),
    )?;
    let expected_revision_event_count: i64 = connection.query_row(
        "SELECT count(*) FROM project_charter_revisions WHERE revision > 1",
        [],
        |row| row.get(0),
    )?;
    let revision_event_count: i64 = connection.query_row(
        "SELECT count(*) FROM events WHERE kind = 'project.charter_revised'",
        [],
        |row| row.get(0),
    )?;
    if project_count != snapshot_count || project_count != project_event_count {
        return Err(ProjectError::InvalidStore);
    }
    if expected_revision_event_count != revision_event_count {
        return Err(ProjectError::InvalidStore);
    }
    Ok(())
}

fn verify_all_charter_revisions(revisions: &[ProjectCharter]) -> Result<u32, ProjectError> {
    if revisions.is_empty() {
        return Err(ProjectError::InvalidStore);
    }
    let mut terminal_revision = FIRST_CHARTER_REVISION;
    for (index, charter) in revisions.iter().enumerate() {
        let expected_revision = u32::try_from(index + 1).map_err(|_| ProjectError::InvalidStore)?;
        if charter.revision != expected_revision {
            return Err(ProjectError::InvalidStore);
        }
        terminal_revision = charter.revision;
    }
    Ok(terminal_revision)
}

fn validate_stored_single_line(value: String, max_chars: usize) -> Result<String, ProjectError> {
    let normalized =
        normalize_single_line(value.clone(), max_chars).ok_or(ProjectError::InvalidStore)?;
    if normalized != value {
        return Err(ProjectError::InvalidStore);
    }
    Ok(value)
}

fn parse_uuid(value: String) -> Result<Uuid, ProjectError> {
    Uuid::parse_str(&value).map_err(|_| ProjectError::InvalidStore)
}

fn positive_u32(value: i64) -> Result<u32, ProjectError> {
    let value = u32::try_from(value).map_err(|_| ProjectError::InvalidStore)?;
    if value == 0 {
        return Err(ProjectError::InvalidStore);
    }
    Ok(value)
}

fn nonnegative_u64(value: i64) -> Result<u64, ProjectError> {
    u64::try_from(value).map_err(|_| ProjectError::InvalidStore)
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()`; see the note in `outcome_publication`'s twin.
    fn expected_name(error: &ProjectError) -> &'static str {
        match error {
            ProjectError::Account(_) => "Account",
            ProjectError::InvalidName => "InvalidName",
            ProjectError::InvalidPurpose => "InvalidPurpose",
            ProjectError::InvalidDesiredOutcomes => "InvalidDesiredOutcomes",
            ProjectError::InvalidConstraint => "InvalidConstraint",
            ProjectError::InvalidProhibition => "InvalidProhibition",
            ProjectError::DuplicateCharterEntry => "DuplicateCharterEntry",
            ProjectError::StaleCharterRevision { .. } => "StaleCharterRevision",
            ProjectError::UnchangedCharter => "UnchangedCharter",
            ProjectError::NotFound => "NotFound",
            ProjectError::InvalidStore => "InvalidStore",
            ProjectError::Store(_) => "Store",
            ProjectError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_project_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            ProjectError::Account(OperatorAccountError::PasswordTooShort),
            ProjectError::InvalidName,
            ProjectError::InvalidPurpose,
            ProjectError::InvalidDesiredOutcomes,
            ProjectError::InvalidConstraint,
            ProjectError::InvalidProhibition,
            ProjectError::DuplicateCharterEntry,
            ProjectError::StaleCharterRevision {
                expected: 1,
                current: 2,
            },
            ProjectError::UnchangedCharter,
            ProjectError::NotFound,
            ProjectError::InvalidStore,
            ProjectError::Store(rusqlite::Error::QueryReturnedNoRows),
            ProjectError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            13,
            "an arm was added to `ProjectError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(ProjectError::name).collect();
        assert_eq!(
            names.len(),
            13,
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
