use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

use data_encoding::HEXLOWER;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::resource_discovery::ollama_agent;
use crate::{
    DiscoveryStatus, LocalModelDiscoveryReport, LocalModelEndpoint, ModelProvider,
    OperatorAccountError, OperatorAccountService, ResourceDiscoveryService,
};

const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_PROVIDER_FEATURES: usize = 64;
const MAX_PROVIDER_FEATURE_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFeature {
    Completion,
    Embedding,
    Thinking,
    Tools,
    Vision,
}

impl ProviderFeature {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "completion" => Some(Self::Completion),
            "embedding" => Some(Self::Embedding),
            "thinking" => Some(Self::Thinking),
            "tools" => Some(Self::Tools),
            "vision" => Some(Self::Vision),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Completion => "completion",
            Self::Embedding => "embedding",
            Self::Thinking => "thinking",
            Self::Tools => "tools",
            Self::Vision => "vision",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalModelBinding {
    pub provider: ModelProvider,
    pub endpoint: String,
    pub model_name: String,
    pub model_digest: String,
    pub inspection_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalModelActivationReview {
    pub binding: LocalModelBinding,
    pub provider_features: Vec<ProviderFeature>,
    pub unrecognized_features: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalModelActivationState {
    Active,
    Inactive,
}

impl LocalModelActivationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Inactive => "inactive",
        }
    }

    fn parse(value: &str) -> Result<Self, LocalModelActivationError> {
        match value {
            "active" => Ok(Self::Active),
            "inactive" => Ok(Self::Inactive),
            _ => Err(LocalModelActivationError::InvalidStore),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalModelActivation {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub binding: LocalModelBinding,
    pub provider_features: Vec<ProviderFeature>,
    pub unrecognized_features: Vec<String>,
    pub approved_features: Vec<ProviderFeature>,
    pub state: LocalModelActivationState,
    pub predecessor_id: Option<Uuid>,
    pub decided_at_ms: u64,
}

/// Which fault the LocalModelActivation startup pass hit, and WHICH ROW hit it (R27.1's shape, R28.1).
pub(crate) enum LocalModelActivationStateFault {
    /// The enumeration itself failed: no row to name, because none was reached.
    Store(rusqlite::Error),
    /// One LocalModelActivation did not re-prove. `cause` is the owning reader's OWN error, carried whole.
    Row {
        workspace_id: Uuid,
        activation_id: Uuid,
        cause: LocalModelActivationError,
    },
    // NO `Totals` ARM. Every check belongs to one activation decision and its own Event.
    /// A row that could not be READ at all, so it cannot be named -- the identity columns are
    /// part of what failed. Distinct from `Row` (pass 11's shape).
    Unreadable(LocalModelActivationError),
    /// A row whose raw identity columns cannot be represented by the public UUID keys. Those
    /// columns are read before the typed row so startup can preserve an escaped witness.
    Identity {
        workspace_id: String,
        activation_id: String,
    },
    /// A decision whose stored ordering key is outside the positive domain the service writes.
    Sequence {
        workspace_id: String,
        activation_id: String,
        sequence: i64,
    },
}

impl LocalModelActivationError {
    /// The variant's NAME, for R28.4's one-line startup message.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Account(_) => "Account",
            Self::ResourceUnavailable => "ResourceUnavailable",
            Self::InvalidProviderResponse => "InvalidProviderResponse",
            Self::ModelNotFound => "ModelNotFound",
            Self::ModelDrift => "ModelDrift",
            Self::InvalidApproval => "InvalidApproval",
            Self::AlreadyActive => "AlreadyActive",
            Self::StaleDecision => "StaleDecision",
            Self::InvalidStore => "InvalidStore",
            Self::Store(_) => "Store",
            Self::Json(_) => "Json",
        }
    }
}

impl fmt::Display for LocalModelActivationStateFault {
    /// R28.4: the KEY and the fault's NAME, never the carried error's own text.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(_) => formatter.write_str("the LocalModelActivations could not be read"),
            Self::Row {
                workspace_id,
                activation_id,
                cause,
            } => write!(
                formatter,
                "LocalModelActivation {activation_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::Unreadable(cause) => write!(
                formatter,
                "a local model activation row could not be read: {}",
                cause.name()
            ),
            Self::Identity { .. } => {
                formatter.write_str("a local-model activation row's identity is not readable")
            }
            Self::Sequence { .. } => formatter
                .write_str("a local-model activation decision's ordering key is not readable"),
        }
    }
}

