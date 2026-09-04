use std::fmt;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod artifact;
mod candidate;
mod check_preflight;
mod declared_checks;
#[cfg(unix)]
mod disk_encryption;
mod dispatch;
mod evaluation;
mod git_object_store;
mod local_model_activation;
mod mission;
mod mission_completion;
mod mission_impact;
mod operator_account;
mod outcome;
mod outcome_collector;
mod platform_session;
mod project;
mod project_policy;
mod project_repository;
mod proposal;
mod recent_receipts;
mod resident_configuration;
mod resident_lifecycle;
mod resource_discovery;
mod ruling;
mod sample_project;
mod sandbox;
mod spend;
mod starter_organization;
mod team;
mod worktree;

#[cfg(unix)]
mod local_transport;

pub use check_preflight::{
    CheckPreflightError, CheckPreflightService, PreflightOutcome, PreflightReceipt, Recorded,
    ResolvedCheck, UnavailableEvidence,
};
#[cfg(unix)]
pub use declared_checks::{
    DeclaredCheckError, DeclaredCheckService, ExecutionReceipt, ExecutionSummary,
};
#[cfg(unix)]
pub use disk_encryption::{
    DiskEncryptionService, EncryptionEvidence, LiveDataEncryption, LiveDataEncryptionConclusion,
    LiveDataEncryptionReport, MAX_RENDERED_CONCLUSION_CHARS, ReasonCode, Subject, SubjectRole,
    SystemView, disk_encryption_detection_candidate_for_test, live_data_encryption_for_test,
    render_conclusion,
};
pub use local_transport::{
    LocalErrorCode, LocalServerConfig, LocalSupervisorClient, LocalSupervisorServer,
    LocalTransportError, initialize_local_auth_token,
};
#[cfg(unix)]
pub use recent_receipts::{
    DEFAULT_RECENT_RUN_WINDOW, MAX_RECENT_RUN_WINDOW, RecentReceipt, RecentReceiptError,
    RecentReceiptOutcome, RecentReceiptPage, RecentReceiptService,
};
pub use sandbox::{
    Ceiling, ExecutionBounds, ExecutionOutcome, ExecutionRecord, Interrupt, ProbeObservation,
    SandboxCapability, SandboxUnavailableReason, detect as detect_sandbox, interrupt_channel,
    run_bounded_for_test,
};

pub use artifact::{
    Artifact, ArtifactContext, ArtifactDigest, ArtifactKind, ArtifactV1, MalformedArtifact,
};
pub use candidate::{
    Candidate, CandidateDigest, CandidateEntry, CandidateError, CandidateImages, CandidateService,
    CandidateV1, InconsistentCandidate, MalformedCandidate,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use dispatch::DispatchFailpoints;
pub use dispatch::{
    DispatchActor, DispatchError, DispatchOutcome, DispatchReceipt, DispatchService, GitStep,
    GitStepName, GitStepResult, MutationEffect, MutationRecord, NotVerified, PauseService,
    RepositoryEffect, SourceIdentity, SuppliedContent, UnresolvedRuns,
};
pub use evaluation::{
    Conflicts, Evaluation, EvaluationDigest, EvaluationDraft, EvaluationError, EvaluationForce,
    EvaluationService, EvaluationSupersedes, EvaluationV1, Evaluator, Finding, FindingSeverity,
    InconsistentEvaluation, MalformedEvaluation, RubricDeclaration, Verdict,
};
pub use local_model_activation::{
    LocalModelActivation, LocalModelActivationError, LocalModelActivationReview,
    LocalModelActivationService, LocalModelActivationState, LocalModelBinding, ProviderFeature,
};
pub use mission::{
    MissionAdmissionActor, MissionDraft, MissionError, MissionLifecycle, MissionMemberDraft,
    MissionMemberView, MissionService, MissionView,
};
pub use mission_completion::{
    COMPLETION_CANONICALIZER, CompletionActor, CompletionDigest, CompletionDisposition,
    CompletionSupersedes, ConditionOutcome, ConditionResultRecord, MalformedCompletion,
    MissionCompletionV1, NarrativeItem, SpendReference, SupportKind, SupportRef, UncertaintyKind,
    UnfinishedKind,
};
pub use mission_impact::{
    CANONICALIZER as MISSION_IMPACT_CANONICALIZER, ClassificationActor, ClassificationBasis,
    ClassificationDigest, ClassificationSupersedes, ImpactLevel, MalformedClassification,
    MissionImpactClassificationV1, PolicyRevisionRef,
};
pub use mission_impact::{
    ClassificationRecordError, ClassificationStoreError, MissionImpactClassification,
    MissionImpactService, verified_classification_for_citation, verified_classification_within,
};
pub use operator_account::{
    AccessOrigin, OperatorAccountError, OperatorAccountService, OperatorSessionPhase,
    OperatorSessionPolicy, OperatorSessionStatus, PlatformSessionEvent, RecoveryKey, SecretString,
};
pub use outcome::{
    AppendixMode, AppendixRequest, BoundRefusal, ConditionResult, MalformedOutcome, NarrativeRef,
    OutcomeBound, OutcomeManifestV1, OutcomeRefusal, OutcomeState, PinnedRevision, ReceiptRef,
    RevisionKind, SourceDigest, SourceKind, SourceMismatch, SourceRef, SpendRef, SuccessCondition,
    VerifiedArtifact, VerifiedSources, VerifiedText, compare_canonical_keys, render,
};
pub use outcome_collector::{
    CollectedDigest, CollectedSources, CollectionRequest, Collector, CollectorRefusal,
    MissingReader, OUTCOME_RECEIPT_CHECKSUM_LABEL,
};
pub use platform_session::{
    PlatformMonitorState, PlatformSessionMonitor, PlatformSessionMonitorStatus,
};
pub use project::{
    AutonomyProfile, BudgetBoundary, ExternalEffects, ProjectCharter, ProjectCharterDraft,
    ProjectDraft, ProjectError, ProjectService, ProjectSummary, ProjectView,
};
pub use project_policy::{
    PolicyAuthor, ProjectPolicy, ProjectPolicyDraft, ProjectPolicyError, ProjectPolicyService,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TrackRecordEvidenceKind, TrackRecordRequirement,
};
pub use project_repository::{
    DeclaredCheck, InvalidRepositoryDraft, ProjectRepository, ProjectRepositoryError,
    ProjectRepositoryService, RepositoryAuthor, RepositoryDraft, RepositorySourceKind,
};
pub use proposal::{
    BudgetEnvelope, ConstraintBinding, ContextComparison, ContextEntry, ContextMismatch,
    ContextMismatchReason, DeclaredMutation, InconsistentProposal, MalformedProposal, MutationKind,
    OwnerBootstrap, Proposal, ProposalActor, ProposalBindings, ProposalBounds, ProposalDigest,
    ProposalError, ProposalIntent, ProposalReadiness, ProposalRequester, ProposalRequirements,
    ProposalScope, ProposalService, ProposalV1, ReadableContext, RollbackStrategy,
    UnmetPrecondition,
};
pub use resident_configuration::{
    BeaconCapability, BeaconCapabilityDescriptor, ResidentConfiguration,
    ResidentConfigurationBinding, ResidentConfigurationError, ResidentConfigurationService,
    ResidentConfigurationState, TeamIneligibilityReason,
};
pub use resident_lifecycle::{
    ResidentAgentView, ResidentLifecycle, ResidentLifecycleActor, ResidentLifecycleDecision,
    ResidentLifecycleError, ResidentLifecycleService,
};
pub use resource_discovery::{
    DiscoveredLocalModel, DiscoveryStatus, LocalModelDetails, LocalModelDiscoveryReport,
    LocalModelEndpoint, ModelActivation, ModelProvider, ResourceDiscoveryError,
    ResourceDiscoveryService,
};
pub use ruling::{
    InconsistentRuling, MalformedRuling, Ruling, RulingDecider, RulingDecision, RulingDigest,
    RulingError, RulingForce, RulingGrammar, RulingService, RulingSupersedes, RulingV1,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use sample_project::CopyFailpoints;
pub use sample_project::{
    SAMPLE_PROJECT_BUNDLE_VERSION, SampleProjectDigest, SampleProjectError,
    SampleProjectInstallOutcome, SampleProjectInstallResult, SampleProjectInstallation,
    SampleProjectProvenance, SampleProjectService, SampleProjectStatus,
};
pub use spend::{
    CANONICALIZER as SPEND_CANONICALIZER, MalformedSpend, MeasuredSpend, MeasuredSpendService,
    MeasuredSpendV1, SpendDigest, SpendLine, SpendMeasurement, SpendOrigin, SpendRecordError,
    SpendStoreError, SpendSupersedes, SpendTotals, UncountedEntry, UncountedKind, compare_units,
    verified_spend_for_citation, verified_spend_within,
};
pub use starter_organization::{
    ResidentAgentSummary, StarterOrganizationDecision, StarterOrganizationError,
    StarterOrganizationService, StarterOrganizationView, StarterResidentDraft, StarterRole,
    StarterRoleTemplate,
};
pub use team::{
    TeamDraft, TeamError, TeamFormationActor, TeamMemberDraft, TeamMemberView, TeamService,
    TeamView,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use worktree::set_pinned_spawn_hook;
pub use worktree::{CleanupOutcome, KeepReason, Worktree, WorktreeError};

const SCHEMA_VERSION: i64 = 26;
const MAX_WORKSPACE_NAME_CHARS: usize = 120;

const SCHEMA_V1: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_V2: &str = include_str!("../migrations/0002_operator_account.sql");
const MIGRATION_V3: &str = include_str!("../migrations/0003_session_policy.sql");
const MIGRATION_V4: &str = include_str!("../migrations/0004_starter_organization.sql");
const MIGRATION_V5: &str = include_str!("../migrations/0005_projects.sql");
const MIGRATION_V6: &str = include_str!("../migrations/0006_project_charter_revisions.sql");
const MIGRATION_V7: &str = include_str!("../migrations/0007_local_model_activations.sql");
const MIGRATION_V8: &str = include_str!("../migrations/0008_resident_lifecycle.sql");
const MIGRATION_V9: &str = include_str!("../migrations/0009_resident_configurations.sql");
const MIGRATION_V10: &str = include_str!("../migrations/0010_teams.sql");
const MIGRATION_V11: &str = include_str!("../migrations/0011_missions.sql");
const MIGRATION_V12: &str = include_str!("../migrations/0012_project_policy.sql");
const MIGRATION_V13: &str = include_str!("../migrations/0013_project_repository.sql");
const MIGRATION_V14: &str = include_str!("../migrations/0014_run_receipt.sql");
const MIGRATION_V15: &str = include_str!("../migrations/0015_declared_check_runs.sql");
// 0016 and 0017 were once concatenated into one `MIGRATION_V16` for the bootstrap paths, while the
// schema check kept them apart so a declared v16 store could be verified before the additive 0017
// was applied. The ladder keeps them apart for everyone, which is the same behaviour with one
// spelling instead of two.
const MIGRATION_V16_ONLY: &str = include_str!("../migrations/0016_execution_receipt_v2.sql");
const MIGRATION_V17: &str = include_str!("../migrations/0017_sample_project.sql");
const MIGRATION_V18: &str = include_str!("../migrations/0018_proposals.sql");
const MIGRATION_V19: &str = include_str!("../migrations/0019_rulings.sql");
const MIGRATION_V20: &str = include_str!("../migrations/0020_recent_receipts_index.sql");
// Dispatch owns 0022 (FABLE-RULINGS-2026-09-02 R5) and landed first, so Evaluation --
// which held 0021 while the two lanes ran in parallel -- renumbers to 0023 at its final
// rebase, exactly as the note on this line said whichever landed second would.
const MIGRATION_V22: &str = include_str!("../migrations/0022_candidates.sql");
const MIGRATION_V23: &str = include_str!("../migrations/0023_evaluations.sql");
const MIGRATION_V24: &str = include_str!("../migrations/0024_mission_success_conditions.sql");
// 0024 landed first (the Mission-completion lane's success conditions), so this one took 0025 at
// its rebase, exactly as the note on it said whichever landed second would, and the ladder steps
// 24 -> 25.
const MIGRATION_V25: &str = include_str!("../migrations/0025_spend_and_impact.sql");
// PROVISIONAL. 0024 and 0025 landed first, so this one took 0026 at its rebase. kimi's Artifact
// store is the remaining lane in this window and renumbers to 0027 when it lands; the ladder
// step below moves with the file, and the file says the same.
const MIGRATION_V26: &str = include_str!("../migrations/0026_mission_completions.sql");

#[derive(Debug, Clone)]
pub struct StartupConfig {
    pub store_path: PathBuf,
    pub initial_workspace_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceSummary {
    pub id: Uuid,
    pub name: String,
    pub created_at_ms: u64,
    pub created: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStatus {
    Ready,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreDiagnostics {
    pub status: DiagnosticStatus,
    pub schema_version: i64,
    pub integrity_check: String,
    pub foreign_key_violations: u64,
    pub workspace_count: u64,
    pub persisted_event_count: u64,
    pub security_event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupEvent {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupReport {
    pub store_path: String,
    pub workspace: WorkspaceSummary,
    pub diagnostics: StoreDiagnostics,
    pub events: Vec<StartupEvent>,
}

#[derive(Debug)]
pub enum SupervisorError {
    EmptyWorkspaceName,
    WorkspaceNameTooLong,
    WorkspaceNameContainsControl,
    ClockBeforeUnixEpoch,
    Io(std::io::Error),
    Store(rusqlite::Error),
    Json(serde_json::Error),
    InvalidWorkspaceId(String),
    InvalidEventKind(String),
    EventPayloadScopeMismatch(i64),
    StarterOrganizationEventMismatch(i64),
    StarterOrganizationStateMismatch,
    ProjectEventMismatch(i64),
    ProjectStateMismatch,
    LocalModelActivationEventMismatch(i64),
    LocalModelActivationStateMismatch,
    ResidentLifecycleEventMismatch(i64),
    ResidentLifecycleStateMismatch,
    ResidentConfigurationEventMismatch(i64),
    ResidentConfigurationStateMismatch,
    TeamEventMismatch(i64),
    TeamStateMismatch,
    MissionEventMismatch(i64),
    MissionStateMismatch,
    ProjectPolicyEventMismatch(i64),
    ProjectPolicyStateMismatch,
    ProjectRepositoryEventMismatch(i64),
    ProjectRepositoryStateMismatch,
    SampleProjectEventMismatch(i64),
    SampleProjectStateMismatch,
    RunEventMismatch(i64),
    RunStateMismatch,
    ProposalEventMismatch(i64),
    ProposalStateMismatch,
    RulingEventMismatch(i64),
    EvaluationEventMismatch(i64),
    RulingStateMismatch,
    CandidateEventMismatch(i64),
    CandidateStateMismatch,
    DispatchEventMismatch(i64),
    DispatchStateMismatch,
    EvaluationStateMismatch,
    PauseEventMismatch(i64),
    SchemaMismatch,
    UnsupportedSchemaVersion(i64),
    IntegrityCheckFailed(String),
    ForeignKeyViolations(u64),
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWorkspaceName => write!(formatter, "workspace name cannot be empty"),
            Self::WorkspaceNameTooLong => write!(
                formatter,
                "workspace name cannot exceed {MAX_WORKSPACE_NAME_CHARS} characters"
            ),
            Self::WorkspaceNameContainsControl => {
                write!(
                    formatter,
                    "workspace name cannot contain control characters"
                )
            }
            Self::ClockBeforeUnixEpoch => {
                write!(formatter, "system clock is before the Unix epoch")
            }
            Self::Io(error) => write!(formatter, "cannot prepare the store directory: {error}"),
            Self::Store(error) => write!(formatter, "store operation failed: {error}"),
            Self::Json(error) => write!(formatter, "cannot encode an event payload: {error}"),
            Self::InvalidWorkspaceId(value) => {
                write!(formatter, "store contains an invalid Workspace id: {value}")
            }
            Self::InvalidEventKind(value) => {
                write!(formatter, "store contains an unknown Event kind: {value}")
            }
            Self::EventPayloadScopeMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} payload does not match its Workspace scope"
            ),
            Self::StarterOrganizationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Starter Organization state"
            ),
            Self::StarterOrganizationStateMismatch => {
                write!(
                    formatter,
                    "Starter Organization state has no exact audit Event"
                )
            }
            Self::ProjectEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project creation state"
            ),
            Self::ProjectStateMismatch => {
                write!(formatter, "Project state has no exact creation audit Event")
            }
            Self::LocalModelActivationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match local-model activation state"
            ),
            Self::LocalModelActivationStateMismatch => write!(
                formatter,
                "local-model activation state has no exact audit Event"
            ),
            Self::ResidentLifecycleEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Resident lifecycle state"
            ),
            Self::ResidentLifecycleStateMismatch => write!(
                formatter,
                "Resident lifecycle state has no exact audit Event"
            ),
            Self::ResidentConfigurationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Resident configuration state"
            ),
            Self::ResidentConfigurationStateMismatch => write!(
                formatter,
                "Resident configuration state has no exact audit Event"
            ),
            Self::TeamEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Event sequence {sequence} does not match Team state"
                )
            }
            Self::TeamStateMismatch => {
                write!(formatter, "Team state has no exact formation audit Event")
            }
            Self::MissionEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Event sequence {sequence} does not match Mission admission state"
                )
            }
            Self::MissionStateMismatch => {
                write!(
                    formatter,
                    "Mission state has no exact admission audit Event"
                )
            }
            Self::ProjectPolicyEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project Policy state"
            ),
            Self::ProjectPolicyStateMismatch => {
                write!(formatter, "Project Policy state has no exact audit Event")
            }
            Self::ProjectRepositoryEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project Repository state"
            ),
            Self::ProjectRepositoryStateMismatch => {
                write!(
                    formatter,
                    "Project Repository state has no exact audit Event"
                )
            }
            Self::SampleProjectEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Sample Project installation state"
            ),
            Self::SampleProjectStateMismatch => {
                write!(
                    formatter,
                    "Sample Project state has no exact installation audit Event"
                )
            }
            Self::RunEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Run event {sequence} does not match its recorded Receipt"
                )
            }
            Self::RunStateMismatch => {
                write!(formatter, "a Run's state does not agree with its Receipt")
            }
            Self::ProposalEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Proposal bytes"
            ),
            Self::ProposalStateMismatch => {
                write!(
                    formatter,
                    "Proposal state does not agree with its bytes, references, or audit Event"
                )
            }
            Self::RulingEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Ruling bytes"
            ),
            Self::EvaluationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Evaluation bytes"
            ),
            Self::EvaluationStateMismatch => {
                write!(
                    formatter,
                    "Evaluation state does not agree with its bytes, the Proposal it binds, its \
                     findings and cited Runs, or its audit Event"
                )
            }
            Self::RulingStateMismatch => {
                write!(
                    formatter,
                    "Ruling state does not agree with its bytes, the Proposal it binds, its \
                     supersession chain, or its audit Event"
                )
            }
            Self::CandidateEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Candidate bytes"
            ),
            Self::CandidateStateMismatch => {
                write!(
                    formatter,
                    "Candidate state does not agree with its bytes, the Proposal it binds, or \
                     its audit Event"
                )
            }
            Self::DispatchEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored dispatch Receipt"
            ),
            Self::DispatchStateMismatch => write!(
                formatter,
                "an interrupted dispatch could not be finished from its preparation record"
            ),
            Self::PauseEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} is not a well-formed Pause record"
            ),

            Self::SchemaMismatch => {
                write!(
                    formatter,
                    "store schema does not match its declared version"
                )
            }
            Self::UnsupportedSchemaVersion(version) => {
                write!(formatter, "store schema version {version} is unsupported")
            }
            Self::IntegrityCheckFailed(result) => {
                write!(formatter, "SQLite integrity check failed: {result}")
            }
            Self::ForeignKeyViolations(count) => {
                write!(formatter, "store contains {count} foreign-key violation(s)")
            }
        }
    }
}

