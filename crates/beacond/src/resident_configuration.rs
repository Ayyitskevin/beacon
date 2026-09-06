use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::local_model_activation::{activation_by_id, latest_activation_for_resource};
use crate::resident_lifecycle::load_resident;
use crate::{
    BudgetBoundary, LocalModelActivationError, LocalModelActivationState, OperatorAccountError,
    OperatorAccountService, ProviderFeature, ResidentLifecycle, ResidentLifecycleError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum BeaconCapability {
    #[serde(rename = "work.create_mission")]
    WorkCreateMission,
    #[serde(rename = "work.form_team")]
    WorkFormTeam,
    #[serde(rename = "work.coordinate_mission")]
    WorkCoordinateMission,
    #[serde(rename = "work.assign_work")]
    WorkAssignWork,
    #[serde(rename = "artifact.produce")]
    ArtifactProduce,
    #[serde(rename = "artifact.evaluate")]
    ArtifactEvaluate,
    #[serde(rename = "evidence.develop")]
    EvidenceDevelop,
}

impl BeaconCapability {
    pub const ALL: [Self; 7] = [
        Self::WorkCreateMission,
        Self::WorkFormTeam,
        Self::WorkCoordinateMission,
        Self::WorkAssignWork,
        Self::ArtifactProduce,
        Self::ArtifactEvaluate,
        Self::EvidenceDevelop,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::WorkCreateMission => "work.create_mission",
            Self::WorkFormTeam => "work.form_team",
            Self::WorkCoordinateMission => "work.coordinate_mission",
            Self::WorkAssignWork => "work.assign_work",
            Self::ArtifactProduce => "artifact.produce",
            Self::ArtifactEvaluate => "artifact.evaluate",
            Self::EvidenceDevelop => "evidence.develop",
        }
    }

    fn parse(value: &str) -> Result<Self, ResidentConfigurationError> {
        match value {
            "work.create_mission" => Ok(Self::WorkCreateMission),
            "work.form_team" => Ok(Self::WorkFormTeam),
            "work.coordinate_mission" => Ok(Self::WorkCoordinateMission),
            "work.assign_work" => Ok(Self::WorkAssignWork),
            "artifact.produce" => Ok(Self::ArtifactProduce),
            "artifact.evaluate" => Ok(Self::ArtifactEvaluate),
            "evidence.develop" => Ok(Self::EvidenceDevelop),
            _ => Err(ResidentConfigurationError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeaconCapabilityDescriptor {
    pub capability: BeaconCapability,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidentConfigurationState {
    Configured,
    Cleared,
}

impl ResidentConfigurationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Configured => "configured",
            Self::Cleared => "cleared",
        }
    }

    fn parse(value: &str) -> Result<Self, ResidentConfigurationError> {
        match value {
            "configured" => Ok(Self::Configured),
            "cleared" => Ok(Self::Cleared),
            _ => Err(ResidentConfigurationError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidentConfigurationBinding {
    pub activation_id: Uuid,
    pub capabilities: Vec<BeaconCapability>,
    pub budget_boundary: BudgetBoundary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamIneligibilityReason {
    ConfigurationCleared,
    ResidentInactive,
    ResidentSuspended,
    ResidentRetired,
    ModelActivationUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResidentConfiguration {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub resident_id: Uuid,
    pub state: ResidentConfigurationState,
    pub binding: Option<ResidentConfigurationBinding>,
    pub predecessor_id: Option<Uuid>,
    pub decided_at_ms: u64,
    pub eligible_for_team: bool,
    pub ineligibility_reasons: Vec<TeamIneligibilityReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredResidentConfigurationDecision {
    id: Uuid,
    workspace_id: Uuid,
    resident_id: Uuid,
    state: ResidentConfigurationState,
    binding: Option<ResidentConfigurationBinding>,
    predecessor_id: Option<Uuid>,
    decided_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ResidentConfigurationDecisionPayload {
    decision: StoredResidentConfigurationDecision,
}

/// Which fault the ResidentConfiguration startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum ResidentConfigurationStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One ResidentConfiguration did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        decision_id: String,
        cause: ResidentConfigurationError,
    },
    // NO `Totals` ARM. Every check here belongs to one decision and its own Event.
    /// A row whose Workspace column is not a UUID. Unreachable while the foreign-key check and
    /// the Workspace pass run first.
    Identity {
        workspace_id: String,
        decision_id: String,
    },
    /// A decision whose stored ordering key is outside the positive domain the service writes.
    Sequence {
        workspace_id: String,
        decision_id: String,
        sequence: i64,
    },
}

impl ResidentConfigurationError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ResidentNotFound => "ResidentNotFound",
            Self::ResidentRetired => "ResidentRetired",
            Self::ModelActivationUnavailable => "ModelActivationUnavailable",
            Self::ModelMissingCompletion => "ModelMissingCompletion",
            Self::InvalidCapabilities => "InvalidCapabilities",
            Self::StaleDecision => "StaleDecision",
            Self::AlreadyCleared => "AlreadyCleared",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for ResidentConfigurationStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the ResidentConfigurations could not be read"),
            Self::Row {
                workspace_id,
                decision_id,
                cause,
            } => write!(
                formatter,
                "ResidentConfiguration {decision_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Identity {
                workspace_id,
                decision_id,
            } => write!(
                formatter,
                "a ResidentConfiguration row's Workspace is not readable: {workspace_id}, ResidentConfiguration \
                 {decision_id}"
            ),
            Self::Sequence { .. } => formatter
                .write_str("a Resident configuration decision's ordering key is not readable"),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for ResidentConfigurationStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                decision_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("decision_id", decision_id)
                .field("cause", cause)
                .finish(),
            Self::Identity {
                workspace_id,
                decision_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("decision_id", decision_id)
                .finish(),
            Self::Sequence {
                workspace_id,
                decision_id,
                sequence,
            } => formatter
                .debug_struct("Sequence")
                .field("workspace_id", workspace_id)
                .field("decision_id", decision_id)
                .field("sequence", sequence)
                .finish(),
        }
    }
}

impl std::error::Error for ResidentConfigurationStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } | Self::Sequence { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum ResidentConfigurationError {
    Account(OperatorAccountError),
    ResidentNotFound,
    ResidentRetired,
    ModelActivationUnavailable,
    ModelMissingCompletion,
    InvalidCapabilities,
    StaleDecision,
    AlreadyCleared,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ResidentConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ResidentNotFound => write!(formatter, "Resident was not found"),
            Self::ResidentRetired => write!(formatter, "retired Resident cannot be configured"),
            Self::ModelActivationUnavailable => {
                write!(
                    formatter,
                    "local-model activation is not currently available"
                )
            }
            Self::ModelMissingCompletion => {
                write!(
                    formatter,
                    "local-model activation lacks approved completion"
                )
            }
            Self::InvalidCapabilities => write!(formatter, "Capability selection is invalid"),
            Self::StaleDecision => write!(formatter, "Resident configuration decision is stale"),
            Self::AlreadyCleared => write!(formatter, "Resident configuration is already cleared"),
            Self::InvalidStore => write!(formatter, "Resident configuration state is invalid"),
            Self::Store(error) => write!(formatter, "Resident configuration store failed: {error}"),
            Self::Json(error) => write!(
                formatter,
                "Resident configuration event is invalid: {error}"
            ),
        }
    }
}

impl std::error::Error for ResidentConfigurationError {}

impl From<OperatorAccountError> for ResidentConfigurationError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for ResidentConfigurationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for ResidentConfigurationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns explicit Resident model, Capability, and Alpha budget configuration decisions.
pub struct ResidentConfigurationService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
}

impl ResidentConfigurationService {
    pub fn new(accounts: Arc<OperatorAccountService>, workspace_id: Uuid) -> Self {
        Self {
            accounts,
            workspace_id,
        }
    }

    pub fn catalog(&self) -> Result<Vec<BeaconCapabilityDescriptor>, ResidentConfigurationError> {
        self.accounts.with_authenticated_store(|_connection, _now| {
            Ok(BeaconCapability::ALL
                .into_iter()
                .map(capability_descriptor)
                .collect())
        })
    }

    pub fn list(&self) -> Result<Vec<ResidentConfiguration>, ResidentConfigurationError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            list_latest_configurations(connection, self.workspace_id)
        })
    }

    pub fn configure(
        &self,
        resident_id: Uuid,
        expected_decision_id: Option<Uuid>,
        activation_id: Uuid,
        capabilities: Vec<BeaconCapability>,
    ) -> Result<ResidentConfiguration, ResidentConfigurationError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let capabilities = normalize_capabilities(capabilities)?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let resident = load_resident(&transaction, self.workspace_id, resident_id)
                .map_err(map_resident_error)?
                .ok_or(ResidentConfigurationError::ResidentNotFound)?;
            if resident.lifecycle == ResidentLifecycle::Retired {
                return Err(ResidentConfigurationError::ResidentRetired);
            }
            let predecessor = latest_configuration(&transaction, self.workspace_id, resident_id)?;
            require_expected(predecessor.as_ref(), expected_decision_id)?;
            let activation = activation_by_id(&transaction, self.workspace_id, activation_id)
                .map_err(map_activation_error)?
                .ok_or(ResidentConfigurationError::ModelActivationUnavailable)?;
            let latest = latest_activation_for_resource(
                &transaction,
                self.workspace_id,
                &activation.binding,
            )
            .map_err(map_activation_error)?;
            if activation.state != LocalModelActivationState::Active
                || latest.as_ref().map(|candidate| candidate.id) != Some(activation.id)
            {
                return Err(ResidentConfigurationError::ModelActivationUnavailable);
            }
            if !activation
                .approved_features
                .contains(&ProviderFeature::Completion)
            {
                return Err(ResidentConfigurationError::ModelMissingCompletion);
            }
            let decision = StoredResidentConfigurationDecision {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                resident_id,
                state: ResidentConfigurationState::Configured,
                binding: Some(ResidentConfigurationBinding {
                    activation_id,
                    capabilities,
                    budget_boundary: BudgetBoundary::NoSpend,
                }),
                predecessor_id: predecessor.as_ref().map(|decision| decision.id),
                decided_at_ms: monotonic_time(now, predecessor.as_ref())?,
            };
            insert_decision(&transaction, &decision)?;
            let view = render_configuration(&transaction, decision)?;
            transaction.commit()?;
            Ok(view)
        })
    }

    pub fn clear(
        &self,
        resident_id: Uuid,
        expected_decision_id: Uuid,
    ) -> Result<ResidentConfiguration, ResidentConfigurationError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let resident = load_resident(&transaction, self.workspace_id, resident_id)
                .map_err(map_resident_error)?
                .ok_or(ResidentConfigurationError::ResidentNotFound)?;
            if resident.lifecycle == ResidentLifecycle::Retired {
                return Err(ResidentConfigurationError::ResidentRetired);
            }
            let predecessor = latest_configuration(&transaction, self.workspace_id, resident_id)?
                .ok_or(ResidentConfigurationError::StaleDecision)?;
            require_expected(Some(&predecessor), Some(expected_decision_id))?;
            if predecessor.state == ResidentConfigurationState::Cleared {
                return Err(ResidentConfigurationError::AlreadyCleared);
            }
            let decision = StoredResidentConfigurationDecision {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                resident_id,
                state: ResidentConfigurationState::Cleared,
                binding: None,
                predecessor_id: Some(predecessor.id),
                decided_at_ms: monotonic_time(now, Some(&predecessor))?,
            };
            insert_decision(&transaction, &decision)?;
            let view = render_configuration(&transaction, decision)?;
            transaction.commit()?;
            Ok(view)
        })
    }
}