/// Hand-written and NOT delegating to `Display`: R28.4 keeps the carried text reachable here.
impl fmt::Debug for LocalModelActivationStateFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => formatter.debug_tuple("Store").field(error).finish(),
            Self::Row {
                workspace_id,
                activation_id,
                cause,
            } => formatter
                .debug_struct("Row")
                .field("workspace_id", workspace_id)
                .field("activation_id", activation_id)
                .field("cause", cause)
                .finish(),
            Self::Unreadable(cause) => formatter.debug_tuple("Unreadable").field(cause).finish(),
            Self::Identity {
                workspace_id,
                activation_id,
            } => formatter
                .debug_struct("Identity")
                .field("workspace_id", workspace_id)
                .field("activation_id", activation_id)
                .finish(),
            Self::Sequence {
                workspace_id,
                activation_id,
                sequence,
            } => formatter
                .debug_struct("Sequence")
                .field("workspace_id", workspace_id)
                .field("activation_id", activation_id)
                .field("sequence", sequence)
                .finish(),
        }
    }
}

impl std::error::Error for LocalModelActivationStateFault {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            Self::Unreadable(cause) | Self::Row { cause, .. } => Some(cause),
            Self::Identity { .. } | Self::Sequence { .. } => None,
        }
    }
}

#[derive(Debug)]
pub enum LocalModelActivationError {
    Account(OperatorAccountError),
    ResourceUnavailable,
    InvalidProviderResponse,
    ModelNotFound,
    ModelDrift,
    InvalidApproval,
    AlreadyActive,
    StaleDecision,
    InvalidStore,
    Store(rusqlite::Error),
    Json(serde_json::Error),
}

impl fmt::Display for LocalModelActivationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::ResourceUnavailable => write!(formatter, "local model provider is unavailable"),
            Self::InvalidProviderResponse => {
                write!(
                    formatter,
                    "local model provider returned invalid inspection data"
                )
            }
            Self::ModelNotFound => write!(formatter, "local model was not found"),
            Self::ModelDrift => write!(formatter, "local model changed during review"),
            Self::InvalidApproval => write!(formatter, "local model approval is invalid"),
            Self::AlreadyActive => write!(formatter, "local model is already active"),
            Self::StaleDecision => write!(formatter, "local model activation decision is stale"),
            Self::InvalidStore => write!(formatter, "local model activation state is invalid"),
            Self::Store(error) => write!(formatter, "local model activation store failed: {error}"),
            Self::Json(error) => write!(
                formatter,
                "local model activation event is invalid: {error}"
            ),
        }
    }
}

impl std::error::Error for LocalModelActivationError {}

impl From<OperatorAccountError> for LocalModelActivationError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

impl From<rusqlite::Error> for LocalModelActivationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for LocalModelActivationError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Deserialize)]
struct OllamaShowResponse {
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct LocalModelActivationDecisionPayload {
    activation: LocalModelActivation,
}

/// Owns exact local-model inspection and Workspace-scoped activation decisions.
pub struct LocalModelActivationService {
    accounts: Arc<OperatorAccountService>,
    workspace_id: Uuid,
    endpoint: LocalModelEndpoint,
    discovery: ResourceDiscoveryService,
}

impl LocalModelActivationService {
    pub fn new(
        accounts: Arc<OperatorAccountService>,
        workspace_id: Uuid,
        endpoint: LocalModelEndpoint,
    ) -> Self {
        Self {
            discovery: ResourceDiscoveryService::new(Arc::clone(&accounts), endpoint),
            accounts,
            workspace_id,
            endpoint,
        }
    }