impl std::error::Error for SupervisorError {}

impl From<std::io::Error> for SupervisorError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for SupervisorError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for SupervisorError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

/// Owns store startup, first-Workspace creation, and the diagnostic report behind one interface.
pub struct Supervisor;

impl Supervisor {
    pub fn bootstrap(config: StartupConfig) -> Result<StartupReport, SupervisorError> {
        let workspace_name = validate_workspace_name(&config.initial_workspace_name)?;

        prepare_parent(&config.store_path)?;
        prepare_store_file(&config.store_path)?;

        let mut connection = Connection::open(&config.store_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // The immediate transaction prevents two starting supervisors from both creating the
        // first Workspace. Source:
        // https://docs.rs/rusqlite/0.40.2/rusqlite/struct.Connection.html#method.transaction_with_behavior
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // The ladder is applied from ONE list (`MIGRATION_LADDER`), for both an existing store and
        // a new one. It used to be stated three times -- twenty-four hand-written match arms, a
        // hand-written batch for a fresh store, and the test helper -- and the three could disagree
        // silently: a migration missing from one arm leaves stores at that one version refusing
        // startup while every other version chains through and looks fine. opus's O3 mutant deleted
        // a step from the `20 =>` arm and all 255 tests stayed green.
        let declared = if has_user_schema(&transaction)? {
            let declared = declared_schema_version(&transaction)?;
            if !MIGRATION_LADDER
                .iter()
                .any(|(produces, _)| *produces == declared)
            {
                return Err(SupervisorError::UnsupportedSchemaVersion(declared));
            }
            // The store is checked against the schema it CLAIMS before anything is applied to it,
            // so a mis-stamped store is refused rather than migrated from a shape it never had.
            verify_schema(&transaction, declared)?;
            declared
        } else {
            0
        };
        apply_migrations_above(&transaction, declared)?;
        verify_schema(&transaction, SCHEMA_VERSION)?;

        let existing = transaction
            .query_row(
                "SELECT id, name, created_at_ms
                 FROM workspaces
                 ORDER BY created_at_ms, id
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
            .optional()?;

        let (workspace_id, stored_name, created_at_ms, created) = match existing {
            Some((id, name, created_at_ms)) => (
                parse_workspace_id(id)?,
                name,
                nonnegative_u64(created_at_ms)?,
                false,
            ),
            None => {
                let id = Uuid::new_v4();
                let occurred_at_ms = unix_time_ms()?;
                transaction.execute(
                    "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, ?2, ?3)",
                    params![id.to_string(), workspace_name, occurred_at_ms],
                )?;
                let payload = serde_json::to_string(&WorkspaceCreatedPayload {
                    workspace_id: id,
                    name: workspace_name.to_owned(),
                })?;
                transaction.execute(
                    "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        id.to_string(),
                        PersistedEventKind::WorkspaceCreated.as_str(),
                        payload,
                        occurred_at_ms
                    ],
                )?;
                (
                    id,
                    workspace_name.to_owned(),
                    nonnegative_u64(occurred_at_ms)?,
                    true,
                )
            }
        };

        transaction.commit()?;

        let diagnostics = inspect_store(&connection)?;
        let workspace = WorkspaceSummary {
            id: workspace_id,
            name: stored_name,
            created_at_ms,
            created,
        };
        let state = if created { "created" } else { "reused" };
        let events = vec![
            StartupEvent {
                kind: "store.opened".to_owned(),
                detail: config.store_path.display().to_string(),
            },
            StartupEvent {
                kind: format!("workspace.{state}"),
                detail: workspace.id.to_string(),
            },
            StartupEvent {
                kind: "diagnostic.completed".to_owned(),
                detail: "store is ready".to_owned(),
            },
        ];

        Ok(StartupReport {
            store_path: config.store_path.display().to_string(),
            workspace,
            diagnostics,
            events,
        })
    }
}

fn validate_workspace_name(value: &str) -> Result<&str, SupervisorError> {
    if value.chars().any(char::is_control) {
        return Err(SupervisorError::WorkspaceNameContainsControl);
    }
    let value = value.trim();
    if value.is_empty() {
        return Err(SupervisorError::EmptyWorkspaceName);
    }
    if value.chars().count() > MAX_WORKSPACE_NAME_CHARS {
        return Err(SupervisorError::WorkspaceNameTooLong);
    }
    Ok(value)
}

fn prepare_parent(store_path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = store_path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn prepare_store_file(store_path: &Path) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(store_path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("store path is not a regular file"));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_store_file(_store_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

fn parse_workspace_id(value: String) -> Result<Uuid, SupervisorError> {
    Uuid::parse_str(&value).map_err(|_| SupervisorError::InvalidWorkspaceId(value))
}

#[derive(Debug, PartialEq, Eq)]
struct SchemaObject {
    object_type: String,
    name: String,
    table_name: String,
    sql: Option<String>,
}

fn has_user_schema(connection: &Connection) -> Result<bool, SupervisorError> {
    let exists: bool = connection.query_row(
        "
        SELECT EXISTS (
            SELECT 1
            FROM sqlite_schema
            WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
        )
        ",
        [],
        |row| row.get(0),
    )?;
    Ok(exists)
}

fn declared_schema_version(connection: &Connection) -> Result<i64, SupervisorError> {
    connection
        .query_row(
            "SELECT version FROM schema_meta WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(SupervisorError::Store)
}

/// The migration ladder as data: each step with the version it produces.
///
/// **One list.** Startup applies it, the schema check builds its expectation from it, and tests that
/// need a store at some version walk it -- so a new migration is added here and nowhere else. It was
/// three lists before opus's O3: twenty-four match arms, a fresh-store batch, and this table under
/// `#[cfg(test)]`, which read as though the problem were solved while production still walked the
/// hand-written copies. Three statements of one fact disagree silently, and the disagreement is
/// visible only to whoever is at the one version that was missed.
const MIGRATION_LADDER: &[(i64, &str)] = &[
    (1, SCHEMA_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
    (5, MIGRATION_V5),
    (6, MIGRATION_V6),
    (7, MIGRATION_V7),
    (8, MIGRATION_V8),
    (9, MIGRATION_V9),
    (10, MIGRATION_V10),
    (11, MIGRATION_V11),
    (12, MIGRATION_V12),
    (13, MIGRATION_V13),
    (14, MIGRATION_V14),
    (15, MIGRATION_V15),
    (16, MIGRATION_V16_ONLY),
    (17, MIGRATION_V17),
    (18, MIGRATION_V18),
    (19, MIGRATION_V19),
    (20, MIGRATION_V20),
    (22, MIGRATION_V22),
    (23, MIGRATION_V23),
    (24, MIGRATION_V24),
    (25, MIGRATION_V25),
    (26, MIGRATION_V26),
];

/// Applies the ladder up to and including `version` -- a store at that schema, from nothing.
pub(crate) fn apply_migrations_through(
    connection: &Connection,
    version: i64,
) -> Result<(), SupervisorError> {
    for (produces, migration) in MIGRATION_LADDER {
        if *produces <= version {
            connection.execute_batch(migration)?;
        }
    }
    Ok(())
}

/// Applies every step the ladder has above `version`. `0` is a store that does not exist yet, so
/// the same call serves a fresh store and a migrating one -- there is no second list to forget.
fn apply_migrations_above(connection: &Connection, version: i64) -> Result<(), SupervisorError> {
    for (produces, migration) in MIGRATION_LADDER {
        if *produces > version {
            connection.execute_batch(migration)?;
        }
    }
    Ok(())
}

/// Applies every migration in order, for tests that need a store at the current schema.
#[cfg(test)]
pub(crate) fn apply_every_migration(connection: &Connection) -> Result<(), SupervisorError> {
    apply_migrations_through(connection, SCHEMA_VERSION)
}

fn verify_schema(connection: &Connection, version: i64) -> Result<(), SupervisorError> {
    if !(1..=SCHEMA_VERSION).contains(&version) {
        return Err(SupervisorError::UnsupportedSchemaVersion(version));
    }
    // The expectation is built from the same one list startup applies, so "what this version should
    // look like" and "what startup produces" cannot drift apart.
    let expected_connection = Connection::open_in_memory()?;
    apply_migrations_through(&expected_connection, version)?;
    let expected = schema_signature(&expected_connection)?;
    let actual = schema_signature(connection)?;
    if actual != expected {
        return Err(SupervisorError::SchemaMismatch);
    }
    Ok(())
}

fn schema_signature(connection: &Connection) -> Result<Vec<SchemaObject>, SupervisorError> {
    let mut statement = connection.prepare(
        "
        SELECT type, name, tbl_name, sql
        FROM sqlite_schema
        WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
        ORDER BY type, name
        ",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok(SchemaObject {
                object_type: row.get(0)?,
                name: row.get(1)?,
                table_name: row.get(2)?,
                sql: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(objects)
}

fn unix_time_ms() -> Result<i64, SupervisorError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SupervisorError::ClockBeforeUnixEpoch)?;
    i64::try_from(duration.as_millis()).map_err(|_| SupervisorError::ClockBeforeUnixEpoch)
}

fn nonnegative_u64(value: i64) -> Result<u64, SupervisorError> {
    u64::try_from(value)
        .map_err(|_| SupervisorError::Store(rusqlite::Error::IntegralValueOutOfRange(0, value)))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceCreatedPayload {
    workspace_id: Uuid,
    name: String,
}

#[derive(Clone, Copy)]
enum PersistedEventKind {
    WorkspaceCreated,
    StarterOrganizationAccepted,
    StarterOrganizationDiscarded,
    ProjectCreated,
    ProjectCharterRevised,
    ProjectPolicyPublished,
    ProjectRepositoryBound,
    LocalModelActivated,
    LocalModelDeactivated,
    ResidentActivated,
    ResidentSuspended,
    ResidentReactivated,
    ResidentRetired,
    ResidentConfigurationSet,
    ResidentConfigurationCleared,
    TeamFormed,
    MissionAdmitted,
    RunPreflighted,
    RunExecuted,
    SampleProjectInstalled,
    ProposalSubmitted,
    RulingRecorded,
    ProposalCandidateRecorded,
    ProposalDispatched,
    InstallationPaused,
    InstallationResumed,
    EvaluationRecorded,
}

impl PersistedEventKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::WorkspaceCreated => "workspace.created",
            Self::StarterOrganizationAccepted => "starter_organization.accepted",
            Self::StarterOrganizationDiscarded => "starter_organization.discarded",
            Self::ProjectCreated => "project.created",
            Self::ProjectCharterRevised => "project.charter_revised",
            Self::ProjectPolicyPublished => "project.policy_published",
            Self::ProjectRepositoryBound => "project.repository_bound",
            Self::LocalModelActivated => "local_model.activated",
            Self::LocalModelDeactivated => "local_model.deactivated",
            Self::ResidentActivated => "resident.activated",
            Self::ResidentSuspended => "resident.suspended",
            Self::ResidentReactivated => "resident.reactivated",
            Self::ResidentRetired => "resident.retired",
            Self::ResidentConfigurationSet => "resident.configuration_set",
            Self::ResidentConfigurationCleared => "resident.configuration_cleared",
            Self::TeamFormed => "team.formed",
            Self::MissionAdmitted => "mission.admitted",
            Self::RunPreflighted => "run.preflighted",
            Self::RunExecuted => "run.executed",
            Self::SampleProjectInstalled => "sample_project.installed",
            Self::ProposalSubmitted => "proposal.submitted",
            Self::RulingRecorded => "ruling.recorded",
            Self::ProposalCandidateRecorded => "proposal.candidate_recorded",
            Self::ProposalDispatched => "proposal.dispatched",
            Self::InstallationPaused => "installation.paused",
            Self::InstallationResumed => "installation.resumed",
            Self::EvaluationRecorded => "evaluation.recorded",
        }
    }

    fn parse(value: String) -> Result<Self, SupervisorError> {
        match value.as_str() {
            "workspace.created" => Ok(Self::WorkspaceCreated),
            "starter_organization.accepted" => Ok(Self::StarterOrganizationAccepted),
            "starter_organization.discarded" => Ok(Self::StarterOrganizationDiscarded),
            "project.created" => Ok(Self::ProjectCreated),
            "project.charter_revised" => Ok(Self::ProjectCharterRevised),
            "project.policy_published" => Ok(Self::ProjectPolicyPublished),
            "project.repository_bound" => Ok(Self::ProjectRepositoryBound),
            "local_model.activated" => Ok(Self::LocalModelActivated),
            "local_model.deactivated" => Ok(Self::LocalModelDeactivated),
            "resident.activated" => Ok(Self::ResidentActivated),
            "resident.suspended" => Ok(Self::ResidentSuspended),
            "resident.reactivated" => Ok(Self::ResidentReactivated),
            "resident.retired" => Ok(Self::ResidentRetired),
            "resident.configuration_set" => Ok(Self::ResidentConfigurationSet),
            "resident.configuration_cleared" => Ok(Self::ResidentConfigurationCleared),
            "team.formed" => Ok(Self::TeamFormed),
            "mission.admitted" => Ok(Self::MissionAdmitted),
            "run.preflighted" => Ok(Self::RunPreflighted),
            "run.executed" => Ok(Self::RunExecuted),
            "sample_project.installed" => Ok(Self::SampleProjectInstalled),
            "proposal.submitted" => Ok(Self::ProposalSubmitted),
            "ruling.recorded" => Ok(Self::RulingRecorded),
            "proposal.candidate_recorded" => Ok(Self::ProposalCandidateRecorded),
            "proposal.dispatched" => Ok(Self::ProposalDispatched),
            "installation.paused" => Ok(Self::InstallationPaused),
            "installation.resumed" => Ok(Self::InstallationResumed),
            "evaluation.recorded" => Ok(Self::EvaluationRecorded),
            _ => Err(SupervisorError::InvalidEventKind(value)),
        }
    }
}

fn inspect_store(connection: &Connection) -> Result<StoreDiagnostics, SupervisorError> {
    let schema_version = connection.query_row(
        "SELECT version FROM schema_meta WHERE singleton = 1",
        [],
        |row| row.get(0),
    )?;
    if schema_version != SCHEMA_VERSION {
        return Err(SupervisorError::UnsupportedSchemaVersion(schema_version));
    }

    let integrity_check: String =
        connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity_check != "ok" {
        return Err(SupervisorError::IntegrityCheckFailed(integrity_check));
    }

    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut foreign_key_rows = statement.query([])?;
    let mut foreign_key_violations = 0_u64;
    while foreign_key_rows.next()?.is_some() {
        foreign_key_violations += 1;
    }
    if foreign_key_violations > 0 {
        return Err(SupervisorError::ForeignKeyViolations(
            foreign_key_violations,
        ));
    }

    let persisted_event_count = verify_persisted_events(connection)?;
    starter_organization::verify_starter_state(connection)
        .map_err(|_| SupervisorError::StarterOrganizationStateMismatch)?;
    project::verify_project_state(connection).map_err(|_| SupervisorError::ProjectStateMismatch)?;
    project_policy::verify_project_policy_state(connection)
        .map_err(|_| SupervisorError::ProjectPolicyStateMismatch)?;
    project_repository::verify_project_repository_state(connection)
        .map_err(|_| SupervisorError::ProjectRepositoryStateMismatch)?;
    sample_project::verify_sample_project_state(connection)
        .map_err(|_| SupervisorError::SampleProjectStateMismatch)?;
    proposal::verify_proposal_state(connection)
        .map_err(|_| SupervisorError::ProposalStateMismatch)?;
    // After Proposal state, because every Ruling binds one and re-verifies what it copied against
    // that Proposal's own bytes.
    ruling::verify_ruling_state(connection).map_err(|_| SupervisorError::RulingStateMismatch)?;
    // After Proposal state for the same reason a Ruling is: a Candidate copies one Proposal's
    // declared effects and re-verifies them against that Proposal's own bytes.
    candidate::verify_candidate_state(connection)
        .map_err(|_| SupervisorError::CandidateStateMismatch)?;
    // Before the Run-state check, because it is what turns an interrupted dispatch into a Run
    // that HAS a terminal Receipt: an attempt that prepared a commit and never recorded what
    // became of it is finished here, from its durable preparation and the repository's own ref.
    dispatch::finish_prepared_dispatches(connection)
        .map_err(|_| SupervisorError::DispatchStateMismatch)?;
    evaluation::verify_evaluation_state(connection)
        .map_err(|_| SupervisorError::EvaluationStateMismatch)?;
    // A Run whose lease expired without a terminal Receipt becomes unconfirmed
    // here. Lease expiry is what proves that state; the Run is never removed.
    check_preflight::verify_run_state(connection).map_err(|_| SupervisorError::RunStateMismatch)?;
    local_model_activation::verify_activation_state(connection)
        .map_err(|_| SupervisorError::LocalModelActivationStateMismatch)?;
    resident_lifecycle::verify_lifecycle_state(connection)
        .map_err(|_| SupervisorError::ResidentLifecycleStateMismatch)?;
    resident_configuration::verify_configuration_state(connection)
        .map_err(|_| SupervisorError::ResidentConfigurationStateMismatch)?;
    team::verify_team_state(connection).map_err(|_| SupervisorError::TeamStateMismatch)?;
    mission::verify_mission_state(connection).map_err(|_| SupervisorError::MissionStateMismatch)?;

    Ok(StoreDiagnostics {
        status: DiagnosticStatus::Ready,
        schema_version,
        integrity_check,
        foreign_key_violations,
        workspace_count: count_rows(connection, "workspaces")?,
        persisted_event_count,
        security_event_count: count_rows(connection, "security_events")?,
    })
}

fn verify_persisted_events(connection: &Connection) -> Result<u64, SupervisorError> {
    let mut statement = connection.prepare(
        "
        SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
        FROM events
        ORDER BY sequence
        ",
    )?;
    let mut rows = statement.query([])?;
    let mut count = 0_u64;
    while let Some(row) = rows.next()? {
        let sequence = row.get::<_, i64>(0)?;
        let workspace_id = parse_workspace_id(row.get::<_, String>(1)?)?;
        let kind = PersistedEventKind::parse(row.get::<_, String>(2)?)?;
        let payload_json = row.get::<_, String>(3)?;
        let occurred_at_ms = nonnegative_u64(row.get::<_, i64>(4)?)?;

        match kind {
            PersistedEventKind::WorkspaceCreated => {
                let payload: WorkspaceCreatedPayload = serde_json::from_str(&payload_json)?;
                if payload.workspace_id != workspace_id {
                    return Err(SupervisorError::EventPayloadScopeMismatch(sequence));
                }
                validate_workspace_name(&payload.name)?;
            }
            PersistedEventKind::StarterOrganizationAccepted => {
                starter_organization::verify_starter_event(
                    connection,
                    workspace_id,
                    true,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::StarterOrganizationEventMismatch(sequence))?;
            }
            PersistedEventKind::StarterOrganizationDiscarded => {
                starter_organization::verify_starter_event(
                    connection,
                    workspace_id,
                    false,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::StarterOrganizationEventMismatch(sequence))?;
            }
            PersistedEventKind::ProjectCreated => {
                project::verify_project_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectEventMismatch(sequence))?;
            }
            PersistedEventKind::ProjectCharterRevised => {
                project::verify_project_charter_revised_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectEventMismatch(sequence))?;
            }
            PersistedEventKind::ProjectPolicyPublished => {
                project_policy::verify_project_policy_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectPolicyEventMismatch(sequence))?;
            }
            PersistedEventKind::RunExecuted => {
                declared_checks::verify_execution_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::RunEventMismatch(sequence))?;
            }
            PersistedEventKind::RunPreflighted => {
                match check_preflight::verify_run_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                ) {
                    Ok(()) => {}
                    Err(check_preflight::RunEventReplayError::RunStateMismatch) => {
                        return Err(SupervisorError::RunStateMismatch);
                    }
                    Err(check_preflight::RunEventReplayError::EvidenceMismatch) => {
                        return Err(SupervisorError::RunEventMismatch(sequence));
                    }
                    Err(check_preflight::RunEventReplayError::Store(error)) => {
                        return Err(SupervisorError::Store(error));
                    }
                }
            }
            PersistedEventKind::ProjectRepositoryBound => {
                project_repository::verify_project_repository_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProjectRepositoryEventMismatch(sequence))?;
            }
            PersistedEventKind::SampleProjectInstalled => {
                sample_project::verify_sample_project_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::SampleProjectEventMismatch(sequence))?;
            }
            PersistedEventKind::ProposalSubmitted => {
                proposal::verify_proposal_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ProposalEventMismatch(sequence))?;
            }
            PersistedEventKind::EvaluationRecorded => {
                evaluation::verify_evaluation_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::EvaluationEventMismatch(sequence))?;
            }
            PersistedEventKind::RulingRecorded => {
                ruling::verify_ruling_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::RulingEventMismatch(sequence))?;
            }
            PersistedEventKind::ProposalCandidateRecorded => {
                candidate::verify_candidate_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::CandidateEventMismatch(sequence))?;
            }
            PersistedEventKind::InstallationPaused | PersistedEventKind::InstallationResumed => {
                dispatch::verify_pause_event(
                    matches!(kind, PersistedEventKind::InstallationPaused),
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::PauseEventMismatch(sequence))?;
            }
            PersistedEventKind::ProposalDispatched => {
                dispatch::verify_dispatch_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::DispatchEventMismatch(sequence))?;
            }
            PersistedEventKind::LocalModelActivated => {
                local_model_activation::verify_activation_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    LocalModelActivationState::Active,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::LocalModelActivationEventMismatch(sequence))?;
            }
            PersistedEventKind::LocalModelDeactivated => {
                local_model_activation::verify_activation_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    LocalModelActivationState::Inactive,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::LocalModelActivationEventMismatch(sequence))?;
            }
            PersistedEventKind::ResidentActivated
            | PersistedEventKind::ResidentSuspended
            | PersistedEventKind::ResidentReactivated
            | PersistedEventKind::ResidentRetired => {
                resident_lifecycle::verify_lifecycle_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    kind.as_str(),
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ResidentLifecycleEventMismatch(sequence))?;
            }
            PersistedEventKind::ResidentConfigurationSet => {
                resident_configuration::verify_configuration_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    ResidentConfigurationState::Configured,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ResidentConfigurationEventMismatch(sequence))?;
            }
            PersistedEventKind::ResidentConfigurationCleared => {
                resident_configuration::verify_configuration_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    ResidentConfigurationState::Cleared,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ResidentConfigurationEventMismatch(sequence))?;
            }
            PersistedEventKind::TeamFormed => {
                team::verify_team_event(connection, workspace_id, occurred_at_ms, &payload_json)
                    .map_err(|_| SupervisorError::TeamEventMismatch(sequence))?;
            }
            PersistedEventKind::MissionAdmitted => {
                mission::verify_mission_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::MissionEventMismatch(sequence))?;
            }
        }
        count += 1;
    }
    Ok(count)
}

