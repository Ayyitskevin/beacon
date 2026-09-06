use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{OperatorAccountError, ResidentLifecycle};

const PROPOSAL_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StarterRole {
    Lead,
    Builder,
    Reviewer,
    Researcher,
}

impl StarterRole {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lead => "lead",
            Self::Builder => "builder",
            Self::Reviewer => "reviewer",
            Self::Researcher => "researcher",
        }
    }

    pub(crate) fn parse(value: &str) -> Result<Self, StarterOrganizationError> {
        match value {
            "lead" => Ok(Self::Lead),
            "builder" => Ok(Self::Builder),
            "reviewer" => Ok(Self::Reviewer),
            "researcher" => Ok(Self::Researcher),
            _ => Err(StarterOrganizationError::InvalidStore),
        }
    }

    pub(crate) fn ordinal(self) -> u8 {
        match self {
            Self::Lead => 0,
            Self::Builder => 1,
            Self::Reviewer => 2,
            Self::Researcher => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StarterOrganizationDecision {
    Proposed,
    Accepted,
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StarterRoleTemplate {
    pub role: StarterRole,
    pub description: String,
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StarterResidentDraft {
    pub role: StarterRole,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidentAgentSummary {
    pub id: Uuid,
    pub role: StarterRole,
    pub display_name: Option<String>,
    pub lifecycle: ResidentLifecycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StarterOrganizationView {
    pub workspace_id: Uuid,
    pub proposal_version: u16,
    pub decision: StarterOrganizationDecision,
    pub templates: Vec<StarterRoleTemplate>,
    pub residents: Vec<ResidentAgentSummary>,
}

/// Which fault the starter-organization startup pass hit, and WHICH ROW hit it (R28.1).
///
/// The row is a Workspace -- this pass walks `workspaces` -- so the key has no second field.
pub(crate) enum StarterStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One Workspace's starter organization did not re-prove.
    Row {
        workspace_id: Uuid,
        cause: StarterOrganizationError,
    },
    /// A row whose id is not a UUID. Unreachable while the Workspace pass runs first.
    Identity { workspace_id: String },
}

impl StarterOrganizationError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::UnsupportedProposalVersion => "UnsupportedProposalVersion",
            Self::EmptyRoster => "EmptyRoster",
            Self::DuplicateRole => "DuplicateRole",
            Self::InvalidDisplayName => "InvalidDisplayName",
            Self::DecisionAlreadyRecorded => "DecisionAlreadyRecorded",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for StarterStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the starter organizations could not be read"),
            Self::Row {
                workspace_id,
                cause,
            } => write!(
                formatter,
                "the starter organization of Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity { workspace_id } => {
                write!(formatter, "a Workspace id is not readable: {workspace_id}")
            }
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for StarterStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("cause", cause)
                .finish(),
            Self::Identity { workspace_id } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .finish(),
        }
    }
}

impl std::error::Error for StarterStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum StarterOrganizationError {
    Account(OperatorAccountError),
    UnsupportedProposalVersion,
    EmptyRoster,
    DuplicateRole,
    InvalidDisplayName,
    DecisionAlreadyRecorded,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for StarterOrganizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::UnsupportedProposalVersion => {
                write!(
                    formatter,
                    "starter organization proposal is no longer current"
                )
            }
            Self::EmptyRoster => write!(formatter, "select at least one Resident or discard"),
            Self::DuplicateRole => write!(formatter, "each starter role may be selected once"),
            Self::InvalidDisplayName => write!(formatter, "Resident Display Name is invalid"),
            Self::DecisionAlreadyRecorded => {
                write!(
                    formatter,
                    "starter organization decision is already recorded"
                )
            }
            Self::InvalidStore => write!(formatter, "starter organization state is invalid"),
            Self::Store(error) => write!(formatter, "starter organization store failed: {error}"),
            Self::Json(error) => {
                write!(formatter, "starter organization event is invalid: {error}")
            }
        }
    }
}

impl std::error::Error for StarterOrganizationError {}