    pub fn inspect(
        &self,
        model_name: &str,
        expected_digest: &str,
    ) -> Result<LocalModelActivationReview, LocalModelActivationError> {
        let before = self
            .discovery
            .discover_local_models()
            .map_err(map_discovery_error)?;
        require_exact_model(&before, model_name, expected_digest)?;

        let request_body = serde_json::to_string(&serde_json::json!({
            "model": model_name,
            "verbose": false,
        }))
        .map_err(|_| LocalModelActivationError::InvalidProviderResponse)?;
        let mut response = ollama_agent()
            .post(format!("{}/api/show", self.endpoint.base_url()))
            .content_type("application/json")
            .send(request_body)
            .map_err(|_| LocalModelActivationError::ResourceUnavailable)?;
        if response.status().as_u16() != 200 {
            return Err(LocalModelActivationError::ResourceUnavailable);
        }
        let body = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|_| LocalModelActivationError::InvalidProviderResponse)?;
        let response: OllamaShowResponse = serde_json::from_str(&body)
            .map_err(|_| LocalModelActivationError::InvalidProviderResponse)?;
        let (provider_features, unrecognized_features) =
            normalize_provider_features(response.capabilities)?;

        let after = self
            .discovery
            .discover_local_models()
            .map_err(map_discovery_error)?;
        require_exact_model(&after, model_name, expected_digest)?;

        let endpoint = self.endpoint.base_url();
        let inspection_digest = inspection_digest(
            &endpoint,
            model_name,
            expected_digest,
            &provider_features,
            &unrecognized_features,
        );
        Ok(LocalModelActivationReview {
            binding: LocalModelBinding {
                provider: ModelProvider::Ollama,
                endpoint,
                model_name: model_name.to_owned(),
                model_digest: expected_digest.to_owned(),
                inspection_digest,
            },
            provider_features,
            unrecognized_features,
        })
    }

    pub fn list(&self) -> Result<Vec<LocalModelActivation>, LocalModelActivationError> {
        self.accounts.with_authenticated_store(|connection, _now| {
            list_latest_activations(connection, self.workspace_id)
        })
    }

    pub fn activate(
        &self,
        review: LocalModelActivationReview,
        approved_features: Vec<ProviderFeature>,
    ) -> Result<LocalModelActivation, LocalModelActivationError> {
        let current = self.inspect(&review.binding.model_name, &review.binding.model_digest)?;
        if current != review {
            return Err(LocalModelActivationError::ModelDrift);
        }
        let approved_features =
            normalize_approved_features(approved_features, &current.provider_features)?;
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let predecessor =
                latest_activation_for_resource(&transaction, self.workspace_id, &current.binding)?;
            if predecessor
                .as_ref()
                .is_some_and(|decision| decision.state == LocalModelActivationState::Active)
            {
                return Err(LocalModelActivationError::AlreadyActive);
            }
            let activation = LocalModelActivation {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                binding: current.binding,
                provider_features: current.provider_features,
                unrecognized_features: current.unrecognized_features,
                approved_features,
                state: LocalModelActivationState::Active,
                predecessor_id: predecessor.as_ref().map(|decision| decision.id),
                decided_at_ms: monotonic_time(now, predecessor.as_ref())?,
            };
            insert_decision(&transaction, &activation)?;
            transaction.commit()?;
            Ok(activation)
        })
    }

    pub fn deactivate(
        &self,
        expected_activation_id: Uuid,
    ) -> Result<LocalModelActivation, LocalModelActivationError> {
        self.accounts.with_authenticated_store(|connection, now| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let expected =
                activation_by_id(&transaction, self.workspace_id, expected_activation_id)?
                    .ok_or(LocalModelActivationError::StaleDecision)?;
            let latest =
                latest_activation_for_resource(&transaction, self.workspace_id, &expected.binding)?
                    .ok_or(LocalModelActivationError::StaleDecision)?;
            if latest.id != expected_activation_id
                || latest.state != LocalModelActivationState::Active
            {
                return Err(LocalModelActivationError::StaleDecision);
            }
            let decided_at_ms = monotonic_time(now, Some(&latest))?;
            let activation = LocalModelActivation {
                id: Uuid::new_v4(),
                workspace_id: self.workspace_id,
                binding: latest.binding,
                provider_features: latest.provider_features,
                unrecognized_features: latest.unrecognized_features,
                approved_features: latest.approved_features,
                state: LocalModelActivationState::Inactive,
                predecessor_id: Some(latest.id),
                decided_at_ms,
            };
            insert_decision(&transaction, &activation)?;
            transaction.commit()?;
            Ok(activation)
        })
    }
}