fn count_rows(connection: &Connection, table: &str) -> Result<u64, SupervisorError> {
    let sql = match table {
        "workspaces" => "SELECT count(*) FROM workspaces",
        "security_events" => "SELECT count(*) FROM security_events",
        _ => unreachable!("count_rows is restricted to known tables"),
    };
    let count: i64 = connection.query_row(sql, [], |row| row.get(0))?;
    u64::try_from(count)
        .map_err(|_| SupervisorError::Store(rusqlite::Error::IntegralValueOutOfRange(0, count)))
}

#[cfg(test)]
mod tests {
    use crate::SCHEMA_VERSION;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rusqlite::{Connection, OptionalExtension};
    use uuid::Uuid;

    use super::{
        AutonomyProfile, BudgetBoundary, CheckPreflightService, DeclaredCheck, DiagnosticStatus,
        ExternalEffects, MIGRATION_V2, MIGRATION_V3, MIGRATION_V4, MIGRATION_V5, MIGRATION_V6,
        MIGRATION_V7, MIGRATION_V8, MIGRATION_V9, MIGRATION_V10, MIGRATION_V11, MIGRATION_V12,
        MIGRATION_V13, MIGRATION_V14, MIGRATION_V15, MIGRATION_V16_ONLY, MIGRATION_V17,
        MIGRATION_V18, MIGRATION_V19, MIGRATION_V20, MIGRATION_V22, MIGRATION_V23, MIGRATION_V24,
        MIGRATION_V25, MIGRATION_V26, OperatorAccountService, PreflightOutcome, PreflightReceipt,
        ProjectCharter, ProjectCharterDraft, ProjectDraft, ProjectRepositoryService,
        ProjectService, ProjectView, Recorded, RepositoryDraft, RepositorySourceKind,
        ResolvedCheck, SCHEMA_V1, SecretString, StartupConfig, Supervisor, SupervisorError,
        UnavailableEvidence,
    };

