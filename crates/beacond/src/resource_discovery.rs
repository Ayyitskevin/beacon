use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{OperatorAccountError, OperatorAccountService};

const DEFAULT_OLLAMA_PORT: u16 = 11_434;
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
const MAX_MODELS: usize = 512;
const MAX_MODEL_NAME_CHARS: usize = 256;
const MAX_MODIFIED_AT_CHARS: usize = 80;
const MAX_DETAIL_CHARS: usize = 80;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryStatus {
    Available,
    Unavailable,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelProvider {
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelActivation {
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalModelDetails {
    pub format: String,
    pub family: String,
    pub parameter_size: String,
    pub quantization_level: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveredLocalModel {
    pub provider: ModelProvider,
    pub activation: ModelActivation,
    pub name: String,
    pub digest: String,
    pub size_bytes: u64,
    pub modified_at: String,
    pub details: LocalModelDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalModelDiscoveryReport {
    pub status: DiscoveryStatus,
    pub endpoint: String,
    pub models: Vec<DiscoveredLocalModel>,
    pub observed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalModelEndpoint(SocketAddr);

impl LocalModelEndpoint {
    pub fn new(address: SocketAddr) -> Result<Self, ResourceDiscoveryError> {
        if !address.ip().is_loopback() {
            return Err(ResourceDiscoveryError::EndpointNotLoopback);
        }
        Ok(Self(address))
    }

    pub fn ollama_default() -> Self {
        Self(SocketAddr::from(([127, 0, 0, 1], DEFAULT_OLLAMA_PORT)))
    }

    fn base_url(self) -> String {
        format!("http://{}", self.0)
    }
}

#[derive(Debug)]
pub enum ResourceDiscoveryError {
    Account(OperatorAccountError),
    EndpointNotLoopback,
}

impl fmt::Display for ResourceDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Account(error) => write!(formatter, "operator account refused: {error}"),
            Self::EndpointNotLoopback => {
                write!(formatter, "local model discovery endpoint must be loopback")
            }
        }
    }
}

impl std::error::Error for ResourceDiscoveryError {}

impl From<OperatorAccountError> for ResourceDiscoveryError {
    fn from(error: OperatorAccountError) -> Self {
        Self::Account(error)
    }
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
    model: String,
    modified_at: String,
    size: u64,
    digest: String,
    details: OllamaModelDetails,
}

#[derive(Debug, Deserialize)]
struct OllamaModelDetails {
    format: String,
    family: String,
    parameter_size: String,
    quantization_level: String,
}

/// Discovers installed local models without persisting, activating, or invoking them.
pub struct ResourceDiscoveryService {
    accounts: Arc<OperatorAccountService>,
    endpoint: LocalModelEndpoint,
}

impl ResourceDiscoveryService {
    pub fn new(accounts: Arc<OperatorAccountService>, endpoint: LocalModelEndpoint) -> Self {
        Self { accounts, endpoint }
    }

    pub fn discover_local_models(
        &self,
    ) -> Result<LocalModelDiscoveryReport, ResourceDiscoveryError> {
        self.accounts.require_authenticated()?;
        let observed_at_ms = u64::try_from(crate::unix_time_ms().map_err(|_| {
            ResourceDiscoveryError::Account(OperatorAccountError::ClockBeforeUnixEpoch)
        })?)
        .map_err(|_| ResourceDiscoveryError::Account(OperatorAccountError::ClockBeforeUnixEpoch))?;
        let report = discover_ollama(self.endpoint, observed_at_ms);
        self.accounts.require_authenticated()?;
        Ok(report)
    }
}

fn discover_ollama(endpoint: LocalModelEndpoint, observed_at_ms: u64) -> LocalModelDiscoveryReport {
    let base_url = endpoint.base_url();
    let empty_report = |status| LocalModelDiscoveryReport {
        status,
        endpoint: base_url.clone(),
        models: Vec::new(),
        observed_at_ms,
    };

    // Ureq 3.4 Agent configuration. Proxies and redirects are disabled so a loopback-only
    // discovery request cannot leave the machine. The global timeout covers the complete call.
    // Sources: https://docs.rs/ureq/3.4.0/ureq/config/struct.ConfigBuilder.html
    // and https://docs.rs/ureq/3.4.0/ureq/struct.BodyWithConfig.html
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .max_redirects(0)
        .timeout_global(Some(REQUEST_TIMEOUT))
        .user_agent(concat!("Beacon/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    // Ollama documents installed-model discovery as GET /api/tags.
    // Source: https://docs.ollama.com/api/tags
    let mut response = match agent.get(format!("{base_url}/api/tags")).call() {
        Ok(response) if response.status().as_u16() == 200 => response,
        Ok(_) | Err(_) => return empty_report(DiscoveryStatus::Unavailable),
    };
    let body = match response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_to_string()
    {
        Ok(body) => body,
        Err(_) => return empty_report(DiscoveryStatus::InvalidResponse),
    };
    let response: OllamaTagsResponse = match serde_json::from_str(&body) {
        Ok(response) => response,
        Err(_) => return empty_report(DiscoveryStatus::InvalidResponse),
    };
    let models = match normalize_models(response.models) {
        Some(models) => models,
        None => return empty_report(DiscoveryStatus::InvalidResponse),
    };
    LocalModelDiscoveryReport {
        status: DiscoveryStatus::Available,
        endpoint: base_url,
        models,
        observed_at_ms,
    }
}

fn normalize_models(models: Vec<OllamaModel>) -> Option<Vec<DiscoveredLocalModel>> {
    if models.len() > MAX_MODELS {
        return None;
    }
    let mut names = HashSet::new();
    let mut normalized = Vec::with_capacity(models.len());
    for model in models {
        let name = valid_text(model.name, MAX_MODEL_NAME_CHARS)?;
        if model.model != name || !names.insert(name.clone()) || model.size == 0 {
            return None;
        }
        let digest = valid_digest(model.digest)?;
        normalized.push(DiscoveredLocalModel {
            provider: ModelProvider::Ollama,
            activation: ModelActivation::Inactive,
            name,
            digest,
            size_bytes: model.size,
            modified_at: valid_text(model.modified_at, MAX_MODIFIED_AT_CHARS)?,
            details: LocalModelDetails {
                format: valid_text(model.details.format, MAX_DETAIL_CHARS)?,
                family: valid_text(model.details.family, MAX_DETAIL_CHARS)?,
                parameter_size: valid_text(model.details.parameter_size, MAX_DETAIL_CHARS)?,
                quantization_level: valid_text(model.details.quantization_level, MAX_DETAIL_CHARS)?,
            },
        });
    }
    normalized.sort_by(|left, right| left.name.cmp(&right.name));
    Some(normalized)
}

fn valid_text(value: String, max_chars: usize) -> Option<String> {
    let chars = value.chars().count();
    if chars == 0
        || chars > max_chars
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return None;
    }
    Some(value)
}

fn valid_digest(value: String) -> Option<String> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    Some(value)
}