fn map_discovery_error(error: crate::ResourceDiscoveryError) -> LocalModelActivationError {
    match error {
        crate::ResourceDiscoveryError::Account(error) => LocalModelActivationError::Account(error),
        crate::ResourceDiscoveryError::EndpointNotLoopback => {
            LocalModelActivationError::ResourceUnavailable
        }
    }
}

fn normalize_approved_features(
    values: Vec<ProviderFeature>,
    advertised: &[ProviderFeature],
) -> Result<Vec<ProviderFeature>, LocalModelActivationError> {
    if values.is_empty() || values.len() > advertised.len() {
        return Err(LocalModelActivationError::InvalidApproval);
    }
    let normalized = values.iter().copied().collect::<BTreeSet<_>>();
    if normalized.len() != values.len()
        || !normalized
            .iter()
            .all(|feature| advertised.contains(feature))
    {
        return Err(LocalModelActivationError::InvalidApproval);
    }
    Ok(normalized.into_iter().collect())
}

fn monotonic_time(
    now: i64,
    predecessor: Option<&LocalModelActivation>,
) -> Result<u64, LocalModelActivationError> {
    let now = u64::try_from(now).map_err(|_| LocalModelActivationError::InvalidStore)?;
    Ok(predecessor
        .map(|decision| now.max(decision.decided_at_ms))
        .unwrap_or(now))
}

fn insert_decision(
    connection: &Connection,
    activation: &LocalModelActivation,
) -> Result<(), LocalModelActivationError> {
    connection.execute(
        "INSERT INTO local_model_activation_decisions (
            id, workspace_id, provider, endpoint, model_name, model_digest,
            inspection_digest, provider_features_json, unrecognized_features_json,
            approved_features_json, state, predecessor_id, decided_at_ms
         ) VALUES (?1, ?2, 'ollama', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            activation.id.to_string(),
            activation.workspace_id.to_string(),
            activation.binding.endpoint,
            activation.binding.model_name,
            activation.binding.model_digest,
            activation.binding.inspection_digest,
            serde_json::to_string(&activation.provider_features)?,
            serde_json::to_string(&activation.unrecognized_features)?,
            serde_json::to_string(&activation.approved_features)?,
            activation.state.as_str(),
            activation.predecessor_id.map(|id| id.to_string()),
            i64::try_from(activation.decided_at_ms)
                .map_err(|_| LocalModelActivationError::InvalidStore)?,
        ],
    )?;
    let kind = match activation.state {
        LocalModelActivationState::Active => "local_model.activated",
        LocalModelActivationState::Inactive => "local_model.deactivated",
    };
    let payload = serde_json::to_string(&LocalModelActivationDecisionPayload {
        activation: activation.clone(),
    })?;
    crate::event_chain::append_event(
        connection,
        &activation.workspace_id.to_string(),
        kind,
        &payload,
        i64::try_from(activation.decided_at_ms)
            .map_err(|_| LocalModelActivationError::InvalidStore)?,
    )?;
    Ok(())
}

pub(crate) fn latest_activation_for_resource(
    connection: &Connection,
    workspace_id: Uuid,
    binding: &LocalModelBinding,
) -> Result<Option<LocalModelActivation>, LocalModelActivationError> {
    let mut statement = connection.prepare(
        "SELECT id, workspace_id, provider, endpoint, model_name, model_digest,
                inspection_digest, provider_features_json, unrecognized_features_json,
                approved_features_json, state, predecessor_id, decided_at_ms
         FROM local_model_activation_decisions
         WHERE workspace_id = ?1 AND provider = 'ollama' AND endpoint = ?2 AND model_name = ?3
         ORDER BY sequence DESC
         LIMIT 1",
    )?;
    let mut rows = statement.query(params![
        workspace_id.to_string(),
        binding.endpoint,
        binding.model_name,
    ])?;
    rows.next()?.map(read_activation_row).transpose()
}