    struct TestStore {
        path: PathBuf,
    }

    impl TestStore {
        fn new(label: &str) -> Self {
            Self {
                path: std::env::temp_dir()
                    .join(format!("beacond-{label}-{}.sqlite3", Uuid::new_v4())),
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

    struct TestRepository {
        path: PathBuf,
    }

    impl TestRepository {
        fn new(label: &str) -> (Self, String) {
            let path =
                std::env::temp_dir().join(format!("beacond-{label}-repository-{}", Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            let git = |arguments: &[&str]| {
                let status = Command::new("git")
                    .args(arguments)
                    .current_dir(&path)
                    .env("GIT_AUTHOR_NAME", "Beacon Test")
                    .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                    .env("GIT_COMMITTER_NAME", "Beacon Test")
                    .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                    .status()
                    .unwrap();
                assert!(status.success(), "git {arguments:?} failed");
            };
            git(&["init", "--quiet"]);
            fs::write(path.join("tracked.txt"), "preflight evidence").unwrap();
            git(&["add", "."]);
            git(&["commit", "--quiet", "-m", "fixture"]);
            let output = Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(output.status.success());
            let revision = String::from_utf8(output.stdout).unwrap().trim().to_owned();
            (Self { path }, revision)
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn config(path: &Path, workspace_name: &str) -> StartupConfig {
        StartupConfig {
            store_path: path.to_owned(),
            initial_workspace_name: workspace_name.to_owned(),
        }
    }

    fn written_preflight(
        label: &str,
        repository: RepositoryDraft,
    ) -> (TestStore, PreflightReceipt) {
        let store = TestStore::new(label);
        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let accounts = Arc::new(OperatorAccountService::open(&store.path).unwrap());
        accounts
            .begin_password_setup(
                SecretString::new("a long local passphrase".to_owned()),
                SecretString::new("a long local passphrase".to_owned()),
            )
            .unwrap();
        accounts.complete_operator_setup(true).unwrap();
        let projects = ProjectService::new(Arc::clone(&accounts), report.workspace.id);
        let repositories =
            ProjectRepositoryService::new(Arc::clone(&accounts), report.workspace.id);
        let preflights = CheckPreflightService::new(accounts, report.workspace.id);
        let project = projects
            .create(ProjectDraft {
                name: "Beacon Alpha".to_owned(),
                charter: ProjectCharterDraft {
                    purpose: "Exercise exact startup reconciliation.".to_owned(),
                    desired_outcomes: vec!["Reject mismatched evidence".to_owned()],
                    constraints: vec!["Keep Workspace data local".to_owned()],
                    prohibitions: vec!["Do not execute".to_owned()],
                    autonomy_profile: AutonomyProfile::Supervised,
                },
            })
            .unwrap();
        repositories
            .bind(project.id, project.charter.revision, None, repository)
            .unwrap();
        let receipt = preflights.preflight(project.id).unwrap();

        (store, receipt)
    }

    fn written_refused_preflight(label: &str) -> (TestStore, PreflightReceipt) {
        written_preflight(
            label,
            RepositoryDraft {
                source_kind: RepositorySourceKind::GitUrl,
                source: "https://example.invalid/preflight-metadata.git".to_owned(),
                base_revision: "a".repeat(40),
                declared_checks: vec![DeclaredCheck {
                    name: "never".to_owned(),
                    argv: vec!["/bin/true".to_owned()],
                    working_directory: ".".to_owned(),
                }],
            },
        )
    }

    fn written_verified_preflight(label: &str) -> (TestStore, PreflightReceipt) {
        let (repository, revision) = TestRepository::new(label);
        let written = written_preflight(
            label,
            RepositoryDraft {
                source_kind: RepositorySourceKind::LocalPath,
                source: repository.path.to_string_lossy().into_owned(),
                base_revision: revision,
                declared_checks: vec![DeclaredCheck {
                    name: "never".to_owned(),
                    argv: vec!["/bin/true".to_owned()],
                    working_directory: ".".to_owned(),
                }],
            },
        );
        assert_eq!(written.1.outcome, PreflightOutcome::Verified);
        written
    }

    fn tamper_preflight_run_state(store: &TestStore, receipt: &PreflightReceipt, state: &str) {
        let connection = Connection::open(&store.path).unwrap();
        let trigger_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger' AND name = 'runs_single_terminal_transition'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute_batch("DROP TRIGGER runs_single_terminal_transition;")
            .unwrap();
        connection
            .execute(
                "UPDATE runs SET state = ?2 WHERE id = ?1",
                rusqlite::params![receipt.run_id.to_string(), state],
            )
            .unwrap();
        connection.execute_batch(&trigger_sql).unwrap();
    }

    fn preflight_sql_metadata_tamper_error(label: &str, update: &str) -> SupervisorError {
        let (store, receipt) = written_refused_preflight(label);

        let connection = Connection::open(&store.path).unwrap();
        let trigger_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_schema
                 WHERE type = 'trigger' AND name = 'receipts_no_update'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute_batch("DROP TRIGGER receipts_no_update;")
            .unwrap();
        connection
            .execute(update, [receipt.run_id.to_string()])
            .unwrap();
        connection.execute_batch(&trigger_sql).unwrap();
        drop(connection);

        Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap_err()
    }

    #[test]
    fn startup_rejects_preflight_sql_version_that_disagrees_with_v1_json() {
        let error = preflight_sql_metadata_tamper_error(
            "preflight-sql-version",
            "UPDATE receipts SET version = 2 WHERE run_id = ?1",
        );
        assert!(matches!(error, SupervisorError::RunEventMismatch(_)));
    }

    #[test]
    fn startup_rejects_preflight_event_bytes_that_differ_from_receipt_bytes() {
        let (store, receipt) = written_refused_preflight("preflight-event-bytes");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "UPDATE events SET payload_json = ' ' || payload_json
                 WHERE kind = 'run.preflighted'
                   AND json_extract(payload_json, '$.runId') = ?1",
                [receipt.run_id.to_string()],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap_err();

        assert!(matches!(error, SupervisorError::RunEventMismatch(_)));
    }

    #[test]
    fn startup_rejects_refused_preflight_evidence_with_verified_run_state() {
        let (store, receipt) = written_refused_preflight("refused-as-verified");
        assert_ne!(receipt.outcome, PreflightOutcome::Verified);
        tamper_preflight_run_state(&store, &receipt, "verified");

        let error = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap_err();

        assert!(matches!(error, SupervisorError::RunStateMismatch));
    }

    #[test]
    fn startup_rejects_verified_preflight_evidence_with_refused_run_state() {
        if !super::detect_sandbox().admits_execution() {
            eprintln!("skipped: this host reports no enforcing sandbox");
            return;
        }
        let (store, receipt) = written_verified_preflight("verified-as-refused");
        tamper_preflight_run_state(&store, &receipt, "refused");

        let error = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap_err();

        assert!(matches!(error, SupervisorError::RunStateMismatch));
    }

    #[test]
    fn startup_rejects_preflight_sql_outcome_that_disagrees_with_v1_json() {
        let error = preflight_sql_metadata_tamper_error(
            "preflight-sql-outcome",
            "UPDATE receipts SET outcome = 'completed' WHERE run_id = ?1",
        );
        assert!(matches!(error, SupervisorError::RunEventMismatch(_)));
    }

    #[test]
    fn bootstrap_creates_one_empty_workspace_and_reports_a_healthy_store() {
        let store = TestStore::new("fresh");

        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        assert!(report.workspace.created);
        assert_eq!(report.workspace.name, "Personal");
        assert_eq!(report.diagnostics.status, DiagnosticStatus::Ready);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.integrity_check, "ok");
        assert_eq!(report.diagnostics.foreign_key_violations, 0);
        assert_eq!(report.diagnostics.workspace_count, 1);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        assert_eq!(report.diagnostics.security_event_count, 0);
        assert_eq!(
            report
                .events
                .iter()
                .map(|event| event.kind.as_str())
                .collect::<Vec<_>>(),
            ["store.opened", "workspace.created", "diagnostic.completed"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn bootstrap_makes_the_store_owner_only() {
        let root = std::env::temp_dir().join(format!("beacon-store-{}", Uuid::new_v4()));
        let store_path = root.join("beacon.sqlite3");

        Supervisor::bootstrap(StartupConfig {
            store_path: store_path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();

        assert_eq!(
            fs::metadata(&store_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn restart_reuses_the_workspace_without_replaying_creation() {
        let store = TestStore::new("restart");
        let first = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        let second = Supervisor::bootstrap(config(&store.path, "Ignored on restart")).unwrap();

        assert_eq!(second.workspace.id, first.workspace.id);
        assert_eq!(second.workspace.name, "Personal");
        assert!(!second.workspace.created);
        assert_eq!(second.diagnostics.workspace_count, 1);
        assert_eq!(second.diagnostics.persisted_event_count, 1);
        assert_eq!(second.events[1].kind, "workspace.reused");
    }

    #[test]
    fn version_one_store_migrates_transactionally_without_replacing_workspace_state() {
        let store = TestStore::new("version-one-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        let workspace_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.workspace.name, "Existing");
        assert!(!report.workspace.created);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.workspace_count, 1);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        assert_eq!(report.diagnostics.security_event_count, 0);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM operator_accounts", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_two_store_migrates_account_and_security_history_with_default_policy() {
        let store = TestStore::new("version-two-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection
            .execute(
                "INSERT INTO operator_accounts (
                    singleton, role, password_hash, recovery_key_hash, created_at_ms, updated_at_ms
                 ) VALUES (1, 'owner', ?1, ?2, 7, 7)",
                rusqlite::params!["x".repeat(32), vec![1_u8; 32]],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO security_events (kind, detail_json, occurred_at_ms)
                 VALUES ('operator.enrolled', '{}', 7)",
                [],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();

        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.security_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT session_timeout_minutes FROM operator_accounts WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            30
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT kind FROM security_events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "operator.enrolled"
        );
    }

    #[test]
    fn version_four_store_adds_projects_without_replacing_existing_state() {
        let store = TestStore::new("version-four-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        let workspace_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM projects", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_five_store_adds_revision_events_without_replacing_project_state() {
        let store = TestStore::new("version-five-migration");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        let workspace_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO projects (
                    id, workspace_id, name, current_charter_revision, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'Existing Project', 1, 9, 9)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_charter_revisions (
                    workspace_id, project_id, revision, purpose, desired_outcomes_json,
                    constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                    external_effects, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 'Preserve this Project.', '[\"Remain intact\"]', '[]',
                           '[]', 'supervised', 'no_spend', 'prohibited', 'owner', 9)",
                rusqlite::params![workspace_id.to_string(), project_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_creation_snapshots (
                    project_id, workspace_id, initial_name, initial_charter_revision, created_at_ms
                 ) VALUES (?1, ?2, 'Existing Project', 1, 9)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.created', ?2, 9)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "project": {
                            "id": project_id,
                            "workspaceId": workspace_id,
                            "name": "Existing Project",
                            "charter": {
                                "revision": 1,
                                "purpose": "Preserve this Project.",
                                "desiredOutcomes": ["Remain intact"],
                                "constraints": [],
                                "prohibitions": [],
                                "autonomyProfile": "supervised",
                                "budgetBoundary": "no_spend",
                                "externalEffects": "prohibited",
                                "authoredAtMs": 9
                            },
                            "createdAtMs": 9
                        }
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 2);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    [project_id.to_string()],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "Existing Project"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM events WHERE kind = 'project.created'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
    }

    #[test]
    fn version_six_store_adds_the_empty_activation_ledger_without_replacing_workspace_state() {
        let store = TestStore::new("version-six-migration");
        let workspace_id = Uuid::new_v4();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.workspace.name, "Existing");
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM local_model_activation_decisions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_seven_store_adds_resident_lifecycle_without_replacing_admission() {
        let store = TestStore::new("version-seven-migration");
        let workspace_id = Uuid::new_v4();
        let resident_id = Uuid::new_v4();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection.execute_batch(MIGRATION_V7).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "name": "Existing"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO starter_organization_decisions (
                    workspace_id, proposal_version, decision, decided_at_ms
                 ) VALUES (?1, 1, 'accepted', 9)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO agents (
                    id, workspace_id, kind, admission_source, role, display_name_state,
                    display_name, lifecycle, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'resident', 'starter_organization', 'lead',
                           'absent', '', 'inactive', 9, 9)",
                rusqlite::params![resident_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO starter_organization_admissions (
                    workspace_id, agent_id, role, display_name_state, display_name, admitted_at_ms
                 ) VALUES (?1, ?2, 'lead', 'absent', '', 9)",
                rusqlite::params![workspace_id.to_string(), resident_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'starter_organization.accepted', ?2, 9)",
                rusqlite::params![
                    workspace_id.to_string(),
                    serde_json::json!({
                        "workspace_id": workspace_id,
                        "proposal_version": 1,
                        "residents": [{
                            "id": resident_id,
                            "role": "lead",
                            "displayName": null,
                            "lifecycle": "inactive"
                        }]
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 2);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT lifecycle FROM agents WHERE id = ?1",
                    [resident_id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "inactive"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM resident_lifecycle_decisions",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_eight_store_preserves_existing_events_when_adding_resident_configuration() {
        let store = TestStore::new("version-eight-migration");
        let workspace_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "workspace_id": workspace_id,
            "name": "Existing"
        })
        .to_string();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection.execute_batch(MIGRATION_V7).unwrap();
        connection.execute_batch(MIGRATION_V8).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        let event = connection
            .query_row(
                "SELECT sequence, kind, payload_json, occurred_at_ms FROM events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(event, (1, "workspace.created".to_owned(), payload, 7));
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM resident_configuration_decisions",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn version_nine_store_preserves_event_bytes_when_adding_empty_team_state() {
        let store = TestStore::new("version-nine-migration");
        let workspace_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "workspace_id": workspace_id,
            "name": "Existing"
        })
        .to_string();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection.execute_batch(MIGRATION_V7).unwrap();
        connection.execute_batch(MIGRATION_V8).unwrap();
        connection.execute_batch(MIGRATION_V9).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        let event = connection
            .query_row(
                "SELECT sequence, kind, payload_json, occurred_at_ms FROM events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(event, (1, "workspace.created".to_owned(), payload, 7));
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM teams", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM team_formation_members", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn version_eleven_store_preserves_event_bytes_when_adding_empty_project_policy_state() {
        let store = TestStore::new("version-eleven-policy-migration");
        let workspace_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "workspace_id": workspace_id,
            "name": "Existing"
        })
        .to_string();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection.execute_batch(MIGRATION_V7).unwrap();
        connection.execute_batch(MIGRATION_V8).unwrap();
        connection.execute_batch(MIGRATION_V9).unwrap();
        connection.execute_batch(MIGRATION_V10).unwrap();
        connection.execute_batch(MIGRATION_V11).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        let event = connection
            .query_row(
                "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms FROM events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            event,
            (
                1,
                workspace_id.to_string(),
                "workspace.created".to_owned(),
                payload,
                7
            )
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM project_policy_revisions", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        for trigger_name in [
            "team_formation_members_no_insert_after_formation",
            "mission_admission_members_no_insert_after_admission",
        ] {
            let trigger_sql = connection
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger_name],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert!(trigger_sql.contains("FROM events\n"));
            assert!(!trigger_sql.contains("events_v11"));
        }
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn version_twelve_store_preserves_event_bytes_when_adding_empty_project_repository_state() {
        let store = TestStore::new("version-twelve-repository-migration");
        let workspace_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "workspace_id": workspace_id,
            "name": "Existing"
        })
        .to_string();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        connection.execute_batch(MIGRATION_V2).unwrap();
        connection.execute_batch(MIGRATION_V3).unwrap();
        connection.execute_batch(MIGRATION_V4).unwrap();
        connection.execute_batch(MIGRATION_V5).unwrap();
        connection.execute_batch(MIGRATION_V6).unwrap();
        connection.execute_batch(MIGRATION_V7).unwrap();
        connection.execute_batch(MIGRATION_V8).unwrap();
        connection.execute_batch(MIGRATION_V9).unwrap();
        connection.execute_batch(MIGRATION_V10).unwrap();
        connection.execute_batch(MIGRATION_V11).unwrap();
        connection.execute_batch(MIGRATION_V12).unwrap();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        let event = connection
            .query_row(
                "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms FROM events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            event,
            (
                1,
                workspace_id.to_string(),
                "workspace.created".to_owned(),
                payload,
                7
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM project_repository_revisions",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        for trigger_name in [
            "team_formation_members_no_insert_after_formation",
            "mission_admission_members_no_insert_after_admission",
        ] {
            let trigger_sql = connection
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger_name],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert!(trigger_sql.contains("FROM events\n"));
            assert!(!trigger_sql.contains("events_v11"));
            assert!(!trigger_sql.contains("events_v12"));
        }
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn version_thirteen_store_preserves_event_bytes_when_adding_empty_run_state() {
        let store = TestStore::new("version-thirteen-run-migration");
        let workspace_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "workspace_id": workspace_id,
            "name": "Existing"
        })
        .to_string();
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.workspace.id, workspace_id);
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        let event = connection
            .query_row(
                "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms FROM events",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            event,
            (
                1,
                workspace_id.to_string(),
                "workspace.created".to_owned(),
                payload,
                7
            )
        );
        for table in ["runs", "receipts"] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        for trigger_name in [
            "team_formation_members_no_insert_after_formation",
            "mission_admission_members_no_insert_after_admission",
        ] {
            let trigger_sql = connection
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    [trigger_name],
                    |row| row.get::<_, String>(0),
                )
                .unwrap();
            assert!(trigger_sql.contains("FROM events\n"));
            assert!(!trigger_sql.contains("events_v12"));
            assert!(!trigger_sql.contains("events_v13"));
        }
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn populated_version_fourteen_store_preserves_run_and_receipt_bytes() {
        let store = TestStore::new("version-fourteen-populated-run-migration");
        let workspace_id = Uuid::new_v4();
        let project_id = Uuid::new_v4();
        let run_id = Uuid::new_v4();
        let base_revision = "0123456789abcdef0123456789abcdef01234567";
        let declared_checks = vec![DeclaredCheck {
            name: "test".to_owned(),
            argv: vec!["cargo".to_owned(), "test".to_owned()],
            working_directory: ".".to_owned(),
        }];
        let project_event_json = serde_json::json!({
            "project": ProjectView {
                id: project_id,
                workspace_id,
                name: "Migrated project".to_owned(),
                charter: ProjectCharter {
                    revision: 1,
                    purpose: "Preserve migration evidence".to_owned(),
                    desired_outcomes: vec!["survives".to_owned()],
                    constraints: Vec::new(),
                    prohibitions: Vec::new(),
                    autonomy_profile: AutonomyProfile::Observe,
                    budget_boundary: BudgetBoundary::NoSpend,
                    external_effects: ExternalEffects::Prohibited,
                    authored_at_ms: 11,
                },
                created_at_ms: 11,
            }
        })
        .to_string();
        let repository_event_json = serde_json::json!({
            "project": { "id": project_id },
            "repository": {
                "revision": 1,
                "charterRevision": 1,
                "sourceKind": "local_path",
                "source": "/tmp/existing",
                "baseRevision": base_revision,
                "declaredChecks": declared_checks,
                "authorKind": "operator",
                "authoredAtMs": 13
            }
        })
        .to_string();
        let receipt = PreflightReceipt {
            version: 1,
            run_id,
            workspace_id,
            project_id,
            repository_revision: 1,
            charter_revision: 1,
            outcome: PreflightOutcome::Verified,
            source_kind: RepositorySourceKind::LocalPath,
            source: "/tmp/existing".to_owned(),
            base_revision: base_revision.to_owned(),
            resolved_checks: vec![ResolvedCheck {
                name: "test".to_owned(),
                argv: vec!["cargo".to_owned(), "test".to_owned()],
                declared_working_directory: ".".to_owned(),
                resolved_working_directory: Recorded::Present {
                    value: "/tmp/existing".to_owned(),
                },
            }],
            sandbox_version: "fixture-v1".to_owned(),
            policy_charter_revision: 1,
            policy_repository_revision: 1,
            started_at_ms: 17,
            ended_at_ms: 19,
            not_verified: vec!["fixture gap".to_owned()],
            unavailable: UnavailableEvidence {
                proposal_digest: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                ruling_digest: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                resulting_revision: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                filesystem_effects: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                diff_digest: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                model_invocations: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
                cost_usd_micros: Recorded::Absent {
                    reason: "fixture gap".to_owned(),
                },
            },
        };
        let receipt_json = serde_json::to_string_pretty(&receipt).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO projects (
                     id, workspace_id, name, current_charter_revision, created_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, 'Migrated project', 1, 11, 11)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_charter_revisions (
                     workspace_id, project_id, revision, purpose, desired_outcomes_json,
                     constraints_json, prohibitions_json, autonomy_profile, budget_boundary,
                     external_effects, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 'Preserve migration evidence', '[\"survives\"]',
                           '[]', '[]', 'observe', 'no_spend', 'prohibited', 'owner', 11)",
                rusqlite::params![workspace_id.to_string(), project_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_creation_snapshots (
                     project_id, workspace_id, initial_name, initial_charter_revision, created_at_ms
                 ) VALUES (?1, ?2, 'Migrated project', 1, 11)",
                rusqlite::params![project_id.to_string(), workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.created', ?2, 11)",
                rusqlite::params![workspace_id.to_string(), project_event_json],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project_repository_revisions (
                     workspace_id, project_id, revision, charter_revision, source_kind, source,
                     base_revision, declared_checks_json, author_kind, authored_at_ms
                 ) VALUES (?1, ?2, 1, 1, 'local_path', '/tmp/existing', ?3,
                           ?4, 'operator', 13)",
                rusqlite::params![
                    workspace_id.to_string(),
                    project_id.to_string(),
                    base_revision,
                    serde_json::to_string(&declared_checks).unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'project.repository_bound', ?2, 13)",
                rusqlite::params![workspace_id.to_string(), repository_event_json],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runs (
                     workspace_id, id, project_id, kind, repository_revision, charter_revision,
                     state, lease_expires_at_ms, started_at_ms, ended_at_ms
                 ) VALUES (?1, ?2, ?3, 'check_preflight', 1, 1, 'verified', 23, 17, 19)",
                rusqlite::params![
                    workspace_id.to_string(),
                    run_id.to_string(),
                    project_id.to_string()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO receipts (
                     workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                 ) VALUES (?1, ?2, 1, 'verified', ?3, 19)",
                rusqlite::params![workspace_id.to_string(), run_id.to_string(), receipt_json],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'run.preflighted', ?2, 19)",
                rusqlite::params![workspace_id.to_string(), receipt_json],
            )
            .unwrap();

        let read_run = |connection: &Connection| {
            connection
                .query_row(
                    "SELECT workspace_id, id, project_id, kind, repository_revision,
                            charter_revision, state, lease_expires_at_ms, started_at_ms,
                            ended_at_ms
                     FROM runs",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, i64>(7)?,
                            row.get::<_, i64>(8)?,
                            row.get::<_, Option<i64>>(9)?,
                        ))
                    },
                )
                .unwrap()
        };
        let read_receipt = |connection: &Connection| {
            connection
                .query_row(
                    "SELECT workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
                     FROM receipts",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    },
                )
                .unwrap()
        };
        let run_before = read_run(&connection);
        let receipt_before = read_receipt(&connection);
        assert_eq!(receipt_before.4.as_bytes(), receipt_json.as_bytes());
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(read_run(&connection), run_before);
        assert_eq!(read_receipt(&connection), receipt_before);
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn version_sixteen_store_preserves_event_bytes_and_adds_empty_sample_state() {
        let store = TestStore::new("version-sixteen-sample-project-migration");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_json FROM events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            payload
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM sample_project_installations",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn migration_seventeen_rolls_back_exactly_before_forward_startup() {
        let store = TestStore::new("version-seventeen-rollback");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();

        let read_events = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
                     FROM events ORDER BY sequence",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_roster_triggers = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name, sql FROM sqlite_schema
                     WHERE type = 'trigger'
                       AND name IN (
                           'team_formation_members_no_insert_after_formation',
                           'mission_admission_members_no_insert_after_admission'
                       )
                     ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_sqlite_sequence = |connection: &Connection| {
            let mut statement = connection
                .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_foreign_key_violations = |connection: &Connection| {
            let mut statement = connection.prepare("PRAGMA foreign_key_check").unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        let schema_before = super::schema_signature(&connection).unwrap();
        let events_before = read_events(&connection);
        let roster_triggers_before = read_roster_triggers(&connection);
        let sqlite_sequence_before = read_sqlite_sequence(&connection);
        let foreign_keys_before = read_foreign_key_violations(&connection);
        assert_eq!(roster_triggers_before.len(), 2);
        assert!(foreign_keys_before.is_empty());

        connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
        connection.execute_batch(MIGRATION_V17).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            17
        );
        connection.execute_batch("ROLLBACK;").unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            16
        );
        assert_eq!(super::schema_signature(&connection).unwrap(), schema_before);
        assert_eq!(read_events(&connection), events_before);
        assert_eq!(read_roster_triggers(&connection), roster_triggers_before);
        assert_eq!(read_sqlite_sequence(&connection), sqlite_sequence_before);
        assert_eq!(
            read_foreign_key_violations(&connection),
            foreign_keys_before
        );
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        assert_eq!(read_events(&connection), events_before);
        assert!(read_foreign_key_violations(&connection).is_empty());
    }

    #[test]
    fn version_seventeen_store_preserves_event_bytes_and_adds_empty_proposal_state() {
        let store = TestStore::new("version-seventeen-proposal-migration");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        // The exact payload bytes, the exact sequence, and the exact timestamp survive the
        // closed-Event-table rebuild. A rebuild that reserialized the JSON would pass a
        // "one event" count and fail here.
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_json FROM events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            payload
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT occurred_at_ms FROM events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            7
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM proposals", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn migration_eighteen_advances_the_version_only_from_seventeen() {
        // The guarded UPDATE is what stops a store that never ran the rebuild from being stamped
        // as though it had. Applying the migration to a v16 store leaves the version alone.
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        let version = |connection: &Connection| {
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(version(&connection), 16);
        connection.execute_batch(MIGRATION_V18).unwrap();
        assert_eq!(
            version(&connection),
            16,
            "a v16 store must not be stamped v18 by a migration whose rebuild it never ran"
        );

        // The same migration on a store that did run v17 advances it, once. The guard is on the
        // stamp, not on the DDL, which is why every application runs inside one transaction and
        // the bootstrap ladder gates on the declared version before applying anything.
        connection.execute_batch(MIGRATION_V17).unwrap();
        assert_eq!(version(&connection), 17);
        let second = Connection::open_in_memory().unwrap();
        second.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
        ] {
            second.execute_batch(migration).unwrap();
        }
        assert_eq!(version(&second), 17);
        second.execute_batch(MIGRATION_V18).unwrap();
        assert_eq!(version(&second), 18);
    }

    #[test]
    fn migration_eighteen_rolls_back_exactly_before_forward_startup() {
        let store = TestStore::new("version-eighteen-rollback");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();

        let read_events = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
                     FROM events ORDER BY sequence",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_roster_triggers = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name, sql FROM sqlite_schema
                     WHERE type = 'trigger'
                       AND name IN (
                           'team_formation_members_no_insert_after_formation',
                           'mission_admission_members_no_insert_after_admission'
                       )
                     ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_sqlite_sequence = |connection: &Connection| {
            let mut statement = connection
                .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_foreign_key_violations = |connection: &Connection| {
            let mut statement = connection.prepare("PRAGMA foreign_key_check").unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        let schema_before = super::schema_signature(&connection).unwrap();
        let events_before = read_events(&connection);
        let roster_triggers_before = read_roster_triggers(&connection);
        let sqlite_sequence_before = read_sqlite_sequence(&connection);
        let foreign_keys_before = read_foreign_key_violations(&connection);
        assert_eq!(roster_triggers_before.len(), 2);
        assert!(foreign_keys_before.is_empty());

        connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
        connection.execute_batch(MIGRATION_V18).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            18
        );
        connection.execute_batch("ROLLBACK;").unwrap();

        // The exact prior schema and evidence, including both roster guards the rebuild drops
        // and recreates, are restored before any forward startup is attempted.
        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            17
        );
        assert_eq!(super::schema_signature(&connection).unwrap(), schema_before);
        assert_eq!(read_events(&connection), events_before);
        assert_eq!(read_roster_triggers(&connection), roster_triggers_before);
        assert_eq!(read_sqlite_sequence(&connection), sqlite_sequence_before);
        assert_eq!(
            read_foreign_key_violations(&connection),
            foreign_keys_before
        );
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        assert_eq!(read_events(&connection), events_before);
        assert!(read_foreign_key_violations(&connection).is_empty());
    }

    #[test]
    fn version_eighteen_store_preserves_event_bytes_and_adds_empty_ruling_state() {
        let store = TestStore::new("version-eighteen-ruling-migration");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
            MIGRATION_V18,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        // The exact payload bytes, sequence, and timestamp survive the closed-Event-table rebuild.
        // A rebuild that reserialized the JSON would pass a count check and fail here.
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload_json FROM events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            payload
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT occurred_at_ms FROM events WHERE sequence = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            7
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rulings", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |row| row
                    .get::<_, String>(0))
                .optional()
                .unwrap(),
            None
        );
    }

    #[test]
    fn migration_nineteen_advances_the_version_only_from_eighteen() {
        // The guarded UPDATE is what stops a store that never ran the rebuild from being stamped as
        // though it had. Applying 0019 to a v17 store must leave the version alone.
        let ladder = [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
        ];
        let version = |connection: &Connection| {
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        };

        let first = Connection::open_in_memory().unwrap();
        first.execute_batch(SCHEMA_V1).unwrap();
        for migration in ladder {
            first.execute_batch(migration).unwrap();
        }
        assert_eq!(version(&first), 17);
        // Out of order, 0019 does not merely decline to stamp the version -- it cannot run at all,
        // because it indexes the `proposals` table 0018 creates. Refusing is the stronger answer:
        // a store that skipped a migration never reaches the guarded UPDATE.
        assert!(
            first.execute_batch(MIGRATION_V19).is_err(),
            "0019 applied to a v17 store without its 0018 predecessor"
        );
        assert_eq!(
            version(&first),
            17,
            "a v17 store must not be stamped v19 by a migration whose rebuild it never ran"
        );

        // The same migration on a store that did run 0018 advances it, once. The guard is on the
        // stamp, not the DDL, which is why the bootstrap ladder gates on the declared version.
        let second = Connection::open_in_memory().unwrap();
        second.execute_batch(SCHEMA_V1).unwrap();
        for migration in ladder {
            second.execute_batch(migration).unwrap();
        }
        second.execute_batch(MIGRATION_V18).unwrap();
        assert_eq!(version(&second), 18);
        second.execute_batch(MIGRATION_V19).unwrap();
        assert_eq!(version(&second), 19);
    }

    #[test]
    fn migration_nineteen_rolls_back_exactly_before_forward_startup() {
        let store = TestStore::new("version-nineteen-rollback");
        let workspace_id = Uuid::new_v4();
        let payload = format!("{{ \"workspace_id\": \"{workspace_id}\", \"name\": \"Existing\" }}");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
            MIGRATION_V18,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Existing', 7)",
                [workspace_id.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 VALUES (?1, 'workspace.created', ?2, 7)",
                rusqlite::params![workspace_id.to_string(), payload],
            )
            .unwrap();

        let read_events = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
                     FROM events ORDER BY sequence",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_roster_triggers = |connection: &Connection| {
            let mut statement = connection
                .prepare(
                    "SELECT name, sql FROM sqlite_schema
                     WHERE type = 'trigger'
                       AND name IN (
                           'team_formation_members_no_insert_after_formation',
                           'mission_admission_members_no_insert_after_admission'
                       )
                     ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_sqlite_sequence = |connection: &Connection| {
            let mut statement = connection
                .prepare("SELECT name, seq FROM sqlite_sequence ORDER BY name")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let read_foreign_key_violations = |connection: &Connection| {
            let mut statement = connection.prepare("PRAGMA foreign_key_check").unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };

        let schema_before = super::schema_signature(&connection).unwrap();
        let events_before = read_events(&connection);
        let roster_triggers_before = read_roster_triggers(&connection);
        let sqlite_sequence_before = read_sqlite_sequence(&connection);
        let foreign_keys_before = read_foreign_key_violations(&connection);
        assert_eq!(roster_triggers_before.len(), 2);
        assert!(foreign_keys_before.is_empty());

        connection.execute_batch("BEGIN IMMEDIATE;").unwrap();
        connection.execute_batch(MIGRATION_V19).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            19
        );
        connection.execute_batch("ROLLBACK;").unwrap();

        // The exact prior schema and evidence, including both roster guards the rebuild drops
        // and recreates, are restored before any forward startup is attempted.
        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            18
        );
        assert_eq!(super::schema_signature(&connection).unwrap(), schema_before);
        assert_eq!(read_events(&connection), events_before);
        assert_eq!(read_roster_triggers(&connection), roster_triggers_before);
        assert_eq!(read_sqlite_sequence(&connection), sqlite_sequence_before);
        assert_eq!(
            read_foreign_key_violations(&connection),
            foreign_keys_before
        );
        drop(connection);

        let report = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();
        assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        assert_eq!(report.diagnostics.persisted_event_count, 1);
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        assert_eq!(read_events(&connection), events_before);
        assert!(read_foreign_key_violations(&connection).is_empty());
    }

    /// Build a store at the current schema and plant one Workspace, so a Ruling row can be
    /// exercised directly against the constraints without the whole Proposal fixture chain.
    fn ruling_schema_with_workspace() -> (Connection, String) {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
            MIGRATION_V18,
            MIGRATION_V19,
            MIGRATION_V20,
            MIGRATION_V22,
            MIGRATION_V23,
            MIGRATION_V24,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        let workspace_id = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 7)",
                [&workspace_id],
            )
            .unwrap();
        (connection, workspace_id)
    }

    fn insert_ruling(connection: &Connection, workspace_id: &str, digest: &str, proposal: &str) {
        connection
            .execute(
                "INSERT INTO rulings (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, proposal_digest, proposal_digest_algorithm,
                     proposal_digest_canonicalizer, charter_revision, project_policy_revision,
                     repository_revision, pinned_base_revision, decision, reason, decider_basis,
                     ruled_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.ruling.v1', ?2, x'626561636f6e2e72756c696e672e763100', ?3, 'sha256',
                           'beacon.proposal.v1', 1, 1, 1, ?4, 'approve', 'Recorded by hand.',
                           'owner', 7)",
                rusqlite::params![workspace_id, digest, proposal, "a".repeat(40)],
            )
            .unwrap();
    }

    #[test]
    fn a_recorded_ruling_refuses_update_and_delete() {
        // The immutability triggers exist, but until something inserts a Ruling and tries to change
        // it, deleting either trigger leaves every test green -- which is what gpt2 measured. The
        // wrong answer is a store that lets a recorded decision be edited or erased afterwards.
        let (connection, workspace_id) = ruling_schema_with_workspace();
        // Foreign keys are turned off deliberately and explicitly: this test is about the
        // immutability triggers, and building the whole Proposal parent chain would be testing
        // something else. `a_ruling_for_an_unknown_proposal_is_refused_by_the_store` is the test
        // that keeps the foreign key honest, with them on.
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        insert_ruling(&connection, &workspace_id, &"1".repeat(64), &"2".repeat(64));

        let updated = connection.execute("UPDATE rulings SET reason = 'edited'", []);
        let deleted = connection.execute("DELETE FROM rulings", []);

        assert!(
            updated
                .unwrap_err()
                .to_string()
                .contains("Rulings are immutable"),
            "a recorded Ruling accepted an UPDATE"
        );
        assert!(
            deleted
                .unwrap_err()
                .to_string()
                .contains("Rulings are immutable"),
            "a recorded Ruling accepted a DELETE"
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM rulings", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_ruling_for_an_unknown_proposal_is_refused_by_the_store() {
        // The wrong answer: a Ruling that binds a Proposal digest naming nothing, which would be a
        // decision about bytes this store has never seen.
        let (connection, workspace_id) = ruling_schema_with_workspace();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();

        let refused = connection.execute(
            "INSERT INTO rulings (
                 workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                 canonical_bytes, proposal_digest, proposal_digest_algorithm,
                 proposal_digest_canonicalizer, charter_revision, project_policy_revision,
                 repository_revision, pinned_base_revision, decision, reason, decider_basis,
                 ruled_at_ms
             ) VALUES (?1, 'sha256', 'beacon.ruling.v1', ?2, x'626561636f6e2e72756c696e672e763100', ?3, 'sha256',
                       'beacon.proposal.v1', 1, 1, 1, ?4, 'approve', 'Recorded by hand.',
                       'owner', 7)",
            rusqlite::params![workspace_id, "3".repeat(64), "4".repeat(64), "a".repeat(40)],
        );

        assert!(
            refused
                .unwrap_err()
                .to_string()
                .contains("FOREIGN KEY constraint failed"),
            "a Ruling bound a Proposal digest that names nothing"
        );
    }

    #[test]
    fn the_version_guard_alone_refuses_a_structurally_ready_store_that_is_not_stamped_eighteen() {
        // `migration_nineteen_advances_the_version_only_from_eighteen` cannot reach the guarded
        // UPDATE: against a real v17 schema the DDL fails first on the absent `proposals` table. So
        // the `AND version = 18` predicate itself was untested, and removing it stayed green.
        //
        // This builds a structurally v18 schema and then deliberately mis-stamps its version, so
        // the DDL succeeds and the predicate is the only thing left that can refuse the stamp.
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(SCHEMA_V1).unwrap();
        for migration in [
            MIGRATION_V2,
            MIGRATION_V3,
            MIGRATION_V4,
            MIGRATION_V5,
            MIGRATION_V6,
            MIGRATION_V7,
            MIGRATION_V8,
            MIGRATION_V9,
            MIGRATION_V10,
            MIGRATION_V11,
            MIGRATION_V12,
            MIGRATION_V13,
            MIGRATION_V14,
            MIGRATION_V15,
            MIGRATION_V16_ONLY,
            MIGRATION_V17,
            MIGRATION_V18,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .execute(
                "UPDATE schema_meta SET version = 17 WHERE singleton = 1",
                [],
            )
            .unwrap();

        connection.execute_batch(MIGRATION_V19).unwrap();

        assert_eq!(
            connection
                .query_row(
                    "SELECT version FROM schema_meta WHERE singleton = 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            17,
            "a store not stamped 18 was advanced to 19 anyway"
        );
    }

    /// Migration 0025 rebuilds the closed Event-kind table, and that rebuild is the one place a
    /// migration can silently delete history: a kind dropped from the CHECK takes every stored row
    /// that uses it.
    ///
    /// So this plants one Event of EVERY kind the store could hold before this migration -- the
    /// list read out of the parent's own SQL rather than retyped here, because a hand-written
    /// expected list is the same mistake one level up -- applies the migration, and looks for all of
    /// them afterwards. The parent is 0023: 0024 shipped between them and declares no kind list.
    #[test]
    fn the_event_kind_rebuild_carries_every_kind_and_its_rows() {
        let connection = Connection::open_in_memory().unwrap();
        // The store this migration actually meets: 24, the version `main` shipped before it. The
        // kind list's parent is still 0023, because 0024 declares none.
        super::apply_migrations_through(&connection, 24).unwrap();
        let workspace = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 1)",
                rusqlite::params![workspace],
            )
            .unwrap();
        let before = event_kinds_declared_in(MIGRATION_V23);
        assert!(
            before.len() >= 27,
            "0023 declares {} kinds, which is fewer than the store had",
            before.len()
        );
        for (offset, kind) in before.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                     VALUES (?1, ?2, json_object('workspace_id', ?1, 'name', 'Personal'), ?3)",
                    rusqlite::params![workspace, kind, offset as i64],
                )
                .unwrap();
        }

        connection.execute_batch(MIGRATION_V25).unwrap();

        // 25, not SCHEMA_VERSION: this test applies 0025 and nothing above it, so the schema it
        // produces is 25 whatever the newest migration happens to be. It read as SCHEMA_VERSION
        // while 0025 was the top, which is the same literal by coincidence rather than by meaning.
        super::verify_schema(&connection, 25)
            .expect("the rebuilt schema must match the expected one");
        let mut statement = connection
            .prepare("SELECT kind FROM events ORDER BY sequence")
            .unwrap();
        let survived = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            survived, before,
            "migration 0025 did not carry every stored Event across the rebuild, in order"
        );

        // And it admits exactly the two kinds this slice adds -- no more, so a third kind cannot
        // arrive unannounced in a rebuild nobody reads.
        let after = event_kinds_declared_in(MIGRATION_V25);
        let mut added: Vec<&str> = after
            .iter()
            .filter(|kind| !before.contains(kind))
            .map(String::as_str)
            .collect();
        added.sort_unstable();
        assert_eq!(added, ["mission.impact_classified", "spend.measured"]);
        assert_eq!(after.len(), before.len() + 2);
    }

    /// Every version the ladder can produce reaches the current schema.
    ///
    /// This is the test opus's O3 asked for, and it is only writable because the ladder is one list:
    /// it walks `MIGRATION_LADDER`, builds a real store at each version, and makes the real
    /// `Supervisor::bootstrap` migrate it. Before that list, a step missing from one hand-written
    /// arm left stores at exactly that version refusing startup -- loud for the operator who hits
    /// it, silent for everyone else, and invisible to a suite that only ever tests one arm.
    ///
    /// Each store is checked against the schema it claims BEFORE bootstrap, so a fixture cannot pass
    /// by building something no store was ever in.
    #[test]
    fn every_version_the_ladder_produces_reaches_the_current_schema() {
        assert!(
            super::MIGRATION_LADDER.len() >= 20,
            "the ladder lost its steps"
        );
        for (version, _) in super::MIGRATION_LADDER {
            let store = TestStore::new(&format!("ladder-{version}"));
            let connection = Connection::open(&store.path).unwrap();
            super::apply_migrations_through(&connection, *version).unwrap();
            super::verify_schema(&connection, *version)
                .unwrap_or_else(|_| panic!("the ladder must really produce a store at {version}"));
            assert_eq!(
                connection
                    .query_row(
                        "SELECT version FROM schema_meta WHERE singleton = 1",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                *version,
                "the step to {version} did not stamp the version it produces"
            );
            drop(connection);

            let report = Supervisor::bootstrap(config(&store.path, "Personal"))
                .unwrap_or_else(|error| panic!("a store at {version} must migrate: {error:?}"));
            assert_eq!(report.diagnostics.schema_version, SCHEMA_VERSION);
        }
    }

    /// A version the ladder never produces is refused, rather than migrated from a shape no store
    /// was ever in.
    #[test]
    fn a_version_between_two_ladder_steps_is_unsupported() {
        let between = 21;
        assert!(
            !super::MIGRATION_LADDER
                .iter()
                .any(|(produces, _)| *produces == between),
            "21 is a real version now -- pick another gap"
        );
        let store = TestStore::new("ladder-between");
        let connection = Connection::open(&store.path).unwrap();
        super::apply_migrations_through(&connection, 20).unwrap();
        connection
            .execute(
                "UPDATE schema_meta SET version = ?1 WHERE singleton = 1",
                rusqlite::params![between],
            )
            .unwrap();
        drop(connection);

        assert!(matches!(
            Supervisor::bootstrap(config(&store.path, "Personal")),
            Err(SupervisorError::UnsupportedSchemaVersion(v)) if v == between
        ));
    }

    /// Reads the Event kinds a migration's CHECK constraint admits, out of its own SQL.
    fn event_kinds_declared_in(sql: &str) -> Vec<String> {
        let start = sql
            .find("kind TEXT NOT NULL CHECK (kind IN (")
            .expect("the migration declares an Event-kind list");
        let end = sql[start..].find("))").expect("the kind list closes") + start;
        sql[start..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim().trim_end_matches(',');
                line.strip_prefix('\'')
                    .and_then(|rest| rest.strip_suffix('\''))
                    .map(str::to_owned)
            })
            .collect()
    }

    #[test]
    fn empty_workspace_name_is_refused_before_a_store_is_created() {
        let store = TestStore::new("empty-name");

        let error = Supervisor::bootstrap(config(&store.path, "   ")).unwrap_err();

        assert!(matches!(error, SupervisorError::EmptyWorkspaceName));
        assert!(!store.path.exists());
    }

    #[test]
    fn oversized_or_control_character_names_are_refused_before_store_creation() {
        let oversized_store = TestStore::new("oversized-name");
        let oversized =
            Supervisor::bootstrap(config(&oversized_store.path, &"a".repeat(121))).unwrap_err();
        assert!(matches!(oversized, SupervisorError::WorkspaceNameTooLong));
        assert!(!oversized_store.path.exists());

        let control_store = TestStore::new("control-name");
        let control =
            Supervisor::bootstrap(config(&control_store.path, "Line one\nLine two")).unwrap_err();
        assert!(matches!(
            control,
            SupervisorError::WorkspaceNameContainsControl
        ));
        assert!(!control_store.path.exists());

        let leading_control_store = TestStore::new("leading-control-name");
        let leading_control =
            Supervisor::bootstrap(config(&leading_control_store.path, "\nPersonal")).unwrap_err();
        assert!(matches!(
            leading_control,
            SupervisorError::WorkspaceNameContainsControl
        ));
        assert!(!leading_control_store.path.exists());
    }

    #[test]
    fn injection_shaped_workspace_name_is_stored_as_data() {
        let store = TestStore::new("parameterized-name");
        let name = "Personal'); DROP TABLE events; --";

        let first = Supervisor::bootstrap(config(&store.path, name)).unwrap();
        let second = Supervisor::bootstrap(config(&store.path, "Ignored")).unwrap();

        assert_eq!(first.workspace.name, name);
        assert_eq!(second.workspace.id, first.workspace.id);
        assert_eq!(second.diagnostics.persisted_event_count, 1);
    }

    #[test]
    fn unsupported_schema_version_fails_closed() {
        let store = TestStore::new("future-schema");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "UPDATE schema_meta SET version = 99 WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap_err();

        assert!(matches!(
            error,
            SupervisorError::UnsupportedSchemaVersion(99)
        ));
    }

    #[test]
    fn same_version_malformed_schema_is_refused_without_mutation() {
        let store = TestStore::new("malformed-schema");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE schema_meta (
                    singleton INTEGER PRIMARY KEY,
                    version INTEGER NOT NULL
                );
                INSERT INTO schema_meta (singleton, version) VALUES (1, 1);
                CREATE TABLE projects (unexpected_column TEXT);
                ",
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(result.is_err(), "a malformed version-1 store was accepted");
        let connection = Connection::open(&store.path).unwrap();
        let mut statement = connection
            .prepare(
                "
                SELECT name
                FROM sqlite_schema
                WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name
                ",
            )
            .unwrap();
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(tables, ["projects", "schema_meta"]);
    }

    #[test]
    fn unexpected_view_is_refused_without_creating_application_tables() {
        let store = TestStore::new("unexpected-view");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute("CREATE VIEW unexpected AS SELECT 1 AS value", [])
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a view-only store was accepted and mutated"
        );
        let connection = Connection::open(&store.path).unwrap();
        let mut statement = connection
            .prepare(
                "
                SELECT type, name
                FROM sqlite_schema
                WHERE name NOT LIKE 'sqlite_%'
                ORDER BY type, name
                ",
            )
            .unwrap();
        let objects = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(objects, [("view".to_owned(), "unexpected".to_owned())]);
    }

    #[test]
    fn sqlite_internal_prefix_filter_treats_underscore_literally() {
        let store = TestStore::new("sqlite-prefix-wildcard");
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute("CREATE VIEW sqlitexunexpected AS SELECT 1 AS value", [])
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a user view matching an unescaped SQLite prefix pattern was accepted"
        );
        let connection = Connection::open(&store.path).unwrap();
        let object_count: i64 = connection
            .query_row(
                "
                SELECT COUNT(*)
                FROM sqlite_schema
                WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
                ",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(object_count, 1, "bootstrap mutated the rejected store");
    }

    #[test]
    fn foreign_key_corruption_is_refused_instead_of_reported_ready() {
        let store = TestStore::new("foreign-key-corruption");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute(
                "
                INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                VALUES (?1, 'workspace.created', ?2, 0)
                ",
                rusqlite::params![
                    Uuid::new_v4().to_string(),
                    serde_json::json!({
                        "workspace_id": Uuid::new_v4(),
                        "name": "Orphan"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "a store with a foreign-key violation was reported ready"
        );
        assert!(matches!(
            result,
            Err(SupervisorError::ForeignKeyViolations(1))
        ));
    }

    #[test]
    fn store_rejects_an_event_without_workspace_scope() {
        let store = TestStore::new("unscoped-event");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();

        let result = connection.execute(
            "
            INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
            VALUES (NULL, 'workspace.created', '{}', 0)
            ",
            [],
        );

        assert!(result.is_err(), "an unscoped Event was persisted");
    }

    #[test]
    fn concurrent_bootstrap_creates_exactly_one_workspace_and_event() {
        let store = TestStore::new("concurrent");
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let path = store.path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    Supervisor::bootstrap(config(&path, "Personal")).unwrap()
                })
            })
            .collect::<Vec<_>>();
        let reports = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            reports
                .iter()
                .filter(|report| report.workspace.created)
                .count(),
            1
        );
        assert_eq!(reports[0].workspace.id, reports[1].workspace.id);
        assert!(
            reports
                .iter()
                .all(|report| report.diagnostics.workspace_count == 1)
        );
        assert!(
            reports
                .iter()
                .all(|report| report.diagnostics.persisted_event_count == 1)
        );
    }