fn capability_descriptor(capability: BeaconCapability) -> BeaconCapabilityDescriptor {
    let (label, description) = match capability {
        BeaconCapability::WorkCreateMission => (
            "Create Missions",
            "Request creation of a Charter-aligned Mission through Work Policy.",
        ),
        BeaconCapability::WorkFormTeam => (
            "Form Teams",
            "Request a named Team from eligible Agents through Work Policy.",
        ),
        BeaconCapability::WorkCoordinateMission => (
            "Coordinate Missions",
            "Coordinate one assigned Mission without gaining extra execution authority.",
        ),
        BeaconCapability::WorkAssignWork => (
            "Assign Work",
            "Request bounded Assignments for Agents already eligible for the Mission.",
        ),
        BeaconCapability::ArtifactProduce => (
            "Produce Artifacts",
            "Create attributable Mission Artifacts inside declared bounds.",
        ),
        BeaconCapability::ArtifactEvaluate => (
            "Evaluate Artifacts",
            "Evaluate another Agent's Material Artifact without granting authority.",
        ),
        BeaconCapability::EvidenceDevelop => (
            "Develop Evidence",
            "Create Claims and attach provenance-linked Evidence.",
        ),
    };
    BeaconCapabilityDescriptor {
        capability,
        label: label.to_owned(),
        description: description.to_owned(),
    }
}