pub(crate) fn activation_by_id(
    connection: &Connection,
    workspace_id: Uuid,
    activation_id: Uuid,
) -> Result<Option<LocalModelActivation>, LocalModelActivationError> {
    let mut statement = connection.prepare(
        "SELECT id, workspace_id, provider, endpoint, model_name, model_digest,
                inspection_digest, provider_features_json, unrecognized_features_json,
                approved_features_json, state, predecessor_id, decided_at_ms
         FROM local_model_activation_decisions
         WHERE workspace_id = ?1 AND id = ?2",
    )?;
    let mut rows = statement.query(params![workspace_id.to_string(), activation_id.to_string()])?;
    rows.next()?.map(read_activation_row).transpose()
}

fn list_latest_activations(
    connection: &Connection,
    workspace_id: Uuid,
) -> Result<Vec<LocalModelActivation>, LocalModelActivationError> {
    let mut statement = connection.prepare(
        "SELECT decision.id, decision.workspace_id, decision.provider, decision.endpoint,
                decision.model_name, decision.model_digest, decision.inspection_digest,
                decision.provider_features_json, decision.unrecognized_features_json,
                decision.approved_features_json, decision.state, decision.predecessor_id,
                decision.decided_at_ms
         FROM local_model_activation_decisions AS decision
         WHERE decision.workspace_id = ?1
           AND NOT EXISTS (
             SELECT 1
             FROM local_model_activation_decisions AS newer
             WHERE newer.workspace_id = decision.workspace_id
               AND newer.provider = decision.provider
               AND newer.endpoint = decision.endpoint
               AND newer.model_name = decision.model_name
               AND newer.sequence > decision.sequence
           )
         ORDER BY decision.model_name, decision.endpoint",
    )?;
    let mut rows = statement.query([workspace_id.to_string()])?;
    let mut activations = Vec::new();
    while let Some(row) = rows.next()? {
        activations.push(read_activation_row(row)?);
    }
    Ok(activations)
}

fn read_activation_row(
    row: &rusqlite::Row<'_>,
) -> Result<LocalModelActivation, LocalModelActivationError> {
    let id = parse_uuid(row.get(0)?)?;
    let workspace_id = parse_uuid(row.get(1)?)?;
    let provider = match row.get::<_, String>(2)?.as_str() {
        "ollama" => ModelProvider::Ollama,
        _ => return Err(LocalModelActivationError::InvalidStore),
    };
    let endpoint = row.get::<_, String>(3)?;
    let model_name = row.get::<_, String>(4)?;
    let model_digest = row.get::<_, String>(5)?;
    let inspection_digest = row.get::<_, String>(6)?;
    let provider_features: Vec<ProviderFeature> = serde_json::from_str(&row.get::<_, String>(7)?)?;
    let unrecognized_features: Vec<String> = serde_json::from_str(&row.get::<_, String>(8)?)?;
    let approved_features: Vec<ProviderFeature> = serde_json::from_str(&row.get::<_, String>(9)?)?;
    let state = LocalModelActivationState::parse(&row.get::<_, String>(10)?)?;
    let predecessor_id = row
        .get::<_, Option<String>>(11)?
        .map(parse_uuid)
        .transpose()?;
    let decided_at_ms = u64::try_from(row.get::<_, i64>(12)?)
        .map_err(|_| LocalModelActivationError::InvalidStore)?;
    let binding = LocalModelBinding {
        provider,
        endpoint,
        model_name,
        model_digest,
        inspection_digest,
    };
    validate_stored_decision(
        &binding,
        &provider_features,
        &unrecognized_features,
        &approved_features,
        state,
        predecessor_id,
    )?;
    Ok(LocalModelActivation {
        id,
        workspace_id,
        binding,
        provider_features,
        unrecognized_features,
        approved_features,
        state,
        predecessor_id,
        decided_at_ms,
    })
}

fn parse_uuid(value: String) -> Result<Uuid, LocalModelActivationError> {
    Uuid::parse_str(&value).map_err(|_| LocalModelActivationError::InvalidStore)
}