    #[test]
    fn sql_schema_independently_enforces_workspace_name_length() {
        let store = TestStore::new("sql-name-limit");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();

        let result = connection.execute(
            "
            INSERT INTO workspaces (id, name, created_at_ms)
            VALUES (?1, ?2, 0)
            ",
            rusqlite::params![Uuid::new_v4().to_string(), "a".repeat(121)],
        );

        assert!(
            result.is_err(),
            "SQLite accepted an oversized Workspace name"
        );
    }

    #[test]
    fn event_payload_workspace_must_match_its_persisted_scope() {
        let store = TestStore::new("event-scope-mismatch");
        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "
                INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                VALUES (?1, 'workspace.created', ?2, 0)
                ",
                rusqlite::params![
                    report.workspace.id.to_string(),
                    serde_json::json!({
                        "workspace_id": Uuid::new_v4(),
                        "name": "Wrong scope"
                    })
                    .to_string()
                ],
            )
            .unwrap();
        drop(connection);

        let error = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap_err();

        assert!(matches!(
            error,
            SupervisorError::EventPayloadScopeMismatch(2)
        ));
    }

    #[test]
    fn event_payload_rejects_unknown_fields() {
        let store = TestStore::new("event-unknown-field");
        let report = Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let connection = Connection::open(&store.path).unwrap();
        connection
            .execute(
                "
                UPDATE events
                SET payload_json = ?1
                WHERE sequence = 1
                ",
                [serde_json::json!({
                    "workspace_id": report.workspace.id,
                    "name": report.workspace.name,
                    "unexpected": true
                })
                .to_string()],
            )
            .unwrap();
        drop(connection);

        let result = Supervisor::bootstrap(config(&store.path, "Personal"));

        assert!(
            result.is_err(),
            "an event payload with an undeclared field was accepted"
        );
    }

    /// Migration 0022 rebuilds `rulings` and `ruling_supersessions` to widen the canonicalizer
    /// CHECK. gpt1 removed the `INSERT INTO rulings ... SELECT ... FROM rulings_v19` copy and every
    /// integration test stayed green, because they all record their Rulings *after* the migration
    /// has already run: nothing proved that a store which already held v1 Rulings keeps them.
    ///
    /// This applies the ladder to version 20, plants a v1 Ruling, its supersession link and its
    /// Events with foreign keys off -- the migration's copy is what is under test, not the parent
    /// chain -- then applies 0022 and requires every byte back.
    #[test]
    fn migration_0022_carries_existing_v1_rulings_across_the_table_rebuild() {
        let connection = Connection::open_in_memory().unwrap();
        // Foreign keys off on purpose: the parent chain a real Ruling hangs from (Proposal,
        // Mission, Project, Charter) is not what this test is about, and building it would make
        // the test a fixture exercise rather than a migration one.
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        for migration in [
            super::SCHEMA_V1,
            super::MIGRATION_V2,
            super::MIGRATION_V3,
            super::MIGRATION_V4,
            super::MIGRATION_V5,
            super::MIGRATION_V6,
            super::MIGRATION_V7,
            super::MIGRATION_V8,
            super::MIGRATION_V9,
            super::MIGRATION_V10,
            super::MIGRATION_V11,
            super::MIGRATION_V12,
            super::MIGRATION_V13,
            super::MIGRATION_V14,
            super::MIGRATION_V15,
            super::MIGRATION_V16_ONLY,
            super::MIGRATION_V17,
            super::MIGRATION_V18,
            super::MIGRATION_V19,
            super::MIGRATION_V20,
        ] {
            connection.execute_batch(migration).unwrap();
        }

        let workspace = Uuid::new_v4().to_string();
        let proposal = "a".repeat(64);
        let first = "b".repeat(64);
        let second = "c".repeat(64);
        let bytes: Vec<u8> = (0..40_u8).collect();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 1)",
                rusqlite::params![workspace],
            )
            .unwrap();
        for digest in [&first, &second] {
            connection
                .execute(
                    "INSERT INTO rulings (
                         workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                         canonical_bytes, proposal_digest, proposal_digest_algorithm,
                         proposal_digest_canonicalizer, charter_revision, project_policy_revision,
                         repository_revision, pinned_base_revision, decision, reason,
                         decider_basis, ruled_at_ms
                     ) VALUES (?1, 'sha256', 'beacon.ruling.v1', ?2, ?3, ?4, 'sha256',
                               'beacon.proposal.v1', 1, 1, 1, ?5, 'approve', 'because', 'owner', 7)",
                    rusqlite::params![workspace, digest, bytes, proposal, "d".repeat(40)],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO ruling_supersessions (
                     workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
                 ) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![workspace, proposal, second, first],
            )
            .unwrap();

        connection.execute_batch(super::MIGRATION_V22).unwrap();

        // Both Rulings, their exact bytes, and their canonicalizer column.
        let mut statement = connection
            .prepare(
                "SELECT digest_value, canonical_bytes, digest_canonicalizer
                 FROM rulings ORDER BY digest_value",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (first.clone(), bytes.clone(), "beacon.ruling.v1".to_owned()),
                (second.clone(), bytes.clone(), "beacon.ruling.v1".to_owned()),
            ],
            "migration 0022 did not carry existing v1 Rulings across the rebuild"
        );

        // And the supersession link, whose foreign keys now point at the rebuilt table.
        let link: (String, String) = connection
            .query_row(
                "SELECT ruling_digest, supersedes_ruling_digest FROM ruling_supersessions",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(link, (second, first));

        // The widened vocabulary is what the rebuild was for. Inserted rather than updated,
        // because the immutability trigger the rebuild restored refuses an UPDATE -- which is
        // itself worth asserting here, since the trigger is one of the objects the rebuild had to
        // put back.
        connection
            .execute(
                "INSERT INTO rulings (
                     workspace_id, digest_algorithm, digest_canonicalizer, digest_value,
                     canonical_bytes, proposal_digest, proposal_digest_algorithm,
                     proposal_digest_canonicalizer, charter_revision, project_policy_revision,
                     repository_revision, pinned_base_revision, decision, reason,
                     decider_basis, ruled_at_ms
                 ) VALUES (?1, 'sha256', 'beacon.ruling.v2', ?2, ?3, ?4, 'sha256',
                           'beacon.proposal.v1', 1, 1, 1, ?5, 'approve', 'because', 'owner', 8)",
                rusqlite::params![workspace, "e".repeat(64), bytes, proposal, "d".repeat(40)],
            )
            .expect("the rebuilt table accepts the second grammar");
        assert!(
            connection
                .execute(
                    "UPDATE rulings SET reason = 'edited' WHERE digest_value = ?1",
                    rusqlite::params![rows[0].0],
                )
                .is_err(),
            "the rebuild did not restore the immutability trigger"
        );
    }
}
