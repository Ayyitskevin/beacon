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

pub(crate) fn verify_project_state(connection: &rusqlite::Connection) -> Result<(), ProjectError> {
    let mut statement =
        connection.prepare("SELECT workspace_id, id FROM projects ORDER BY workspace_id, id")?;
    let projects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    for (workspace_id, project_id) in projects {
        let workspace_id = parse_uuid(workspace_id)?;
        let project_id = parse_uuid(project_id)?;
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