fn validate_stored_decision(
    binding: &LocalModelBinding,
    provider_features: &[ProviderFeature],
    unrecognized_features: &[String],
    approved_features: &[ProviderFeature],
    state: LocalModelActivationState,
    predecessor_id: Option<Uuid>,
) -> Result<(), LocalModelActivationError> {
    if binding.provider != ModelProvider::Ollama
        || !is_canonical_loopback_endpoint(&binding.endpoint)
        || binding.model_name.is_empty()
        || binding.model_name.trim() != binding.model_name
        || binding.model_name.chars().count() > 256
        || binding.model_name.chars().any(char::is_control)
        || !is_lower_hex_64(&binding.model_digest)
        || !is_lower_hex_64(&binding.inspection_digest)
        || !is_sorted_unique(provider_features)
        || !is_sorted_unique(approved_features)
        || approved_features.is_empty()
        || !approved_features
            .iter()
            .all(|feature| provider_features.contains(feature))
        || !valid_unrecognized_features(unrecognized_features)
        || (state == LocalModelActivationState::Inactive && predecessor_id.is_none())
        || inspection_digest(
            &binding.endpoint,
            &binding.model_name,
            &binding.model_digest,
            provider_features,
            unrecognized_features,
        ) != binding.inspection_digest
    {
        return Err(LocalModelActivationError::InvalidStore);
    }
    Ok(())
}

fn is_canonical_loopback_endpoint(value: &str) -> bool {
    let Some(authority) = value.strip_prefix("http://") else {
        return false;
    };
    authority.parse::<SocketAddr>().is_ok_and(|address| {
        LocalModelEndpoint::new(address).is_ok() && format!("http://{address}") == value
    })
}