fn normalize_capabilities(
    mut capabilities: Vec<BeaconCapability>,
) -> Result<Vec<BeaconCapability>, ResidentConfigurationError> {
    if capabilities.is_empty() || capabilities.len() > BeaconCapability::ALL.len() {
        return Err(ResidentConfigurationError::InvalidCapabilities);
    }
    let original_len = capabilities.len();
    capabilities.sort_unstable();
    capabilities.dedup();
    if capabilities.len() != original_len {
        return Err(ResidentConfigurationError::InvalidCapabilities);
    }
    Ok(capabilities)
}

fn require_expected(
    predecessor: Option<&StoredResidentConfigurationDecision>,
    expected_decision_id: Option<Uuid>,
) -> Result<(), ResidentConfigurationError> {
    if predecessor.map(|decision| decision.id) != expected_decision_id {
        return Err(ResidentConfigurationError::StaleDecision);
    }
    Ok(())
}

fn monotonic_time(
    now: i64,
    predecessor: Option<&StoredResidentConfigurationDecision>,
) -> Result<u64, ResidentConfigurationError> {
    let now = u64::try_from(now).map_err(|_| ResidentConfigurationError::InvalidStore)?;
    predecessor.map_or(Ok(now), |decision| {
        Ok(now.max(
            decision
                .decided_at_ms
                .checked_add(1)
                .ok_or(ResidentConfigurationError::InvalidStore)?,
        ))
    })
}

