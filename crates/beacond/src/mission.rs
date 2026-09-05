use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project::current_charter_revision;
use crate::team::load_team_for_mission;
use crate::{OperatorAccountError, OperatorAccountService, ProjectError, TeamError};

const MAX_DESIRED_OUTCOME_CHARS: usize = 2_000;
const MAX_MISSION_ITEMS: usize = 20;
const MAX_MISSION_ITEM_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionAdmissionActor {
    Operator,
}

impl MissionAdmissionActor {
    fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator",
        }
    }

    fn parse(value: &str) -> Result<Self, MissionError> {
        match value {
            "operator" => Ok(Self::Operator),
            _ => Err(MissionError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionLifecycle {
    Admitted,
}

impl MissionLifecycle {
    fn as_str(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
        }
    }

    fn parse(value: &str) -> Result<Self, MissionError> {
        match value {
            "admitted" => Ok(Self::Admitted),
            _ => Err(MissionError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionMemberDraft {
    pub resident_id: Uuid,
    pub expected_configuration_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionDraft {
    pub project_id: Uuid,
    pub expected_charter_revision: u32,
    pub team_id: Uuid,
    pub desired_outcome: String,
    pub success_conditions: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub members: Vec<MissionMemberDraft>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionMemberView {
    pub resident_id: Uuid,
    pub admission_configuration_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissionView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub charter_revision: u32,
    pub team_id: Uuid,
    pub desired_outcome: String,
    pub success_conditions: Vec<String>,
    pub constraints: Vec<String>,
    pub prohibitions: Vec<String>,
    pub lifecycle: MissionLifecycle,
    pub admitted_by: MissionAdmissionActor,
    pub admitted_at_ms: u64,
    pub members: Vec<MissionMemberView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredMissionAdmission {
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    charter_revision: u32,
    team_id: Uuid,
    desired_outcome: String,
    success_conditions: Vec<String>,
    constraints: Vec<String>,
    prohibitions: Vec<String>,
    lifecycle: MissionLifecycle,
    admitted_by: MissionAdmissionActor,
    admitted_at_ms: u64,
    members: Vec<MissionMemberView>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MissionAdmittedPayload {
    mission: StoredMissionAdmission,
}

/// Which fault the Mission startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum MissionStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Mission did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    ///
    /// MEASURED LIMIT, recorded because it bounds what this arm proves: every column of `missions`
    /// is compared against the admission event's payload by `verify_mission_event`, which runs
    /// earlier, so a tampered column reopens as `MissionEventMismatch` and never reaches here. The
    /// faults this pass can uniquely reach -- a wrong admitted-event count, an orphaned condition
    /// row, a condition projection that disagrees with the Mission's own bytes -- are ALL
    /// `MissionError::InvalidStore`. The row identity is therefore the whole of what widening buys
    /// for this pass today, and a "cause flattened" mutant cannot be killed here.
    Row {
        workspace_id: Uuid,
        mission_id: Uuid,
        cause: MissionError,
    },
    /// A check that is about the STORE rather than one Mission: an orphaned condition row, or an
    /// admitted-event count that does not match the number of Missions. There is no row to name,
    /// and naming one with a nil UUID would be the invented identity this lane refuses everywhere
    /// else.
    Totals(crate::TotalsMismatch),
    /// A row whose identity columns are not UUIDs. Unreachable while the foreign-key check and the
    /// Workspace pass run first.
    Identity {
        workspace_id: String,
        mission_id: String,
    },
}

impl MissionError {
    /// The variant's NAME, for R28.4's one-line startup message. `Store`, `Json` and
    /// `ResidentIneligible` all carry values that do not belong in a refusal line.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ProjectNotFound => "ProjectNotFound",
            Self::TeamNotFound => "TeamNotFound",
            Self::MissionNotFound => "MissionNotFound",
            Self::StaleCharterRevision { .. } => "StaleCharterRevision",
            Self::InvalidDesiredOutcome => "InvalidDesiredOutcome",
            Self::InvalidSuccessConditions => "InvalidSuccessConditions",
            Self::InvalidConstraint => "InvalidConstraint",
            Self::InvalidProhibition => "InvalidProhibition",
            Self::DuplicateMissionEntry => "DuplicateMissionEntry",
            Self::InvalidMembers => "InvalidMembers",
            Self::StaleTeamEligibility => "StaleTeamEligibility",
            Self::ResidentIneligible(_) => "ResidentIneligible",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for MissionStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the Missions could not be read"),
            Self::Totals(totals) => write!(
                formatter,
                "the Missions do not agree with the store as a whole -- {totals}"
            ),
            Self::Row {
                workspace_id,
                mission_id,
                cause,
            } => write!(
                formatter,
                "Mission {mission_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                mission_id,
            } => write!(
                formatter,
                "a Mission row's identity is not readable: Workspace {workspace_id}, Mission \
                 {mission_id}"
            ),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for MissionStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Totals(totals) => formatter.debug_tuple("Totals").field(totals).finish(),
            Self::Row {
                workspace_id,
                mission_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("mission_id", mission_id)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                mission_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("mission_id", mission_id)
                .finish(),
        }
    }
}

impl std::error::Error for MissionStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            // A `TotalsMismatch` is DATA, not a nested error: it has no source of its own.
            Self::Totals(_) => None,
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum MissionError {
    Account(OperatorAccountError),
    ProjectNotFound,
    TeamNotFound,
    MissionNotFound,
    StaleCharterRevision { expected: u32, current: u32 },
    InvalidDesiredOutcome,
    InvalidSuccessConditions,
    InvalidConstraint,
    InvalidProhibition,
    DuplicateMissionEntry,
    InvalidMembers,
    StaleTeamEligibility,
    ResidentIneligible(Uuid),
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for MissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ProjectNotFound => write!(formatter, "Project was not found in this Workspace"),
            Self::TeamNotFound => write!(formatter, "Team was not found in this Project"),
            Self::MissionNotFound => write!(formatter, "Mission was not found in this Workspace"),
            Self::StaleCharterRevision { expected, current } => write!(
                formatter,
                "Project Charter changed from revision {expected} to {current} before Mission admission"
            ),
            Self::InvalidDesiredOutcome => write!(formatter, "Mission desired outcome is invalid"),
            Self::InvalidSuccessConditions => write!(
                formatter,
                "Mission needs between 1 and 20 success conditions"
            ),
            Self::InvalidConstraint => write!(formatter, "Mission constraint is invalid"),
            Self::InvalidProhibition => write!(formatter, "Mission prohibition is invalid"),
            Self::DuplicateMissionEntry => {
                write!(
                    formatter,
                    "Mission entries must be distinct within each section"
                )
            }
            Self::InvalidMembers => write!(
                formatter,
                "Mission members must exactly match the Team roster in formation order"
            ),
            Self::StaleTeamEligibility => write!(
                formatter,
                "Team membership or Resident configuration changed before Mission admission"
            ),
            Self::ResidentIneligible(resident_id) => write!(
                formatter,
                "Resident {resident_id} is not currently eligible for Mission admission"
            ),
            Self::InvalidStore => write!(formatter, "Mission state is invalid"),
            Self::Store(error) => write!(formatter, "Mission store failed: {error}"),
            Self::Json(error) => write!(formatter, "Mission event is invalid: {error}"),
        }
    }
}

impl std::error::Error for MissionError {}

impl From<OperatorAccountError> for MissionError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for MissionError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for MissionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns authenticated, Project-scoped Mission admission and immutable admission queries.
pub struct MissionService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl MissionService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn list(&self, project_id: Uuid) -> Result<Vec<MissionView>, MissionError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            require_project(connection, self.workspace_id, project_id)?;
            list_missions(connection, self.workspace_id, project_id)
        })
    }

    pub fn get(&self, mission_id: Uuid) -> Result<MissionView, MissionError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            let mission = load_stored_mission(connection, self.workspace_id, mission_id)?
                .ok_or(MissionError::MissionNotFound)?;
            Ok(render_mission(mission))
        })
    }

    pub fn admit(&self, mut draft: MissionDraft) -> Result<MissionView, MissionError> {
        self.accounts.with_authenticated_store(|connection, now| {
            normalize_draft(&mut draft)?;
            validate_member_draft(&draft.members)?;
            let admitted_at_ms = u64::try_from(now).map_err(|_| MissionError::InvalidStore)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

            let current_revision =
                current_charter_revision(&transaction, self.workspace_id, draft.project_id)
                    .map_err(map_project_error)?
                    .ok_or(MissionError::ProjectNotFound)?;
            if current_revision != draft.expected_charter_revision {
                return Err(MissionError::StaleCharterRevision {
                    expected: draft.expected_charter_revision,
                    current: current_revision,
                });
            }

            let team = load_team_for_mission(
                &transaction,
                self.workspace_id,
                draft.project_id,
                draft.team_id,
            )
            .map_err(map_team_error)?
            .ok_or(MissionError::TeamNotFound)?;
            if team.members.len() != draft.members.len()
                || team
                    .members
                    .iter()
                    .zip(&draft.members)
                    .any(|(actual, expected)| {
                        actual.resident_id != expected.resident_id
                            || actual.current_configuration_id != expected.expected_configuration_id
                    })
            {
                return Err(MissionError::StaleTeamEligibility);
            }
            if let Some(member) = team.members.iter().find(|member| !member.eligible_now) {
                return Err(MissionError::ResidentIneligible(member.resident_id));
            }

            let mission = StoredMissionAdmission {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                project_id: draft.project_id,
                charter_revision: draft.expected_charter_revision,
                team_id: draft.team_id,
                desired_outcome: draft.desired_outcome,
                success_conditions: draft.success_conditions,
                constraints: draft.constraints,
                prohibitions: draft.prohibitions,
                lifecycle: MissionLifecycle::Admitted,
                admitted_by: MissionAdmissionActor::Operator,
                admitted_at_ms,
                members: draft
                    .members
                    .into_iter()
                    .map(|member| MissionMemberView {
                        resident_id: member.resident_id,
                        admission_configuration_id: member.expected_configuration_id,
                    })
                    .collect(),
            };
            insert_admission(&transaction, &mission)?;
            let view = render_mission(mission);
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn normalize_draft(draft: &mut MissionDraft) -> Result<(), MissionError> {
    draft.desired_outcome = normalize_desired_outcome(std::mem::take(&mut draft.desired_outcome))?;
    draft.success_conditions = normalize_items(
        std::mem::take(&mut draft.success_conditions),
        false,
        MissionError::InvalidSuccessConditions,
    )?;
    draft.constraints = normalize_items(
        std::mem::take(&mut draft.constraints),
        true,
        MissionError::InvalidConstraint,
    )?;
    draft.prohibitions = normalize_items(
        std::mem::take(&mut draft.prohibitions),
        true,
        MissionError::InvalidProhibition,
    )?;
    Ok(())
}

fn normalize_desired_outcome(value: String) -> Result<String, MissionError> {
    if value
        .chars()
        .any(|character| character.is_control() && character != '\n')
    {
        return Err(MissionError::InvalidDesiredOutcome);
    }
    let normalized = value.trim();
    if normalized.is_empty() || normalized.chars().count() > MAX_DESIRED_OUTCOME_CHARS {
        return Err(MissionError::InvalidDesiredOutcome);
    }
    Ok(normalized.to_owned())
}

fn normalize_text(
    value: String,
    max_chars: usize,
    invalid: MissionError,
) -> Result<String, MissionError> {
    let normalized = value.trim();
    if normalized.is_empty()
        || normalized.chars().count() > max_chars
        || normalized.chars().any(char::is_control)
    {
        return Err(invalid);
    }
    Ok(normalized.to_owned())
}

fn normalize_items(
    values: Vec<String>,
    allow_empty: bool,
    invalid: MissionError,
) -> Result<Vec<String>, MissionError> {
    if values.len() > MAX_MISSION_ITEMS || (!allow_empty && values.is_empty()) {
        return Err(invalid);
    }
    let normalized = values
        .into_iter()
        .map(|value| normalize_text(value, MAX_MISSION_ITEM_CHARS, MissionError::InvalidStore))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid)?;
    let mut distinct = HashSet::with_capacity(normalized.len());
    if normalized.iter().any(|value| !distinct.insert(value)) {
        return Err(MissionError::DuplicateMissionEntry);
    }
    Ok(normalized)
}

fn validate_member_draft(members: &[MissionMemberDraft]) -> Result<(), MissionError> {
    let mut residents = HashSet::with_capacity(members.len());
    if members.is_empty()
        || members
            .iter()
            .any(|member| !residents.insert(member.resident_id))
    {
        return Err(MissionError::InvalidMembers);
    }
    Ok(())
}

fn require_project(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<(), MissionError> {
    if current_charter_revision(connection, workspace_id, project_id)
        .map_err(map_project_error)?
        .is_none()
    {
        return Err(MissionError::ProjectNotFound);
    }
    Ok(())
}

fn insert_admission(
    connection: &Connection,
    mission: &StoredMissionAdmission,
) -> Result<(), MissionError> {
    connection.execute(
        "INSERT INTO missions (
             id, workspace_id, project_id, charter_revision, team_id, desired_outcome,
             success_conditions_json, constraints_json, prohibitions_json, lifecycle,
             admitted_by, admitted_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            mission.id.to_string(),
            mission.workspace_id.to_string(),
            mission.project_id.to_string(),
            i64::from(mission.charter_revision),
            mission.team_id.to_string(),
            mission.desired_outcome,
            serde_json::to_string(&mission.success_conditions)?,
            serde_json::to_string(&mission.constraints)?,
            serde_json::to_string(&mission.prohibitions)?,
            mission.lifecycle.as_str(),
            mission.admitted_by.as_str(),
            i64::try_from(mission.admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
        ],
    )?;
    for (position, member) in mission.members.iter().enumerate() {
        connection.execute(
            "INSERT INTO mission_admission_members (
                 workspace_id, mission_id, team_id, position, resident_id, configuration_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                mission.workspace_id.to_string(),
                mission.id.to_string(),
                mission.team_id.to_string(),
                i64::try_from(position).map_err(|_| MissionError::InvalidStore)?,
                member.resident_id.to_string(),
                member.admission_configuration_id.to_string(),
            ],
        )?;
    }
    // The declared conditions, given rows in the same transaction that writes the Mission (ADR
    // 0041). The migration's backfill covers the Missions that existed before this table; this
    // covers every Mission after it, and without it the first admission after the migration fails
    // startup reconciliation -- which is how the omission was found, by an existing test, one build
    // after the design's slice-1 list did not mention it.
    for (ordinal, text) in mission.success_conditions.iter().enumerate() {
        connection.execute(
            "INSERT INTO mission_success_conditions (workspace_id, mission_id, ordinal, text)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                mission.workspace_id.to_string(),
                mission.id.to_string(),
                i64::try_from(ordinal).map_err(|_| MissionError::InvalidStore)?,
                text,
            ],
        )?;
    }
    let payload = serde_json::to_string(&MissionAdmittedPayload {
        mission: mission.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, 'mission.admitted', ?2, ?3)",
        params![
            mission.workspace_id.to_string(),
            payload,
            i64::try_from(mission.admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn list_missions(
    connection: &Connection,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Vec<MissionView>, MissionError> {
    let mut statement = connection.prepare(
        "SELECT id FROM missions
         WHERE workspace_id = ?1 AND project_id = ?2
         ORDER BY admitted_at_ms, id",
    )?;
    let ids = statement
        .query_map(
            params![workspace_id.to_string(), project_id.to_string()],
            |row| row.get::<_, String>(0),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| {
            let id = parse_uuid(id)?;
            load_stored_mission(connection, workspace_id, id)?
                .map(render_mission)
                .ok_or(MissionError::InvalidStore)
        })
        .collect()
}

fn load_stored_mission(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<StoredMissionAdmission>, MissionError> {
    let row = connection
        .query_row(
            "SELECT id, workspace_id, project_id, charter_revision, team_id,
                    desired_outcome, success_conditions_json, constraints_json,
                    prohibitions_json, lifecycle, admitted_by, admitted_at_ms
             FROM missions WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, i64>(11)?,
                ))
            },
        )
        .optional()?;
    let Some((
        id,
        stored_workspace_id,
        project_id,
        charter_revision,
        team_id,
        desired_outcome,
        success_conditions_json,
        constraints_json,
        prohibitions_json,
        lifecycle,
        admitted_by,
        admitted_at_ms,
    )) = row
    else {
        return Ok(None);
    };
    let id = parse_uuid(id)?;
    let stored_workspace_id = parse_uuid(stored_workspace_id)?;
    let mut statement = connection.prepare(
        "SELECT position, resident_id, configuration_id
         FROM mission_admission_members
         WHERE workspace_id = ?1 AND mission_id = ?2
         ORDER BY position",
    )?;
    let rows = statement
        .query_map(
            params![stored_workspace_id.to_string(), id.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.is_empty()
        || rows
            .iter()
            .enumerate()
            .any(|(index, (position, _, _))| usize::try_from(*position) != Ok(index))
    {
        return Err(MissionError::InvalidStore);
    }
    let members = rows
        .into_iter()
        .map(|(_, resident_id, configuration_id)| {
            Ok(MissionMemberView {
                resident_id: parse_uuid(resident_id)?,
                admission_configuration_id: parse_uuid(configuration_id)?,
            })
        })
        .collect::<Result<Vec<_>, MissionError>>()?;
    let mut mission = StoredMissionAdmission {
        id,
        workspace_id: stored_workspace_id,
        project_id: parse_uuid(project_id)?,
        charter_revision: u32::try_from(charter_revision)
            .map_err(|_| MissionError::InvalidStore)?,
        team_id: parse_uuid(team_id)?,
        desired_outcome,
        success_conditions: serde_json::from_str(&success_conditions_json)?,
        constraints: serde_json::from_str(&constraints_json)?,
        prohibitions: serde_json::from_str(&prohibitions_json)?,
        lifecycle: MissionLifecycle::parse(&lifecycle)?,
        admitted_by: MissionAdmissionActor::parse(&admitted_by)?,
        admitted_at_ms: u64::try_from(admitted_at_ms).map_err(|_| MissionError::InvalidStore)?,
        members,
    };
    let before = mission.clone();
    normalize_stored_mission(&mut mission)?;
    if mission != before {
        return Err(MissionError::InvalidStore);
    }
    Ok(Some(mission))
}

fn normalize_stored_mission(mission: &mut StoredMissionAdmission) -> Result<(), MissionError> {
    let mut draft = MissionDraft {
        project_id: mission.project_id,
        expected_charter_revision: mission.charter_revision,
        team_id: mission.team_id,
        desired_outcome: std::mem::take(&mut mission.desired_outcome),
        success_conditions: std::mem::take(&mut mission.success_conditions),
        constraints: std::mem::take(&mut mission.constraints),
        prohibitions: std::mem::take(&mut mission.prohibitions),
        members: mission
            .members
            .iter()
            .map(|member| MissionMemberDraft {
                resident_id: member.resident_id,
                expected_configuration_id: member.admission_configuration_id,
            })
            .collect(),
    };
    normalize_draft(&mut draft)?;
    validate_member_draft(&draft.members)?;
    mission.desired_outcome = draft.desired_outcome;
    mission.success_conditions = draft.success_conditions;
    mission.constraints = draft.constraints;
    mission.prohibitions = draft.prohibitions;
    Ok(())
}

fn render_mission(mission: StoredMissionAdmission) -> MissionView {
    MissionView {
        id: mission.id,
        workspace_id: mission.workspace_id,
        project_id: mission.project_id,
        charter_revision: mission.charter_revision,
        team_id: mission.team_id,
        desired_outcome: mission.desired_outcome,
        success_conditions: mission.success_conditions,
        constraints: mission.constraints,
        prohibitions: mission.prohibitions,
        lifecycle: mission.lifecycle,
        admitted_by: mission.admitted_by,
        admitted_at_ms: mission.admitted_at_ms,
        members: mission.members,
    }
}

fn parse_uuid(value: String) -> Result<Uuid, MissionError> {
    Uuid::parse_str(&value).map_err(|_| MissionError::InvalidStore)
}

fn map_project_error(error: ProjectError) -> MissionError {
    match error {
        ProjectError::Store(error) => MissionError::Store(error),
        ProjectError::Json(error) => MissionError::Json(error),
        _ => MissionError::InvalidStore,
    }
}

fn map_team_error(error: TeamError) -> MissionError {
    match error {
        TeamError::Store(error) => MissionError::Store(error),
        TeamError::Json(error) => MissionError::Json(error),
        _ => MissionError::InvalidStore,
    }
}

/// What one admitted Mission tells a caller that must bind to it: the Project it
/// belongs to, and the immutable admission Configuration recorded for one Resident —
/// `None` when that Resident is not on this Mission's admission roster.
///
/// A Proposal names Project, Mission, Resident, and Configuration redundantly so a
/// cross-Project or cross-Resident substitution is detectable without following one
/// identity to discover the others. This is one bounded read rather than four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MissionAdmissionFacts {
    pub(crate) project_id: Uuid,
    pub(crate) member_configuration_id: Option<Uuid>,
}

pub(crate) fn mission_admission_facts(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    resident_id: Uuid,
) -> Result<Option<MissionAdmissionFacts>, MissionError> {
    let Some(project_id) = connection
        .query_row(
            "SELECT project_id FROM missions WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(None);
    };
    let member_configuration_id = connection
        .query_row(
            "SELECT configuration_id FROM mission_admission_members
             WHERE workspace_id = ?1 AND mission_id = ?2 AND resident_id = ?3",
            params![
                workspace_id.to_string(),
                mission_id.to_string(),
                resident_id.to_string()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(parse_uuid)
        .transpose()?;
    Ok(Some(MissionAdmissionFacts {
        project_id: parse_uuid(project_id)?,
        member_configuration_id,
    }))
}

/// The Project a Mission belongs to, proved rather than read off a column.
///
/// Startup already re-proves every Mission against its own `mission.admitted` Event; this is the
/// same proof, on demand, for a reader that must not take `missions.project_id` on trust. It finds
/// the one Event that admitted this Mission, runs the same whole-payload comparison
/// `verify_mission_event` runs, and only then returns the Project the verified record names.
///
/// `Ok(None)` means this Workspace admitted no such Mission -- a different fact from a Mission whose
/// record and Event disagree, which is `InvalidStore`.
///
/// Added for sol1's #5: `mission_completions` has no composite key binding a Mission to a Project,
/// so a completion could name Mission A and Project B and satisfy the schema. The completion reader
/// asks this instead of joining.
pub(crate) fn verified_mission_project(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<Uuid>, MissionError> {
    Ok(
        verified_mission_declaration(connection, workspace_id, mission_id)?
            .map(|declaration| declaration.project_id),
    )
}

/// What a Mission declared, proved against its own admission Event.
///
/// The Outcome Document prints the Mission's title and each declared condition's statement, and
/// gpt2 §2 requires that text to come through the owner's verified reader rather than off a column.
/// `missions.desired_outcome` and `missions.success_conditions_json` are columns; this is the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MissionDeclaration {
    pub(crate) project_id: Uuid,
    pub(crate) desired_outcome: String,
    pub(crate) success_conditions: Vec<String>,
}

/// The Mission's declaration, proved the way startup proves it: the stored row must equal the
/// `mission.admitted` Event payload that recorded it, whole. `Ok(None)` means this Workspace admitted
/// no such Mission; a row and Event that disagree are `InvalidStore`.
pub(crate) fn verified_mission_declaration(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<Option<MissionDeclaration>, MissionError> {
    let Some(stored) = load_stored_mission(connection, workspace_id, mission_id)? else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT payload_json, occurred_at_ms FROM events
         WHERE workspace_id = ?1 AND kind = 'mission.admitted'
           AND json_extract(payload_json, '$.mission.id') = ?2",
    )?;
    let events: Vec<(String, i64)> = statement
        .query_map(
            params![workspace_id.to_string(), mission_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    // Exactly one. None is a Mission the log does not account for; two would make "when was this
    // Mission admitted" a question with two answers.
    let [(payload_json, occurred_at_ms)] = events.as_slice() else {
        return Err(MissionError::InvalidStore);
    };
    let occurred_at_ms = u64::try_from(*occurred_at_ms).map_err(|_| MissionError::InvalidStore)?;
    verify_mission_event(connection, workspace_id, occurred_at_ms, payload_json)?;
    Ok(Some(MissionDeclaration {
        project_id: stored.project_id,
        desired_outcome: stored.desired_outcome,
        success_conditions: stored.success_conditions,
    }))
}

pub(crate) fn verify_mission_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    payload_json: &str,
) -> Result<(), MissionError> {
    let payload: MissionAdmittedPayload = serde_json::from_str(payload_json)?;
    if payload.mission.workspace_id != workspace_id
        || payload.mission.admitted_at_ms != occurred_at_ms
        || load_stored_mission(connection, workspace_id, payload.mission.id)?
            != Some(payload.mission)
    {
        return Err(MissionError::InvalidStore);
    }
    Ok(())
}

/// Re-derive `mission_success_conditions` from the Mission's own bytes, and refuse on disagreement.
///
/// The table is a projection of `missions.success_conditions_json`, and a projection that can drift
/// from its source is a second place to be wrong about what a Mission declared. There is no store
/// trigger comparing an inserted row against the JSON, deliberately: it would make a disagreeing row
/// unrepresentable and this check dead code, and a check that cannot fail is a sentence rather than
/// a guard. So the guard is here, and its tests write the bad rows directly.
///
/// **Two checks, not one.** The count and the text answer different questions and are separated so
/// neither can hide the other: a row whose text was rewritten leaves the count correct, and a row
/// appended past the end of the array leaves every text correct. Deriving both from one comparison
/// -- rebuild the expected list, compare the whole set -- would pass both scenarios through one
/// assertion, and a single mutant would kill a test that is supposed to be discriminating.
fn verify_mission_success_conditions(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
    declared: &[String],
) -> Result<(), MissionError> {
    // 1. The count, against the array's length. Catches a row the Mission never declared, which is
    //    the one shape a per-ordinal walk of the array cannot see.
    let stored: i64 = connection.query_row(
        "SELECT count(*) FROM mission_success_conditions
         WHERE workspace_id = ?1 AND mission_id = ?2",
        params![workspace_id.to_string(), mission_id.to_string()],
        |row| row.get(0),
    )?;
    if usize::try_from(stored) != Ok(declared.len()) {
        return Err(MissionError::InvalidStore);
    }
    // 2. The text, per ordinal, against the Mission's own bytes. Catches a row rewritten in place --
    //    which the count cannot see, because rewriting one is not adding one.
    for (ordinal, text) in declared.iter().enumerate() {
        let ordinal = i64::try_from(ordinal).map_err(|_| MissionError::InvalidStore)?;
        let row: Option<String> = connection
            .query_row(
                "SELECT text FROM mission_success_conditions
                 WHERE workspace_id = ?1 AND mission_id = ?2 AND ordinal = ?3",
                params![workspace_id.to_string(), mission_id.to_string(), ordinal],
                |row| row.get(0),
            )
            .optional()?;
        if row.as_deref() != Some(text.as_str()) {
            return Err(MissionError::InvalidStore);
        }
    }
    Ok(())
}

pub(crate) fn verify_mission_state(connection: &Connection) -> Result<(), MissionStateFault> {
    let mut statement = connection
        .prepare("SELECT workspace_id, id FROM missions ORDER BY workspace_id, id")
        .map_err(MissionStateFault::Store)?;
    let ids = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(MissionStateFault::Store)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(MissionStateFault::Store)?;
    for (workspace_id, mission_id) in &ids {
        let identity = || MissionStateFault::Identity {
            workspace_id: workspace_id.clone(),
            mission_id: mission_id.clone(),
        };
        let parsed_workspace = Uuid::parse_str(workspace_id).map_err(|_| identity())?;
        let parsed_mission = Uuid::parse_str(mission_id).map_err(|_| identity())?;
        verify_one_mission(connection, parsed_workspace, parsed_mission).map_err(|cause| {
            MissionStateFault::Row {
                workspace_id: parsed_workspace,
                mission_id: parsed_mission,
                cause,
            }
        })?;
    }
    // Whole-store checks: no row to name, so they are reported as the store fault they are.
    verify_mission_totals(connection, ids.len())?;
    Ok(())
}

/// One stored Mission, re-proved. Unchanged from what the loop held inline.
fn verify_one_mission(
    connection: &Connection,
    workspace_id: Uuid,
    mission_id: Uuid,
) -> Result<(), MissionError> {
    {
        let mission = load_stored_mission(connection, workspace_id, mission_id)?
            .ok_or(MissionError::InvalidStore)?;
        let event_count = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = 'mission.admitted'
               AND json_extract(payload_json, '$.mission.id') = ?2",
            params![workspace_id.to_string(), mission.id.to_string()],
            |row| row.get::<_, i64>(0),
        )?;
        if event_count != 1 {
            return Err(MissionError::InvalidStore);
        }
        verify_mission_success_conditions(
            connection,
            workspace_id,
            mission_id,
            &mission.success_conditions,
        )?;
    }
    Ok(())
}

/// The two checks that are about the STORE rather than about one Mission, so they have no row to
/// name and stay whole-store faults.
fn verify_mission_totals(
    connection: &Connection,
    mission_count: usize,
) -> Result<(), MissionStateFault> {
    // A condition row for a Mission that does not exist cannot be stored -- the composite foreign
    // key forbids it -- so walking the Missions reaches every row there is.
    let orphan_conditions: i64 = connection
        .query_row(
            "SELECT count(*) FROM mission_success_conditions AS conditions
         WHERE NOT EXISTS (
             SELECT 1 FROM missions
             WHERE missions.workspace_id = conditions.workspace_id
               AND missions.id = conditions.mission_id
         )",
            [],
            |row| row.get(0),
        )
        .map_err(MissionStateFault::Store)?;
    if orphan_conditions != 0 {
        return Err(MissionStateFault::Totals(crate::TotalsMismatch {
            table: "mission_success_conditions (rows whose Mission does not exist)",
            expected: 0,
            observed: orphan_conditions.unsigned_abs(),
        }));
    }
    let event_count = connection
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'mission.admitted'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(MissionStateFault::Store)?;
    if usize::try_from(event_count) != Ok(mission_count) {
        return Err(MissionStateFault::Totals(crate::TotalsMismatch {
            table: "events (mission.admitted)",
            expected: mission_count as u64,
            observed: event_count.unsigned_abs(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()`; see the note in `outcome_publication`'s twin.
    fn expected_name(error: &MissionError) -> &'static str {
        match error {
            MissionError::Account(_) => "Account",
            MissionError::ProjectNotFound => "ProjectNotFound",
            MissionError::TeamNotFound => "TeamNotFound",
            MissionError::MissionNotFound => "MissionNotFound",
            MissionError::StaleCharterRevision { .. } => "StaleCharterRevision",
            MissionError::InvalidDesiredOutcome => "InvalidDesiredOutcome",
            MissionError::InvalidSuccessConditions => "InvalidSuccessConditions",
            MissionError::InvalidConstraint => "InvalidConstraint",
            MissionError::InvalidProhibition => "InvalidProhibition",
            MissionError::DuplicateMissionEntry => "DuplicateMissionEntry",
            MissionError::InvalidMembers => "InvalidMembers",
            MissionError::StaleTeamEligibility => "StaleTeamEligibility",
            MissionError::ResidentIneligible(_) => "ResidentIneligible",
            MissionError::InvalidStore => "InvalidStore",
            MissionError::Store(_) => "Store",
            MissionError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_mission_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            MissionError::Account(OperatorAccountError::PasswordTooShort),
            MissionError::ProjectNotFound,
            MissionError::TeamNotFound,
            MissionError::MissionNotFound,
            MissionError::StaleCharterRevision {
                expected: 1,
                current: 2,
            },
            MissionError::InvalidDesiredOutcome,
            MissionError::InvalidSuccessConditions,
            MissionError::InvalidConstraint,
            MissionError::InvalidProhibition,
            MissionError::DuplicateMissionEntry,
            MissionError::InvalidMembers,
            MissionError::StaleTeamEligibility,
            MissionError::ResidentIneligible(Uuid::nil()),
            MissionError::InvalidStore,
            MissionError::Store(rusqlite::Error::QueryReturnedNoRows),
            MissionError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            16,
            "an arm was added to `MissionError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> =
            every_arm.iter().map(MissionError::name).collect();
        assert_eq!(
            names.len(),
            16,
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