fn is_lower_hex_64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_sorted_unique<T: Ord>(values: &[T]) -> bool {
    !values.is_empty() && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_unrecognized_features(values: &[String]) -> bool {
    values.len() <= MAX_PROVIDER_FEATURES
        && values.windows(2).all(|pair| pair[0] < pair[1])
        && values.iter().all(|value| {
            !value.is_empty()
                && value.chars().count() <= MAX_PROVIDER_FEATURE_CHARS
                && value.trim() == value
                && !value.chars().any(char::is_control)
                && ProviderFeature::parse(value).is_none()
        })
}

pub(crate) fn verify_activation_event(
    connection: &Connection,
    workspace_id: Uuid,
    occurred_at_ms: u64,
    expected_state: LocalModelActivationState,
    payload_json: &str,
) -> Result<(), LocalModelActivationError> {
    let payload: LocalModelActivationDecisionPayload = serde_json::from_str(payload_json)?;
    let activation = activation_by_id(connection, workspace_id, payload.activation.id)?
        .ok_or(LocalModelActivationError::InvalidStore)?;
    if activation != payload.activation
        || activation.workspace_id != workspace_id
        || activation.decided_at_ms != occurred_at_ms
        || activation.state != expected_state
    {
        return Err(LocalModelActivationError::InvalidStore);
    }
    Ok(())
}

/// The ORDER BY `verify_activation_state` enumerates under. ONE definition, interpolated into the pass's own
/// query and into `enumerate_activation_decisions` below, so a mutant on the ordering moves both (R44.6).
///
/// This pass reads whole rows while a bar needs only the keys, so the two cannot be one function
/// the way the digest-keyed passes' are. Sharing the ordering is what keeps a bar's placement
/// assertion from agreeing with a copy the pass no longer follows.
const ENUMERATION_ORDER: &str = "ORDER BY workspace_id, provider, endpoint, model_name, sequence";

/// The keys `verify_activation_state` enumerates, in its order.
pub fn enumerate_activation_decisions(
    connection: &Connection,
) -> rusqlite::Result<Vec<(String, String)>> {
    let mut statement = connection.prepare(&format!(
        "SELECT workspace_id, id FROM local_model_activation_decisions {ENUMERATION_ORDER}"
    ))?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub(crate) fn verify_activation_state(
    connection: &Connection,
) -> Result<(), LocalModelActivationStateFault> {
    let invalid_sequence = connection
        .query_row(
            "SELECT workspace_id, id, sequence
             FROM local_model_activation_decisions
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
        .map_err(LocalModelActivationStateFault::Store)?;
    if let Some((workspace_id, activation_id, sequence)) = invalid_sequence {
        return Err(LocalModelActivationStateFault::Sequence {
            workspace_id,
            activation_id,
            sequence,
        });
    }
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, workspace_id, provider, endpoint, model_name, model_digest,
                inspection_digest, provider_features_json, unrecognized_features_json,
                approved_features_json, state, predecessor_id, decided_at_ms
         FROM local_model_activation_decisions
         {ENUMERATION_ORDER}"
        ))
        .map_err(LocalModelActivationStateFault::Store)?;
    let mut rows = statement
        .query([])
        .map_err(LocalModelActivationStateFault::Store)?;
    let mut previous: HashMap<(Uuid, String, String), LocalModelActivation> = HashMap::new();
    while let Some(row) = rows.next().map_err(LocalModelActivationStateFault::Store)? {
        let activation_id = row
            .get::<_, String>(0)
            .map_err(LocalModelActivationStateFault::Store)?;
        let workspace_id = row
            .get::<_, String>(1)
            .map_err(LocalModelActivationStateFault::Store)?;
        let activation = read_activation_row(row).map_err(|cause| {
            if Uuid::parse_str(&workspace_id).is_err() || Uuid::parse_str(&activation_id).is_err() {
                LocalModelActivationStateFault::Identity {
                    workspace_id: workspace_id.clone(),
                    activation_id: activation_id.clone(),
                }
            } else {
                LocalModelActivationStateFault::Unreadable(cause)
            }
        })?;
        let named_workspace = activation.workspace_id;
        let named_activation = activation.id;
        let key = (
            activation.workspace_id,
            activation.binding.endpoint.clone(),
            activation.binding.model_name.clone(),
        );
        match previous.get(&key) {
            None => {
                if activation.predecessor_id.is_some()
                    || activation.state != LocalModelActivationState::Active
                {
                    return Err(LocalModelActivationStateFault::Row {
                        workspace_id: named_workspace,
                        activation_id: named_activation,
                        cause: LocalModelActivationError::InvalidStore,
                    });
                }
            }
            Some(prior) => {
                if activation.predecessor_id != Some(prior.id)
                    || activation.state == prior.state
                    || activation.decided_at_ms < prior.decided_at_ms
                {
                    return Err(LocalModelActivationStateFault::Row {
                        workspace_id: named_workspace,
                        activation_id: named_activation,
                        cause: LocalModelActivationError::InvalidStore,
                    });
                }
            }
        }
        let payload = serde_json::to_string(&LocalModelActivationDecisionPayload {
            activation: activation.clone(),
        })
        .map_err(|cause| LocalModelActivationStateFault::Row {
            workspace_id: named_workspace,
            activation_id: named_activation,
            cause: LocalModelActivationError::Json(cause),
        })?;
        let kind = match activation.state {
            LocalModelActivationState::Active => "local_model.activated",
            LocalModelActivationState::Inactive => "local_model.deactivated",
        };
        let event_count: i64 = connection
            .query_row(
                "SELECT count(*) FROM events
             WHERE workspace_id = ?1 AND kind = ?2 AND payload_json = ?3 AND occurred_at_ms = ?4",
                params![
                    activation.workspace_id.to_string(),
                    kind,
                    payload,
                    i64::try_from(activation.decided_at_ms).map_err(|_| {
                        LocalModelActivationStateFault::Row {
                            workspace_id: named_workspace,
                            activation_id: named_activation,
                            cause: LocalModelActivationError::InvalidStore,
                        }
                    })?,
                ],
                |row| row.get(0),
            )
            .map_err(LocalModelActivationStateFault::Store)?;
        if event_count != 1 {
            return Err(LocalModelActivationStateFault::Row {
                workspace_id: named_workspace,
                activation_id: named_activation,
                cause: LocalModelActivationError::InvalidStore,
            });
        }
        previous.insert(key, activation);
    }
    Ok(())
}

fn require_exact_model(
    report: &LocalModelDiscoveryReport,
    model_name: &str,
    expected_digest: &str,
) -> Result<(), LocalModelActivationError> {
    if report.status != DiscoveryStatus::Available {
        return Err(LocalModelActivationError::ResourceUnavailable);
    }
    let Some(model) = report.models.iter().find(|model| model.name == model_name) else {
        return Err(LocalModelActivationError::ModelNotFound);
    };
    if model.digest != expected_digest {
        return Err(LocalModelActivationError::ModelDrift);
    }
    Ok(())
}

fn normalize_provider_features(
    values: Vec<String>,
) -> Result<(Vec<ProviderFeature>, Vec<String>), LocalModelActivationError> {
    if values.is_empty() || values.len() > MAX_PROVIDER_FEATURES {
        return Err(LocalModelActivationError::InvalidProviderResponse);
    }
    let mut recognized = BTreeSet::new();
    let mut unrecognized = BTreeSet::new();
    for value in values {
        if value.is_empty()
            || value.chars().count() > MAX_PROVIDER_FEATURE_CHARS
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(LocalModelActivationError::InvalidProviderResponse);
        }
        if let Some(feature) = ProviderFeature::parse(&value) {
            recognized.insert(feature);
        } else {
            unrecognized.insert(value);
        }
    }
    Ok((
        recognized.into_iter().collect(),
        unrecognized.into_iter().collect(),
    ))
}