impl From<OperatorAccountError> for StarterOrganizationError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for StarterOrganizationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for StarterOrganizationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AcceptedPayload {
    workspace_id: Uuid,
    proposal_version: u16,
    residents: Vec<ResidentAgentSummary>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiscardedPayload {
    workspace_id: Uuid,
    proposal_version: u16,
}

/// Owns the first-run roster proposal and its one-time decision behind one authenticated interface.
pub struct StarterOrganizationService {
    accounts: Arc<crate::OperatorAccountService>,
    workspace_id: Uuid,
}

impl StarterOrganizationService {
    pub fn new(accounts: Arc<crate::OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn view(&self) -> Result<StarterOrganizationView, StarterOrganizationError> {
        self.accounts
            .with_authenticated_store(|connection, _now| load_view(connection, self.workspace_id))
    }

    pub fn accept(
        &self,
        proposal_version: u16,
        mut residents: Vec<StarterResidentDraft>,
    ) -> Result<StarterOrganizationView, StarterOrganizationError> {
        if proposal_version != PROPOSAL_VERSION {
            return Err(StarterOrganizationError::UnsupportedProposalVersion);
        }
        if residents.is_empty() {
            return Err(StarterOrganizationError::EmptyRoster);
        }
        let mut roles = HashSet::new();
        for resident in &mut residents {
            if !roles.insert(resident.role) {
                return Err(StarterOrganizationError::DuplicateRole);
            }
            resident.display_name = normalize_display_name(resident.display_name.take())?;
        }
        residents.sort_by_key(|resident| resident.role.ordinal());

        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if decision_exists(&transaction, self.workspace_id)? {
                return Err(StarterOrganizationError::DecisionAlreadyRecorded);
            }
            transaction.execute(
                "INSERT INTO starter_organization_decisions (
                    workspace_id, proposal_version, decision, decided_at_ms
                 ) VALUES (?1, ?2, 'accepted', ?3)",
                params![
                    self.workspace_id.to_string(),
                    i64::from(PROPOSAL_VERSION),
                    now
                ],
            )?;
            let mut accepted = Vec::with_capacity(residents.len());
            for resident in residents {
                let id = Uuid::new_v4();
                let (display_name_state, display_name) = match resident.display_name {
                    Some(value) => ("set", value),
                    None => ("absent", String::new()),
                };
                transaction.execute(
                    "INSERT INTO agents (
                        id, workspace_id, kind, admission_source, role,
                        display_name_state, display_name, lifecycle, created_at_ms, updated_at_ms
                     ) VALUES (
                        ?1, ?2, 'resident', 'starter_organization', ?3,
                        ?4, ?5, 'inactive', ?6, ?6
                     )",
                    params![
                        id.to_string(),
                        self.workspace_id.to_string(),
                        resident.role.as_str(),
                        display_name_state,
                        display_name,
                        now
                    ],
                )?;
                transaction.execute(
                    "INSERT INTO starter_organization_admissions (
                        workspace_id, agent_id, role, display_name_state, display_name,
                        admitted_at_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        self.workspace_id.to_string(),
                        id.to_string(),
                        resident.role.as_str(),
                        display_name_state,
                        display_name,
                        now
                    ],
                )?;
                accepted.push(ResidentAgentSummary {
                    id,
                    role: resident.role,
                    display_name: if display_name_state == "set" {
                        Some(display_name)
                    } else {
                        None
                    },
                    lifecycle: ResidentLifecycle::Inactive,
                });
            }
            let payload = serde_json::to_string(&AcceptedPayload {
                workspace_id: self.workspace_id,
                proposal_version: PROPOSAL_VERSION,
                residents: accepted,
            })?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'starter_organization.accepted', ?2, ?3)",
                params![self.workspace_id.to_string(), payload, now],
            )?;
            transaction.commit()?;
            load_view(connection, self.workspace_id)
        })
    }

    pub fn discard(
        &self,
        proposal_version: u16,
    ) -> Result<StarterOrganizationView, StarterOrganizationError> {
        if proposal_version != PROPOSAL_VERSION {
            return Err(StarterOrganizationError::UnsupportedProposalVersion);
        }
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if decision_exists(&transaction, self.workspace_id)? {
                return Err(StarterOrganizationError::DecisionAlreadyRecorded);
            }
            transaction.execute(
                "INSERT INTO starter_organization_decisions (
                    workspace_id, proposal_version, decision, decided_at_ms
                 ) VALUES (?1, ?2, 'discarded', ?3)",
                params![
                    self.workspace_id.to_string(),
                    i64::from(PROPOSAL_VERSION),
                    now
                ],
            )?;
            let payload = serde_json::to_string(&DiscardedPayload {
                workspace_id: self.workspace_id,
                proposal_version: PROPOSAL_VERSION,
            })?;
            transaction.execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'starter_organization.discarded', ?2, ?3)",
                params![self.workspace_id.to_string(), payload, now],
            )?;
            transaction.commit()?;
            load_view(connection, self.workspace_id)
        })
    }
}

pub(crate) fn normalize_display_name(
    display_name: Option<String>,
) -> Result<Option<String>, StarterOrganizationError> {
    let Some(value) = display_name else {
        return Ok(None);
    };
    if value.chars().any(char::is_control) {
        return Err(StarterOrganizationError::InvalidDisplayName);
    }
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 {
        return Err(StarterOrganizationError::InvalidDisplayName);
    }
    Ok(Some(value.to_owned()))
}