fn insert_decision(
    connection: &Connection,
    decision: &StoredResidentConfigurationDecision,
) -> Result<(), ResidentConfigurationError> {
    connection.execute(
        "INSERT INTO resident_configuration_decisions (
             id, workspace_id, resident_id, state, predecessor_id, decided_by, decided_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, 'operator', ?6)",
        params![
            decision.id.to_string(),
            decision.workspace_id.to_string(),
            decision.resident_id.to_string(),
            decision.state.as_str(),
            decision.predecessor_id.map(|id| id.to_string()),
            i64::try_from(decision.decided_at_ms)
                .map_err(|_| ResidentConfigurationError::InvalidStore)?,
        ],
    )?;
    if let Some(binding) = &decision.binding {
        connection.execute(
            "INSERT INTO resident_configuration_bindings (
                 workspace_id, resident_id, configuration_id, activation_id, budget_boundary
             ) VALUES (?1, ?2, ?3, ?4, 'no_spend')",
            params![
                decision.workspace_id.to_string(),
                decision.resident_id.to_string(),
                decision.id.to_string(),
                binding.activation_id.to_string(),
            ],
        )?;
        for capability in &binding.capabilities {
            connection.execute(
                "INSERT INTO resident_configuration_capabilities (
                     workspace_id, resident_id, configuration_id, capability
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    decision.workspace_id.to_string(),
                    decision.resident_id.to_string(),
                    decision.id.to_string(),
                    capability.as_str(),
                ],
            )?;
        }
    }
    let payload = serde_json::to_string(&ResidentConfigurationDecisionPayload {
        decision: decision.clone(),
    })?;
    connection.execute(
        "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            decision.workspace_id.to_string(),
            event_kind(decision.state),
            payload,
            i64::try_from(decision.decided_at_ms)
                .map_err(|_| ResidentConfigurationError::InvalidStore)?,
        ],
    )?;
    Ok(())
}