fn inspection_digest(
    endpoint: &str,
    model_name: &str,
    model_digest: &str,
    provider_features: &[ProviderFeature],
    unrecognized_features: &[String],
) -> String {
    let provider_features = provider_features
        .iter()
        .map(|feature| feature.as_str())
        .collect::<Vec<_>>();
    let provider_features = serde_json::to_string(&provider_features)
        .expect("known Provider Features always serialize");
    let unrecognized_features = serde_json::to_string(unrecognized_features)
        .expect("validated provider feature names always serialize");
    let canonical = format!(
        "beacon.local-model-inspection.v1\nprovider=ollama\nendpoint={endpoint}\nmodel={model_name}\nmodel_digest={model_digest}\nprovider_features={provider_features}\nunrecognized_features={unrecognized_features}\n"
    );
    HEXLOWER.encode(&Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod name_witness {
    use super::*;

    /// Pinned independently of `name()` (the fold at 30d2a20): an exhaustive `name()` catches an
    /// added arm, only a second table catches a wrong or duplicated name.
    fn expected_name(error: &LocalModelActivationError) -> &'static str {
        match error {
            LocalModelActivationError::Account(_) => "Account",
            LocalModelActivationError::ResourceUnavailable => "ResourceUnavailable",
            LocalModelActivationError::InvalidProviderResponse => "InvalidProviderResponse",
            LocalModelActivationError::ModelNotFound => "ModelNotFound",
            LocalModelActivationError::ModelDrift => "ModelDrift",
            LocalModelActivationError::InvalidApproval => "InvalidApproval",
            LocalModelActivationError::AlreadyActive => "AlreadyActive",
            LocalModelActivationError::StaleDecision => "StaleDecision",
            LocalModelActivationError::InvalidStore => "InvalidStore",
            LocalModelActivationError::Store(_) => "Store",
            LocalModelActivationError::Json(_) => "Json",
        }
    }

    #[test]
    fn every_local_model_activation_error_arm_has_its_own_pinned_name() {
        let every_arm = [
            LocalModelActivationError::Account(
                crate::operator_account::OperatorAccountError::PasswordTooShort,
            ),
            LocalModelActivationError::ResourceUnavailable,
            LocalModelActivationError::InvalidProviderResponse,
            LocalModelActivationError::ModelNotFound,
            LocalModelActivationError::ModelDrift,
            LocalModelActivationError::InvalidApproval,
            LocalModelActivationError::AlreadyActive,
            LocalModelActivationError::StaleDecision,
            LocalModelActivationError::InvalidStore,
            LocalModelActivationError::Store(rusqlite::Error::QueryReturnedNoRows),
            LocalModelActivationError::Json(serde_json::from_str::<u8>("\"x\"").unwrap_err()),
        ];
        assert_eq!(
            every_arm.len(),
            11,
            "an arm was added to `LocalModelActivationError` without a fixture here"
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
            .map(LocalModelActivationError::name)
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