fn decision_exists(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
) -> Result<bool, StarterOrganizationError> {
    connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM starter_organization_decisions WHERE workspace_id = ?1
             )",
            [workspace_id.to_string()],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn load_view(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
) -> Result<StarterOrganizationView, StarterOrganizationError> {
    let decision = connection
        .query_row(
            "SELECT proposal_version, decision
             FROM starter_organization_decisions
             WHERE workspace_id = ?1",
            [workspace_id.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let decision = match decision {
        None => StarterOrganizationDecision::Proposed,
        Some((version, value)) if version == i64::from(PROPOSAL_VERSION) => match value.as_str() {
            "accepted" => StarterOrganizationDecision::Accepted,
            "discarded" => StarterOrganizationDecision::Discarded,
            _ => return Err(StarterOrganizationError::InvalidStore),
        },
        Some(_) => return Err(StarterOrganizationError::InvalidStore),
    };

    let residents = load_admissions(connection, workspace_id)?;
    match decision {
        StarterOrganizationDecision::Proposed | StarterOrganizationDecision::Discarded
            if !residents.is_empty() =>
        {
            return Err(StarterOrganizationError::InvalidStore);
        }
        StarterOrganizationDecision::Accepted if residents.is_empty() => {
            return Err(StarterOrganizationError::InvalidStore);
        }
        _ => {}
    }

    Ok(StarterOrganizationView {
        workspace_id,
        proposal_version: PROPOSAL_VERSION,
        decision,
        templates: starter_templates(),
        residents,
    })
}

pub(crate) fn verify_starter_event(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
    accepted: bool,
    payload_json: &str,
) -> Result<(), StarterOrganizationError> {
    let expected_decision = if accepted { "accepted" } else { "discarded" };
    let decision: Option<(i64, String)> = connection
        .query_row(
            "SELECT proposal_version, decision
             FROM starter_organization_decisions
             WHERE workspace_id = ?1",
            [workspace_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if decision != Some((i64::from(PROPOSAL_VERSION), expected_decision.to_owned())) {
        return Err(StarterOrganizationError::InvalidStore);
    }

    if accepted {
        let payload: AcceptedPayload = serde_json::from_str(payload_json)?;
        if payload.workspace_id != workspace_id
            || payload.proposal_version != PROPOSAL_VERSION
            || payload.residents != load_admissions(connection, workspace_id)?
        {
            return Err(StarterOrganizationError::InvalidStore);
        }
    } else {
        let payload: DiscardedPayload = serde_json::from_str(payload_json)?;
        if payload.workspace_id != workspace_id
            || payload.proposal_version != PROPOSAL_VERSION
            || !load_admissions(connection, workspace_id)?.is_empty()
        {
            return Err(StarterOrganizationError::InvalidStore);
        }
    }
    Ok(())
}

/// The rows `verify_starter_state` enumerates, in the order it enumerates them.
///
/// EXTRACTED SO A BAR CAN CALL IT (R44.6): a placement assertion that COPIES this query agrees
/// with itself, so flipping the pass's own `ORDER BY` leaves it decorative. One function, called
/// by both, moves the pass and the bar's view of it together.
pub fn enumerate_starter_workspaces(
    connection: &rusqlite::Connection,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare("SELECT id FROM workspaces ORDER BY id")?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_starter_state(
    connection: &rusqlite::Connection,
) -> Result<(), StarterStateFault> {
    let workspace_ids =
        enumerate_starter_workspaces(connection).map_err(StarterStateFault::Store)?;

    for workspace_id in workspace_ids {
        let parsed = Uuid::parse_str(&workspace_id).map_err(|_| StarterStateFault::Identity {
            workspace_id: workspace_id.clone(),
        })?;
        verify_one_starter_organization(connection, parsed).map_err(|cause| {
            StarterStateFault::Row {
                workspace_id: parsed,
                cause,
            }
        })?;
    }
    Ok(())
}

/// One Workspace's starter organization, re-proved. Unchanged from what the loop held inline.
fn verify_one_starter_organization(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
) -> Result<(), StarterOrganizationError> {
    {
        let view = load_view(connection, workspace_id)?;
        let (accepted_events, discarded_events): (i64, i64) = connection.query_row(
            "SELECT
                count(*) FILTER (WHERE kind = 'starter_organization.accepted'),
                count(*) FILTER (WHERE kind = 'starter_organization.discarded')
             FROM events
             WHERE workspace_id = ?1",
            [workspace_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let event_counts_match = match view.decision {
            StarterOrganizationDecision::Proposed => accepted_events == 0 && discarded_events == 0,
            StarterOrganizationDecision::Accepted => accepted_events == 1 && discarded_events == 0,
            StarterOrganizationDecision::Discarded => accepted_events == 0 && discarded_events == 1,
        };
        let starter_agent_count: i64 = connection.query_row(
            "SELECT count(*) FROM agents
             WHERE workspace_id = ?1 AND admission_source = 'starter_organization'",
            [workspace_id.to_string()],
            |row| row.get(0),
        )?;
        if !event_counts_match
            || starter_agent_count != i64::try_from(view.residents.len()).unwrap_or(i64::MAX)
        {
            return Err(StarterOrganizationError::InvalidStore);
        }
    }
    Ok(())
}

fn starter_templates() -> Vec<StarterRoleTemplate> {
    [
        (
            StarterRole::Lead,
            "Coordinates Missions and keeps work aligned with the Project Charter.",
            true,
        ),
        (
            StarterRole::Builder,
            "Produces implementation Artifacts within declared capability and budget.",
            true,
        ),
        (
            StarterRole::Reviewer,
            "Independently evaluates material work before it can support a Ruling.",
            true,
        ),
        (
            StarterRole::Researcher,
            "Develops Claims and Evidence without receiving execution authority.",
            false,
        ),
    ]
    .into_iter()
    .map(|(role, description, recommended)| StarterRoleTemplate {
        role,
        description: description.to_owned(),
        recommended,
    })
    .collect()
}

fn load_admissions(
    connection: &rusqlite::Connection,
    workspace_id: Uuid,
) -> Result<Vec<ResidentAgentSummary>, StarterOrganizationError> {
    let mut statement = connection.prepare(
        "SELECT admission.agent_id, admission.role, admission.display_name_state,
                admission.display_name
         FROM starter_organization_admissions AS admission
         JOIN agents AS agent
           ON agent.id = admission.agent_id
          AND agent.workspace_id = admission.workspace_id
          AND agent.kind = 'resident'
          AND agent.admission_source = 'starter_organization'
         WHERE admission.workspace_id = ?1
         ORDER BY CASE admission.role
             WHEN 'lead' THEN 0
             WHEN 'builder' THEN 1
             WHEN 'reviewer' THEN 2
             WHEN 'researcher' THEN 3
             ELSE 4
         END",
    )?;
    statement
        .query_map(params![workspace_id.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .map(|row| {
            let (id, role, display_name_state, display_name) = row?;
            let id = Uuid::parse_str(&id).map_err(|_| StarterOrganizationError::InvalidStore)?;
            let role = StarterRole::parse(&role)?;
            let display_name = match (display_name_state.as_str(), display_name.as_str()) {
                ("absent", "") => None,
                ("set", value) => {
                    let normalized = normalize_display_name(Some(value.to_owned()))?
                        .ok_or(StarterOrganizationError::InvalidStore)?;
                    if normalized != value {
                        return Err(StarterOrganizationError::InvalidStore);
                    }
                    Some(normalized)
                }
                _ => return Err(StarterOrganizationError::InvalidStore),
            };
            Ok(ResidentAgentSummary {
                id,
                role,
                display_name,
                lifecycle: ResidentLifecycle::Inactive,
            })
        })
        .collect()
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &StarterOrganizationError) -> &'static str {
        match error {
            StarterOrganizationError::Account(_) => "Account",
            StarterOrganizationError::UnsupportedProposalVersion => "UnsupportedProposalVersion",
            StarterOrganizationError::EmptyRoster => "EmptyRoster",
            StarterOrganizationError::DuplicateRole => "DuplicateRole",
            StarterOrganizationError::InvalidDisplayName => "InvalidDisplayName",
            StarterOrganizationError::DecisionAlreadyRecorded => "DecisionAlreadyRecorded",
            StarterOrganizationError::InvalidStore => "InvalidStore",
            StarterOrganizationError::Store(_) => "Store",
            StarterOrganizationError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_starter_organization_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            StarterOrganizationError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            StarterOrganizationError::UnsupportedProposalVersion,
            StarterOrganizationError::EmptyRoster,
            StarterOrganizationError::DuplicateRole,
            StarterOrganizationError::InvalidDisplayName,
            StarterOrganizationError::DecisionAlreadyRecorded,
            StarterOrganizationError::InvalidStore,
            StarterOrganizationError::Store(rusqlite::Error::QueryReturnedNoRows),
            StarterOrganizationError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            9,
            "an arm was added to `StarterOrganizationError` without a fixture here"
        );
        for arm in &every_arm {
            assert_eq!(
                arm.name(),
                expected_name(arm),
                "the startup name does not match the pinned table: {arm:?}"
            );
        }
        let names: std::collections::BTreeSet<&str> = every_arm
            .iter()
            .map(StarterOrganizationError::name)
            .collect();
        assert_eq!(
            names.len(),
            9,
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