fn latest_configuration(
    connection: &Connection,
    workspace_id: Uuid,
    resident_id: Uuid,
) -> Result<Option<StoredResidentConfigurationDecision>, ResidentConfigurationError> {
    load_configuration_where(
        connection,
        "WHERE workspace_id = ?1 AND resident_id = ?2 ORDER BY sequence DESC LIMIT 1",
        params![workspace_id.to_string(), resident_id.to_string()],
    )
}

pub(crate) fn current_configuration(
    connection: &Connection,
    workspace_id: Uuid,
    resident_id: Uuid,
) -> Result<Option<ResidentConfiguration>, ResidentConfigurationError> {
    latest_configuration(connection, workspace_id, resident_id)?
        .map(|decision| render_configuration(connection, decision))
        .transpose()
}

fn configuration_by_id(
    connection: &Connection,
    workspace_id: Uuid,
    configuration_id: Uuid,
) -> Result<Option<StoredResidentConfigurationDecision>, ResidentConfigurationError> {
    load_configuration_where(
        connection,
        "WHERE workspace_id = ?1 AND id = ?2",
        params![workspace_id.to_string(), configuration_id.to_string()],
    )
}

fn load_configuration_where<P: rusqlite::Params>(
    connection: &Connection,
    clause: &str,
    parameters: P,
) -> Result<Option<StoredResidentConfigurationDecision>, ResidentConfigurationError> {
    let sql = format!(
        "SELECT id, workspace_id, resident_id, state, predecessor_id, decided_at_ms
         FROM resident_configuration_decisions {clause}"
    );
    let base = connection
        .query_row(&sql, parameters, |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .optional()?;
    base.map(|base| read_stored_decision(connection, base))
        .transpose()
}

fn read_stored_decision(
    connection: &Connection,
    base: (String, String, String, String, Option<String>, i64),
) -> Result<StoredResidentConfigurationDecision, ResidentConfigurationError> {
    let id = parse_uuid(base.0)?;
    let workspace_id = parse_uuid(base.1)?;
    let resident_id = parse_uuid(base.2)?;
    let state = ResidentConfigurationState::parse(&base.3)?;
    let predecessor_id = base.4.map(parse_uuid).transpose()?;
    let decided_at_ms =
        u64::try_from(base.5).map_err(|_| ResidentConfigurationError::InvalidStore)?;
    let binding_row = connection
        .query_row(
            "SELECT activation_id, budget_boundary
             FROM resident_configuration_bindings
             WHERE workspace_id = ?1 AND resident_id = ?2 AND configuration_id = ?3",
            params![
                workspace_id.to_string(),
                resident_id.to_string(),
                id.to_string()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let binding = binding_row
        .map(|(activation_id, budget)| {
            if budget != "no_spend" {
                return Err(ResidentConfigurationError::InvalidStore);
            }
            let mut statement = connection.prepare(
                "SELECT capability
                 FROM resident_configuration_capabilities
                 WHERE workspace_id = ?1 AND resident_id = ?2 AND configuration_id = ?3
                 ORDER BY CASE capability
                   WHEN 'work.create_mission' THEN 0
                   WHEN 'work.form_team' THEN 1
                   WHEN 'work.coordinate_mission' THEN 2
                   WHEN 'work.assign_work' THEN 3
                   WHEN 'artifact.produce' THEN 4
                   WHEN 'artifact.evaluate' THEN 5
                   WHEN 'evidence.develop' THEN 6
                   ELSE 7
                 END",
            )?;
            let mut rows = statement.query(params![
                workspace_id.to_string(),
                resident_id.to_string(),
                id.to_string(),
            ])?;
            let mut capabilities = Vec::new();
            while let Some(row) = rows.next()? {
                capabilities.push(BeaconCapability::parse(&row.get::<_, String>(0)?)?);
            }
            validate_stored_capabilities(&capabilities)?;
            Ok(ResidentConfigurationBinding {
                activation_id: parse_uuid(activation_id)?,
                capabilities,
                budget_boundary: BudgetBoundary::NoSpend,
            })
        })
        .transpose()?;
    match (state, binding.is_some()) {
        (ResidentConfigurationState::Configured, true)
        | (ResidentConfigurationState::Cleared, false) => {}
        _ => return Err(ResidentConfigurationError::InvalidStore),
    }
    Ok(StoredResidentConfigurationDecision {
        id,
        workspace_id,
        resident_id,
        state,
        binding,
        predecessor_id,
        decided_at_ms,
    })
}

fn validate_stored_capabilities(
    capabilities: &[BeaconCapability],
) -> Result<(), ResidentConfigurationError> {
    if capabilities.is_empty()
        || capabilities.len() > BeaconCapability::ALL.len()
        || !capabilities.windows(2).all(|pair| pair[0] < pair[1])
    {
        return Err(ResidentConfigurationError::InvalidStore);
    }
    Ok(())
}

fn list_latest_configurations(
    connection: &Connection,
    workspace_id: Uuid,
) -> Result<Vec<ResidentConfiguration>, ResidentConfigurationError> {
    let mut statement = connection.prepare(
        "SELECT decision.id
         FROM resident_configuration_decisions AS decision
         WHERE decision.workspace_id = ?1
           AND NOT EXISTS (
             SELECT 1 FROM resident_configuration_decisions AS newer
             WHERE newer.workspace_id = decision.workspace_id
               AND newer.resident_id = decision.resident_id
               AND newer.sequence > decision.sequence
           )
         ORDER BY decision.resident_id",
    )?;
    let mut rows = statement.query([workspace_id.to_string()])?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next()? {
        ids.push(parse_uuid(row.get::<_, String>(0)?)?);
    }
    drop(rows);
    drop(statement);
    ids.into_iter()
        .map(|id| {
            let decision = configuration_by_id(connection, workspace_id, id)?
                .ok_or(ResidentConfigurationError::InvalidStore)?;
            render_configuration(connection, decision)
        })
        .collect()
}

fn render_configuration(
    connection: &Connection,
    decision: StoredResidentConfigurationDecision,
) -> Result<ResidentConfiguration, ResidentConfigurationError> {
    let resident = load_resident(connection, decision.workspace_id, decision.resident_id)
        .map_err(map_resident_error)?
        .ok_or(ResidentConfigurationError::InvalidStore)?;
    let mut reasons = Vec::new();
    match decision.state {
        ResidentConfigurationState::Cleared => {
            reasons.push(TeamIneligibilityReason::ConfigurationCleared)
        }
        ResidentConfigurationState::Configured => {}
    }
    match resident.lifecycle {
        ResidentLifecycle::Inactive => reasons.push(TeamIneligibilityReason::ResidentInactive),
        ResidentLifecycle::Active => {}
        ResidentLifecycle::Suspended => reasons.push(TeamIneligibilityReason::ResidentSuspended),
        ResidentLifecycle::Retired => reasons.push(TeamIneligibilityReason::ResidentRetired),
    }
    if let Some(binding) = &decision.binding {
        let activation = activation_by_id(connection, decision.workspace_id, binding.activation_id)
            .map_err(map_activation_error)?
            .ok_or(ResidentConfigurationError::InvalidStore)?;
        if !activation
            .approved_features
            .contains(&ProviderFeature::Completion)
        {
            return Err(ResidentConfigurationError::InvalidStore);
        }
        let latest =
            latest_activation_for_resource(connection, decision.workspace_id, &activation.binding)
                .map_err(map_activation_error)?;
        if activation.state != LocalModelActivationState::Active
            || latest.as_ref().map(|candidate| candidate.id) != Some(activation.id)
        {
            reasons.push(TeamIneligibilityReason::ModelActivationUnavailable);
        }
    }
    reasons.sort_unstable();
    reasons.dedup();
    Ok(ResidentConfiguration {
        id: decision.id,
        workspace_id: decision.workspace_id,
        resident_id: decision.resident_id,
        state: decision.state,
        binding: decision.binding,
        predecessor_id: decision.predecessor_id,
        decided_at_ms: decision.decided_at_ms,
        eligible_for_team: reasons.is_empty(),
        ineligibility_reasons: reasons,
    })
}

fn parse_uuid(value: String) -> Result<Uuid, ResidentConfigurationError> {
    Uuid::parse_str(&value).map_err(|_| ResidentConfigurationError::InvalidStore)
}

fn map_resident_error(error: ResidentLifecycleError) -> ResidentConfigurationError {
    match error {
        ResidentLifecycleError::Store(error) => ResidentConfigurationError::Store(error),
        ResidentLifecycleError::Json(error) => ResidentConfigurationError::Json(error),
        _ => ResidentConfigurationError::InvalidStore,
    }
}

fn map_activation_error(error: LocalModelActivationError) -> ResidentConfigurationError {
    match error {
        LocalModelActivationError::Store(error) => ResidentConfigurationError::Store(error),
        LocalModelActivationError::Json(error) => ResidentConfigurationError::Json(error),
        _ => ResidentConfigurationError::InvalidStore,
    }
}

fn event_kind(state: ResidentConfigurationState) -> &'static str {
    match state {
        ResidentConfigurationState::Configured => "resident.configuration_set",
        ResidentConfigurationState::Cleared => "resident.configuration_cleared",
    }
}

pub(crate) fn verify_configuration_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    expected_state: ResidentConfigurationState,
    payload_json: &str,
) -> Result<(), ResidentConfigurationError> {
    let payload: ResidentConfigurationDecisionPayload = serde_json::from_str(payload_json)?;
    if payload.decision.workspace_id != workspace_id
        || payload.decision.state != expected_state
        || payload.decision.decided_at_ms != occurred_at_ms
    {
        return Err(ResidentConfigurationError::InvalidStore);
    }
    let stored = configuration_by_id(connection, workspace_id, payload.decision.id)?
        .ok_or(ResidentConfigurationError::InvalidStore)?;
    if stored != payload.decision {
        return Err(ResidentConfigurationError::InvalidStore);
    }
    Ok(())
}

pub(crate) fn verify_configuration_state(
    connection: &Connection,
) -> Result<(), ResidentConfigurationStateFault> {
    let invalid_sequence = connection
        .query_row(
            "SELECT workspace_id, id, sequence
             FROM resident_configuration_decisions
             WHERE sequence < 1
             ORDER BY workspace_id, id, sequence
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(ResidentConfigurationStateFault::Store)?;
    if let Some((workspace_id, decision_id, sequence)) = invalid_sequence {
        return Err(ResidentConfigurationStateFault::Sequence {
            workspace_id,
            decision_id,
            sequence,
        });
    }
    let mut statement = connection
        .prepare(
            "SELECT id, workspace_id, resident_id, state, predecessor_id, decided_at_ms
         FROM resident_configuration_decisions
         ORDER BY workspace_id, resident_id, sequence",
        )
        .map_err(ResidentConfigurationStateFault::Store)?;
    let mut rows = statement
        .query([])
        .map_err(ResidentConfigurationStateFault::Store)?;
    let mut bases = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(ResidentConfigurationStateFault::Store)?
    {
        bases.push((
            row.get::<_, String>(0)
                .map_err(ResidentConfigurationStateFault::Store)?,
            row.get::<_, String>(1)
                .map_err(ResidentConfigurationStateFault::Store)?,
            row.get::<_, String>(2)
                .map_err(ResidentConfigurationStateFault::Store)?,
            row.get::<_, String>(3)
                .map_err(ResidentConfigurationStateFault::Store)?,
            row.get::<_, Option<String>>(4)
                .map_err(ResidentConfigurationStateFault::Store)?,
            row.get::<_, i64>(5)
                .map_err(ResidentConfigurationStateFault::Store)?,
        ));
    }
    drop(rows);
    drop(statement);
    let mut latest: HashMap<(Uuid, Uuid), (Uuid, u64)> = HashMap::new();
    for base in bases {
        // The decision's own id and Workspace, as the relation holds them: this pass reads them
        // as text, so the key is the reader's own type here (R28.2) rather than a parsed one.
        let decision_id = base.0.clone();
        let workspace_text = base.1.clone();
        verify_one_configuration_decision(connection, base, &mut latest).map_err(|cause| {
            match Uuid::parse_str(&workspace_text) {
                Ok(workspace_id) => ResidentConfigurationStateFault::Row {
                    workspace_id,
                    decision_id: decision_id.clone(),
                    cause,
                },
                Err(_) => ResidentConfigurationStateFault::Identity {
                    workspace_id: workspace_text.clone(),
                    decision_id: decision_id.clone(),
                },
            }
        })?;
    }
    Ok(())
}

/// One stored configuration decision, re-proved. Unchanged from what the loop held inline.
#[allow(clippy::type_complexity)]
fn verify_one_configuration_decision(
    connection: &Connection,
    base: (String, String, String, String, Option<String>, i64),
    latest: &mut HashMap<(Uuid, Uuid), (Uuid, u64)>,
) -> Result<(), ResidentConfigurationError> {
    {
        let decision = read_stored_decision(connection, base)?;
        load_resident(connection, decision.workspace_id, decision.resident_id)
            .map_err(map_resident_error)?
            .ok_or(ResidentConfigurationError::InvalidStore)?;
        let key = (decision.workspace_id, decision.resident_id);
        match latest.get(&key) {
            None if decision.predecessor_id.is_none() => {}
            Some((predecessor_id, predecessor_time))
                if decision.predecessor_id == Some(*predecessor_id)
                    && decision.decided_at_ms > *predecessor_time => {}
            _ => return Err(ResidentConfigurationError::InvalidStore),
        }
        if let Some(binding) = &decision.binding {
            let activation =
                activation_by_id(connection, decision.workspace_id, binding.activation_id)
                    .map_err(map_activation_error)?
                    .ok_or(ResidentConfigurationError::InvalidStore)?;
            if !activation
                .approved_features
                .contains(&ProviderFeature::Completion)
            {
                return Err(ResidentConfigurationError::InvalidStore);
            }
        }
        let payload = serde_json::to_string(&ResidentConfigurationDecisionPayload {
            decision: decision.clone(),
        })?;
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = ?2 AND payload_json = ?3 AND occurred_at_ms = ?4",
            params![
                decision.workspace_id.to_string(),
                event_kind(decision.state),
                payload,
                i64::try_from(decision.decided_at_ms)
                    .map_err(|_| ResidentConfigurationError::InvalidStore)?,
            ],
            |row| row.get(0),
        )?;
        if count != 1 {
            return Err(ResidentConfigurationError::InvalidStore);
        }
        latest.insert(key, (decision.id, decision.decided_at_ms));
    }
    Ok(())
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &ResidentConfigurationError) -> &'static str {
        match error {
            ResidentConfigurationError::Account(_) => "Account",
            ResidentConfigurationError::ResidentNotFound => "ResidentNotFound",
            ResidentConfigurationError::ResidentRetired => "ResidentRetired",
            ResidentConfigurationError::ModelActivationUnavailable => "ModelActivationUnavailable",
            ResidentConfigurationError::ModelMissingCompletion => "ModelMissingCompletion",
            ResidentConfigurationError::InvalidCapabilities => "InvalidCapabilities",
            ResidentConfigurationError::StaleDecision => "StaleDecision",
            ResidentConfigurationError::AlreadyCleared => "AlreadyCleared",
            ResidentConfigurationError::InvalidStore => "InvalidStore",
            ResidentConfigurationError::Store(_) => "Store",
            ResidentConfigurationError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_resident_configuration_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            ResidentConfigurationError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            ResidentConfigurationError::ResidentNotFound,
            ResidentConfigurationError::ResidentRetired,
            ResidentConfigurationError::ModelActivationUnavailable,
            ResidentConfigurationError::ModelMissingCompletion,
            ResidentConfigurationError::InvalidCapabilities,
            ResidentConfigurationError::StaleDecision,
            ResidentConfigurationError::AlreadyCleared,
            ResidentConfigurationError::InvalidStore,
            ResidentConfigurationError::Store(rusqlite::Error::QueryReturnedNoRows),
            ResidentConfigurationError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            11,
            "an arm was added to `ResidentConfigurationError` without a fixture here"
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
            .map(ResidentConfigurationError::name)
            .collect();
        assert_eq!(
            names.len(),
            11,
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
