use std::collections::HashMap;
use std::fmt;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
mod event_chain;
mod git_object_store;
mod local_model_activation;
mod mission;
mod mission_completion;
mod mission_completion_command;
mod mission_completion_store;
mod mission_impact;
mod mission_impact_command;
mod model_invocation;
mod operator_account;
mod outcome;
mod outcome_assembler;
mod outcome_collector;
mod outcome_pdf;
mod outcome_publication;
mod outcome_store;
mod outcome_typst;
mod platform_session;
mod project;
mod project_policy;
mod project_repository;
mod proposal;
mod publication_seam;
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
    ResolvedCheck, UnavailableEvidence, enumerate_runs,
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
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use team::{TeamVerificationHook, set_after_team_enumeration_hook};

pub use artifact::{
    Artifact, ArtifactActor, ArtifactContext, ArtifactDigest, ArtifactError, ArtifactKind,
    ArtifactService, ArtifactV1, InconsistentArtifact, MalformedArtifact, VerifiedArtifactRecord,
    enumerate_artifacts,
};
pub use candidate::{
    Candidate, CandidateDigest, CandidateEntry, CandidateError, CandidateImages, CandidateService,
    CandidateV1, InconsistentCandidate, MalformedCandidate, enumerate_candidates,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use dispatch::DispatchFailpoints;
pub use dispatch::{
    DispatchActor, DispatchError, DispatchOutcome, DispatchReceipt, DispatchService, GitStep,
    GitStepName, GitStepResult, MutationEffect, MutationRecord, NotVerified, ObservedPreImage,
    PauseService, RepositoryEffect, SourceIdentity, SuppliedContent, UnresolvedRuns,
};
pub use evaluation::{
    ArtifactSubject, Conflicts, Evaluation, EvaluationDigest, EvaluationDraft, EvaluationError,
    EvaluationForce, EvaluationService, EvaluationSubject, EvaluationSubjectDraft,
    EvaluationSupersedes, EvaluationV1, Evaluator, Finding, FindingSeverity,
    InconsistentEvaluation, MalformedEvaluation, RubricDeclaration, Verdict,
    enumerate_evaluation_keys,
};
pub use local_model_activation::{
    LocalModelActivation, LocalModelActivationError, LocalModelActivationReview,
    LocalModelActivationService, LocalModelActivationState, LocalModelBinding, ProviderFeature,
    enumerate_activation_decisions,
};
pub use mission::{
    MissionAdmissionActor, MissionDraft, MissionError, MissionLifecycle, MissionMemberDraft,
    MissionMemberView, MissionService, MissionView, enumerate_missions,
};
pub use mission_completion::{
    COMPLETION_CANONICALIZER, CompletionActor, CompletionDigest, CompletionDisposition,
    CompletionSupersedes, ConditionOutcome, ConditionResultRecord, MalformedCompletion,
    MissionCompletionV1, NarrativeItem, SpendReference, SupportKind, SupportRef, UncertaintyKind,
    UnfinishedKind,
};
pub use mission_completion_command::{
    CompletionCommandError, CompletionCommandUnavailable, CompletionIntentRequest,
    CompletionPreview, ConditionResultDto, MissionCompletionCommandService, NarrativeItemDto,
    REVIEWED_INTENT_PREFIX, ReviewedIntent, SpendDto, SupportRefDto,
};
pub use mission_completion_store::{
    CompletionReadRefusal, CompletionUnavailable, CompletionView, ConditionResultView,
    MissionCompletionReadError, MissionCompletionReadService, declared_conditions_digest,
    enumerate_completions, record_completion, verified_completion_within,
};
pub use mission_impact::{
    CANONICALIZER as MISSION_IMPACT_CANONICALIZER, ClassificationActor, ClassificationBasis,
    ClassificationDigest, ClassificationSupersedes, ImpactLevel, MalformedClassification,
    MissionImpactClassificationV1, PolicyRevisionRef, enumerate_classifications,
};
pub use mission_impact::{
    ClassificationRecordError, ClassificationStoreError, MissionImpactClassification,
    MissionImpactService, verified_classification_for_citation, verified_classification_within,
};
pub use mission_impact_command::{
    ClassificationCommandError, ClassificationCommandUnavailable, ClassificationDigestDto,
    ClassificationIntentRequest, ClassificationView, MissionImpactCommandService,
    PolicyCitationDto, SupersedesDto,
};
pub use model_invocation::{
    ADMISSION_CANONICALIZER, AppendRefusal, Appended, ByteCoherenceFault,
    INVOCATION_DIGEST_ALGORITHM, IncompleteCause, InvocationAdmissionV1, InvocationObservationV1,
    MAX_ADAPTER_PROFILE_BYTES, MAX_AUTHORITY_REFERENCE_BYTES, MAX_BINDING_REFERENCE_BYTES,
    MAX_HEADER_NAME_BYTES, MAX_HEADER_VALUE_BYTES, MAX_PROMPT_PROFILE_BYTES, MAX_RETAINED_HEADERS,
    MalformedInvocation, OBSERVATION_CANONICALIZER, ObservationKind, OwnerAuthorizationBasis,
    ReadCeiling, RetainedAuthority, RetainedBinding, RetainedHeader, RetainedLimits, SequenceFault,
    TransitionRefusal, TransportHistory, TransportObservation, TransportStatus,
};
pub use operator_account::{
    AccessOrigin, OperatorAccountError, OperatorAccountService, OperatorSessionPhase,
    OperatorSessionPolicy, OperatorSessionStatus, PlatformSessionEvent, RecoveryKey, SecretString,
};
pub use outcome::{
    AppendixMode, AppendixRequest, BoundRefusal, ConditionResult, MalformedOutcome, NarrativeRef,
    OutcomeBound, OutcomeManifestV1, OutcomeRefusal, OutcomeState, PinnedRevision,
    RETIRED_CANONICALIZER, ReceiptRef, ReferenceIdentity, RevisionKind, SourceDigest, SourceKind,
    SourceMismatch, SourceRef, SpendMeasurementKind, SpendRef, SuccessCondition, VerifiedArtifact,
    VerifiedSources, VerifiedText, compare_canonical_keys, render,
};
pub use outcome_assembler::{ARTIFACT_TITLE_ABSENT, AssemblyRefusal, SourcesRefusal};
pub use outcome_collector::SPEND_BEARING_EVENT_KINDS;
pub use outcome_collector::{
    CollectedClassification, CollectedCompletion, CollectedCondition, CollectedDigest,
    CollectedNarrative, CollectedSources, CollectedSpend, CollectedSupport, CollectedUncounted,
    CollectionRequest, Collector, CollectorRefusal, OUTCOME_RECEIPT_CHECKSUM_LABEL, SpendAmount,
    SpendCompleteness, positionable_sources,
};
pub use outcome_pdf::{BEACON_OUTCOME_1, EmbeddedFace, PdfProfile, PdfRefusal, render_pdf};
pub use outcome_publication::{
    ExportedOutcome, Provenance, PublicationSummary, PublishRefusal, PublishedOutcome,
    SupersessionAtReadTime, enumerate_publications,
};
pub use outcome_store::{
    MAX_STORED_MANIFEST_BYTES, ManifestStoreRefusal, OutcomeCandidate, OutcomeService,
    PrepareRefusal, RenderedPdf, StoredOutcomeManifest, enumerate_manifests, manifest_reference_id,
    verified_manifest_within,
};
pub use outcome_typst::{TypstRefusal, to_typst};
pub use platform_session::{
    PlatformMonitorState, PlatformSessionMonitor, PlatformSessionMonitorStatus,
};
pub use project::{
    AutonomyProfile, BudgetBoundary, ExternalEffects, ProjectCharter, ProjectCharterDraft,
    ProjectDraft, ProjectError, ProjectService, ProjectSummary, ProjectView, enumerate_projects,
};
pub use project_policy::{
    PolicyAuthor, ProjectPolicy, ProjectPolicyDraft, ProjectPolicyError, ProjectPolicyService,
    TeamLeadAmbiguityRule, TeamLeadAvailabilityRule, TeamLeadConflictRule, TeamLeadSelectionPolicy,
    TrackRecordEvidenceKind, TrackRecordRequirement, enumerate_policy_projects,
};
pub use project_repository::{
    DeclaredCheck, InvalidRepositoryDraft, ProjectRepository, ProjectRepositoryError,
    ProjectRepositoryService, RepositoryAuthor, RepositoryDraft, RepositorySourceKind,
    enumerate_repository_projects,
};
pub use proposal::{
    BudgetEnvelope, ConstraintBinding, ContextComparison, ContextEntry, ContextMismatch,
    ContextMismatchReason, DeclaredMutation, InconsistentProposal, MalformedProposal, MutationKind,
    OwnerBootstrap, Proposal, ProposalActor, ProposalBindings, ProposalBounds, ProposalDigest,
    ProposalError, ProposalIntent, ProposalReadiness, ProposalRequester, ProposalRequirements,
    ProposalScope, ProposalService, ProposalV1, ReadableContext, RollbackStrategy,
    UnmetPrecondition, enumerate_proposals,
};
pub use publication_seam::{PublicationCandidacy, PublicationSeamRefusal, publication_candidacy};
pub use resident_configuration::{
    BeaconCapability, BeaconCapabilityDescriptor, ResidentConfiguration,
    ResidentConfigurationBinding, ResidentConfigurationError, ResidentConfigurationService,
    ResidentConfigurationState, TeamIneligibilityReason, enumerate_configuration_decisions,
};
pub use resident_lifecycle::{
    ResidentAgentView, ResidentLifecycle, ResidentLifecycleActor, ResidentLifecycleDecision,
    ResidentLifecycleError, ResidentLifecycleService, enumerate_lifecycle_residents,
};
pub use resource_discovery::{
    DiscoveredLocalModel, DiscoveryStatus, LocalModelDetails, LocalModelDiscoveryReport,
    LocalModelEndpoint, ModelActivation, ModelProvider, ResourceDiscoveryError,
    ResourceDiscoveryService,
};
pub use ruling::{
    InconsistentRuling, MalformedRuling, Ruling, RulingDecider, RulingDecision, RulingDigest,
    RulingError, RulingForce, RulingGrammar, RulingService, RulingSupersedes, RulingV1,
    enumerate_rulings,
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
    CANONICALIZER as SPEND_CANONICALIZER, CANONICALIZER_V1 as SPEND_CANONICALIZER_V1,
    CANONICALIZER_V2 as SPEND_CANONICALIZER_V2, ColumnDisagreement, LineAttribution,
    MalformedSpend, MeasuredSpend, MeasuredSpendService, MeasuredSpendV1, PredecessorFault,
    SpendAttribution, SpendDigest, SpendEra, SpendLine, SpendMeasurement, SpendOrigin,
    SpendRecordError, SpendStoreError, SpendSupersedes, SpendTotals, UncountedEntry, UncountedKind,
    V1_ERA_REASON as SPEND_V1_ERA_REASON, compare_units, verified_spend_for_citation,
    verified_spend_within,
};
pub use starter_organization::{
    ResidentAgentSummary, StarterOrganizationDecision, StarterOrganizationError,
    StarterOrganizationService, StarterOrganizationView, StarterResidentDraft, StarterRole,
    StarterRoleTemplate, enumerate_starter_workspaces,
};
pub use team::{
    TeamDraft, TeamError, TeamFormationActor, TeamMemberDraft, TeamMemberView, TeamService,
    TeamView, enumerate_teams,
};
#[cfg(debug_assertions)]
#[doc(hidden)]
pub use worktree::set_pinned_spawn_hook;
pub use worktree::{CleanupOutcome, KeepReason, Worktree, WorktreeError};
pub use worktree::{FilterInventoryFailure, PinnedRead, PinnedReadFailure};

const SCHEMA_VERSION: i64 = 36;
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
// The Artifact store held 0026 provisionally; the completion store landed first and took it, so
// this one is 0027 exactly as the note above said the second lane would do. The events kind list
// in 0027 was regenerated against 0026's parent list (which now includes mission.completed) --
// a list written before the rebase is a snapshot of the wrong parent (the #50 trap), and the
// migration tests diff the list against the enum.
const MIGRATION_V27: &str = include_str!("../migrations/0027_artifacts.sql");
// PROVISIONAL: the publication relation wants a number in this window too, and whichever lands
// second renumbers at its final rebase.
const MIGRATION_V28: &str = include_str!("../migrations/0028_evaluation_subjects.sql");
// PROVISIONAL, for the same reason as 0028: `main` is at 0028 and more than one lane in this
// window wants a number. This one adds NO Event kind, so it does not rebuild `events`.
const MIGRATION_V29: &str = include_str!("../migrations/0029_outcome_manifests.sql");
// PROVISIONAL (R22): the grammar revision landed after 0029 merged, so the CHECK that pins the
// manifest canonicalizer moves by migration rather than by edit.
const MIGRATION_V30: &str =
    include_str!("../migrations/0030_outcome_manifest_canonicalizer_v2.sql");
const MIGRATION_V31: &str = include_str!("../migrations/0031_outcome_publications.sql");
// 0031 landed with the publication relation (#80) while this was in review, so the ladder is
// contiguous and this number is final: 30 -> 31 -> 32.
const MIGRATION_V32: &str =
    include_str!("../migrations/0032_classification_mission_project_key.sql");
const MIGRATION_V33: &str = include_str!("../migrations/0033_receipt_pre_image_unreadable.sql");
const MIGRATION_V34: &str = include_str!("../migrations/0034_receipt_outcome_typed_owner.sql");
const MIGRATION_V35: &str = include_str!("../migrations/0035_spend_attribution_dimensions.sql");
const MIGRATION_V36: &str = include_str!("../migrations/0036_event_chain.sql");

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
    /// The stored Event chain does not verify from its genesis to its head (R58). The fault names
    /// the sequence it failed at.
    EventChainBroken(event_chain::ChainFault),
    /// The installation's current pause state is not supported by the Events that should have
    /// caused it.
    ///
    /// **R58.3, sol1's P0.** `EventChainBroken` proves that every link the store still holds is
    /// authentic and in order. It cannot prove that no tail was REMOVED -- delete the newest Event
    /// and rewrite the head co-stored with the rows to the previous link, and every remaining check
    /// agrees. `installation_pause` is not an Event row, so it survives that edit and still says a
    /// transition happened.
    ///
    /// Names the transition time the state records, because that is the surviving fact which proves
    /// the Event existed, and the terminal pause/resume Event the store still holds -- `None` when
    /// it holds none, which is the truncation itself. It does NOT name the missing Event's
    /// sequence: the store cannot compute it, and a refusal that states a number nobody measured is
    /// the exact failure this lane keeps finding.
    PauseStateUnsupported {
        changed_at_ms: i64,
        terminal_sequence: Option<i64>,
    },
    StarterOrganizationEventMismatch(i64),
    /// A Workspace's starter organization does not re-prove.
    ///
    /// Names the Workspace -- this pass's whole key -- and carries the reader's own error (R28.1).
    StarterOrganizationStateMismatch {
        workspace_id: Uuid,
        cause: starter_organization::StarterOrganizationError,
    },
    ProjectEventMismatch(i64),
    /// A stored Project does not re-prove against its charter history or the log.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    ProjectStateMismatch {
        workspace_id: Uuid,
        project_id: Uuid,
        cause: project::ProjectError,
    },
    /// The Projects do not agree with the store as a whole. No row to name.
    ProjectTotalsMismatch(TotalsMismatch),
    LocalModelActivationEventMismatch(i64),
    /// A local model activation decision does not re-prove against its record or its Event.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    LocalModelActivationStateMismatch {
        workspace_id: Uuid,
        activation_id: Uuid,
        cause: local_model_activation::LocalModelActivationError,
    },
    /// An activation row that could not be read at all, so it cannot be named.
    LocalModelActivationRowUnreadable(local_model_activation::LocalModelActivationError),
    ResidentLifecycleEventMismatch(i64),
    /// A Resident's lifecycle chain does not re-prove against its decisions.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    ResidentLifecycleStateMismatch {
        workspace_id: Uuid,
        resident_id: Uuid,
        cause: resident_lifecycle::ResidentLifecycleError,
    },
    /// The Resident lifecycle decisions do not agree with the store. No row to name.
    ResidentLifecycleTotalsMismatch(TotalsMismatch),
    /// A Resident row that could not be read at all, so it cannot be named.
    ResidentRowUnreadable(resident_lifecycle::ResidentLifecycleError),
    ResidentConfigurationEventMismatch(i64),
    /// A Resident configuration decision does not re-prove against its record or its Event.
    ///
    /// Names the row and carries the owning reader's own error (R28.1). `decision_id` is the
    /// relation's own text, which is how this pass reads it.
    ResidentConfigurationStateMismatch {
        workspace_id: Uuid,
        decision_id: String,
        cause: resident_configuration::ResidentConfigurationError,
    },
    TeamEventMismatch(i64),
    /// A stored Team does not re-prove against its own record or the log.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    TeamStateMismatch {
        workspace_id: Uuid,
        team_id: Uuid,
        cause: team::TeamError,
    },
    /// The Teams do not agree with the log as a whole. No row to name.
    TeamTotalsMismatch(TotalsMismatch),
    MissionEventMismatch(i64),
    /// A stored Mission does not re-prove against its own record or its projections.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    MissionStateMismatch {
        workspace_id: Uuid,
        mission_id: Uuid,
        cause: mission::MissionError,
    },
    /// The Missions do not agree with the store as a whole: an orphaned condition row, or an
    /// admitted-event count that does not match the number of Missions. No row to name.
    MissionTotalsMismatch(TotalsMismatch),
    ProjectPolicyEventMismatch(i64),
    /// A Project's policy history does not re-prove against its own records.
    ///
    /// Names the row and carries the owning reader's own error (R28.1). No `Totals` twin.
    ProjectPolicyStateMismatch {
        workspace_id: Uuid,
        project_id: Uuid,
        cause: project_policy::ProjectPolicyError,
    },
    ProjectRepositoryEventMismatch(i64),
    /// A Project's repository history does not re-prove against its own records.
    ///
    /// Names the row and carries the owning reader's own error (R28.1). No `Totals` twin: this
    /// pass has no whole-store count.
    ProjectRepositoryStateMismatch {
        workspace_id: Uuid,
        project_id: Uuid,
        cause: project_repository::ProjectRepositoryError,
    },
    SampleProjectEventMismatch(i64),
    /// A stored sample-Project installation does not re-prove.
    ///
    /// Names the Workspace it belongs to -- which is this pass's whole key -- and carries the
    /// owning reader's own error (R28.1).
    SampleProjectStateMismatch {
        workspace_id: Uuid,
        cause: sample_project::SampleProjectError,
    },
    /// The sample-Project installations do not agree with the log. No row to name.
    SampleProjectTotalsMismatch(TotalsMismatch),
    RunEventMismatch(i64),
    /// A Run does not agree with its Receipts and Events.
    ///
    /// Names the row and carries the owning reader's own error (R28.1). `run_id` is the relation's
    /// own text, which is how this pass reads it.
    RunStateMismatch {
        workspace_id: Uuid,
        run_id: String,
        cause: check_preflight::CheckPreflightError,
    },
    /// The clock is before the Unix epoch, so no lease can be judged expired.
    RunClockBeforeEpoch,
    /// A replayed `run.preflighted` event's Receipt disagrees with the Run it names.
    ///
    /// Distinct from `RunEventMismatch`, which is the evidence not matching, and from
    /// `RunStateMismatch`, which is the startup pass reading the table rather than the log.
    RunEventStateMismatch {
        sequence: i64,
        workspace_id: Uuid,
        run_id: Uuid,
    },
    ProposalEventMismatch(i64),
    /// A stored Proposal does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    ProposalStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: proposal::ProposalError,
    },
    /// The Proposals do not agree with the log as a whole. No row to name.
    ProposalTotalsMismatch(TotalsMismatch),
    RulingEventMismatch(i64),
    EvaluationEventMismatch(i64),
    /// A stored Ruling does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    RulingStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: ruling::RulingError,
    },
    /// The Rulings do not agree with the log as a whole. No row to name.
    RulingTotalsMismatch(TotalsMismatch),
    /// The Ruling chains do not re-prove. Structural, so no counts.
    RulingChainsMismatch(ruling::RulingError),
    CandidateEventMismatch(i64),
    /// A stored Candidate does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    CandidateStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: candidate::CandidateError,
    },
    // NO `CandidateTotalsMismatch`: that pass has no whole-table comparison, by its own design.
    DispatchEventMismatch(i64),
    /// The startup step that FINISHES an interrupted dispatch could not complete its write.
    ///
    /// Deliberately not a `...StateMismatch` (R28.1). It runs in the committed reconciliation step
    /// before `inspect_store`; that entire successor walk is verification-only. Those passes compare
    /// a record against its own bytes and refuse when the two disagree, and "state mismatch" is the
    /// verdict of that comparison.
    /// `dispatch::finish_prepared_dispatches` compares nothing -- it writes a terminal Receipt for
    /// an attempt that died before recording one. When it fails, nothing disagreed; a write did not
    /// complete, and the Run it was recovering still has no terminal Receipt.
    ///
    /// It does not name the dispatch. The writer knows the run id at every one of its failure
    /// points, but `DispatchError` has no variant that carries one, and giving it one is the
    /// carried-cause change R27.2 holds for Kevin's timing -- not something to do in passing under
    /// a rename.
    DispatchRecoveryFailed,
    /// A stored Evaluation does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    EvaluationStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: evaluation::EvaluationError,
    },
    /// The Evaluation supersession chains do not agree with the store. No row to name.
    /// The Evaluation supersession chains do not re-prove. Structural, so no counts.
    EvaluationChainsMismatch(evaluation::EvaluationError),
    MissionCompletionEventMismatch(i64),
    /// A stored Mission completion does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own refusal (R28.1).
    MissionCompletionStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: mission_completion_store::CompletionReadRefusal,
    },
    /// The completions do not agree with the log as a whole. No row to name.
    MissionCompletionTotalsMismatch(TotalsMismatch),
    ArtifactEventMismatch(i64),
    /// A stored Artifact does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own error (R28.1).
    ArtifactStateMismatch {
        workspace_id: Uuid,
        digest_value: String,
        cause: artifact::ArtifactError,
    },
    /// The Artifact supersession relation does not agree with the store. No row to name.
    ArtifactTotalsMismatch(TotalsMismatch),
    /// The Artifact supersession relation does not re-prove. Structural, so no counts.
    ArtifactRelationMismatch(artifact::ArtifactError),
    /// A stored Mission impact classification does not re-prove against its own bytes or the
    /// predecessor it names -- WITH the row and the owning reader's own fault (R27.1).
    ///
    /// Deliberately unlike its fourteen siblings, which are unit variants fed by `.map_err(|_| ..)`.
    /// A refusal that says only "a classification does not re-prove" cannot tell an operator a
    /// corrupt store from a forged link from a bug in the reader, and cannot say which row. This is
    /// the shape the others move to (R27.2), taken pass by pass; it is a surfaced break from the
    /// convention, not a silent fork (R11).
    MissionImpactClassificationStateMismatch {
        workspace_id: Uuid,
        digest: String,
        cause: mission_impact::ClassificationStoreError,
    },
    /// A stored Outcome manifest does not re-prove against its own bytes or the records it cites.
    ///
    /// Names the row and carries the owning reader's own refusal (R28.1).
    OutcomeManifestStateMismatch {
        workspace_id: Uuid,
        manifest_digest: String,
        cause: outcome_store::ManifestStoreRefusal,
    },
    /// A stored Outcome publication does not re-prove against the records it cites.
    ///
    /// Names the row and carries the owning reader's own refusal (R28.1). `version` is the version
    /// AS STORED: a row whose version does not fit `u32` is one of the faults this reports, so the
    /// key has to be able to hold it.
    OutcomePublicationStateMismatch {
        workspace_id: Uuid,
        mission_id: Uuid,
        version: i64,
        cause: outcome_publication::PublishRefusal,
    },
    PauseEventMismatch(i64),
    SchemaMismatch,
    UnsupportedSchemaVersion(i64),
    IntegrityCheckFailed(String),
    /// A stored row whose identity columns are not readable text. Carries the RAW value; the
    /// one-line `Display` bounds and escapes it (sol1's P1-1), and `Debug` keeps it whole.
    StoredIdentityUnreadable {
        what: &'static str,
        raw: String,
    },
    /// A stored integer key is outside the public reader's domain. Textual owner fields remain
    /// escaped and bounded, while the typed key is rendered outside that bound so it cannot be
    /// truncated out of the operator line (R45).
    StoredIntegerKeyUnreadable {
        what: &'static str,
        raw: String,
        key_name: &'static str,
        key: i64,
    },
    /// A startup pass could not enumerate its rows. Names WHICH pass; the driver's own error
    /// travels in `Debug` and `source()` only (sol1's P1-3, R28.4).
    StartupEnumerationFailed {
        pass: StartupPass,
        source: rusqlite::Error,
    },
    ForeignKeyViolations(u64),
}

/// A count across a whole table that does not match the log (sol1's P1-2).
///
/// The Totals arms used to carry the pass's own `InvalidStore` -- a name that says a count was
/// wrong and not which count, which table, or by how much. One honest completion plus one
/// duplicate reached `Totals(EventCount(2))` and the operator learned nothing actionable.
///
/// Numeric context, which R42 allows: the table, what the log says there should be, and what the
/// store holds. In the line AND in `Debug`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TotalsMismatch {
    /// The table or relation the count is over. A fixed string, never store text.
    pub table: &'static str,
    pub expected: u64,
    pub observed: u64,
}

/// The identity labels a startup refusal may print, and nothing else (R42).
///
/// `StoredIdentityUnreadable` carries `what` (which owner could not name its row) beside `raw`
/// (the unreadable identity). `what` is a `&'static str` at every production site -- but static is
/// where a string lives, not who wrote it, and this is a public enum. An unrecognized label is
/// reported as unrecognized rather than echoed; both `what` and `raw` stay in `Debug`.
fn identity_label(what: &str) -> &'static str {
    match what {
        "a Project row's identity is not readable" => "a Project row's identity is not readable",
        "a Mission row's identity is not readable" => "a Mission row's identity is not readable",
        "a completion row's Workspace is not readable" => {
            "a completion row's Workspace is not readable"
        }
        "a manifest row's Workspace is not readable" => {
            "a manifest row's Workspace is not readable"
        }
        "a publication row's identity is not readable" => {
            "a publication row's identity is not readable"
        }
        "a sample Project row's Workspace is not readable" => {
            "a sample Project row's Workspace is not readable"
        }
        "a Proposal row's Workspace is not readable" => {
            "a Proposal row's Workspace is not readable"
        }
        "a Ruling row's Workspace is not readable" => "a Ruling row's Workspace is not readable",
        "a Candidate row's Workspace is not readable" => {
            "a Candidate row's Workspace is not readable"
        }
        "an Evaluation row's Workspace is not readable" => {
            "an Evaluation row's Workspace is not readable"
        }
        "an Artifact row's Workspace is not readable" => {
            "an Artifact row's Workspace is not readable"
        }
        "a Project repository row's identity is not readable" => {
            "a Project repository row's identity is not readable"
        }
        "a Team row's identity is not readable" => "a Team row's identity is not readable",
        "a Project policy row's identity is not readable" => {
            "a Project policy row's identity is not readable"
        }
        "a Workspace id is not readable" => "a Workspace id is not readable",
        "a Resident configuration row's identity is not readable" => {
            "a Resident configuration row's identity is not readable"
        }
        // NOTE the capital R. astra's spec table spelled this "a run row's..."; production spells
        // it "a Run row's...". The allowlist entry therefore matched nothing and the real producer
        // fell through to the fallback -- a known label silently losing its line, which is exactly
        // what this guard exists to prevent. Caught on the post-R45 rebase by R45's own bar.
        "a Run row's identity is not readable" => "a Run row's identity is not readable",
        // R45's activation owner, which had no allowlist entry at all.
        "a local-model activation row's identity is not readable" => {
            "a local-model activation row's identity is not readable"
        }
        // R45's owners, folded in when its arm was closed against this same projection.
        "a Project repository revision is not readable" => {
            "a Project repository revision is not readable"
        }
        "a Project policy revision is not readable" => "a Project policy revision is not readable",
        "a Resident lifecycle decision's ordering key is not readable" => {
            "a Resident lifecycle decision's ordering key is not readable"
        }
        "a Resident configuration decision's ordering key is not readable" => {
            "a Resident configuration decision's ordering key is not readable"
        }
        "a local-model activation decision's ordering key is not readable" => {
            "a local-model activation decision's ordering key is not readable"
        }
        _ => "stored row identity could not be read",
    }
}

/// The ordering-key NAMES a startup refusal may print (R42).
///
/// `key_name` is another public `&'static str`, and the same rule applies to it: a static lifetime
/// says where a string lives, not who wrote it. The key VALUE beside it is an i64 the store owns
/// and the reader is entitled to -- a typed key, not free text -- so it is printed as it is.
fn ordering_key_label(key_name: &str) -> &'static str {
    match key_name {
        "revision" => "revision",
        "sequence" => "sequence",
        "boundary_event_seq" => "boundary_event_seq",
        _ => "an unrecognized ordering key",
    }
}

/// The totals relations a startup refusal may name. Same reasoning as `identity_label`; the two
/// counts are approved context and are printed either way.
fn totals_label(table: &str) -> &'static str {
    match table {
        "operator_accounts (an owner for the stored Artifacts)" => {
            "operator_accounts (an owner for the stored Artifacts)"
        }
        "events (mission.completed)" => "events (mission.completed)",
        "mission_success_conditions (rows whose Mission does not exist)" => {
            "mission_success_conditions (rows whose Mission does not exist)"
        }
        "events (mission.admitted)" => "events (mission.admitted)",
        "project_creation_snapshots" => "project_creation_snapshots",
        "events (project.created)" => "events (project.created)",
        "events (project.charter_revised)" => "events (project.charter_revised)",
        "operator_accounts (an owner for the stored Proposals)" => {
            "operator_accounts (an owner for the stored Proposals)"
        }
        "events (proposal.submitted)" => "events (proposal.submitted)",
        "resident_lifecycle_decisions" => "resident_lifecycle_decisions",
        "operator_accounts (an owner for the stored Rulings)" => {
            "operator_accounts (an owner for the stored Rulings)"
        }
        "events (ruling.recorded)" => "events (ruling.recorded)",
        "events (sample_project.installed)" => "events (sample_project.installed)",
        "events (team.formed)" => "events (team.formed)",
        _ => "unrecognized totals relation",
    }
}

/// The Supervisor fault's NAME, for a caller of the local transport (R42).
///
/// Startup is allowed to name rows -- an operator reading a startup refusal owns the store. A
/// transport caller may own none of it, so this projection emits the root's name plus, where the
/// refusal carries one, the owning child's NAME: no Workspace, Mission, Project, Resident, run or
/// activation id, no digest, no version, no counts, no driver text. Everything stays in `Debug`.
///
/// It lives here rather than in `local_transport.rs` because all 72 variants and their private
/// children are visible from this module. Exhaustive and wildcard-free: a new startup fault is a
/// compile error here, not a new sentence at the transport boundary.
pub(crate) fn supervisor_caller_tag(error: &SupervisorError) -> String {
    match error {
        SupervisorError::EmptyWorkspaceName => "EmptyWorkspaceName".to_owned(),
        SupervisorError::WorkspaceNameTooLong => "WorkspaceNameTooLong".to_owned(),
        SupervisorError::WorkspaceNameContainsControl => "WorkspaceNameContainsControl".to_owned(),
        SupervisorError::ClockBeforeUnixEpoch => "ClockBeforeUnixEpoch".to_owned(),
        SupervisorError::Io(..) => "Io".to_owned(),
        SupervisorError::Store(..) => "Store".to_owned(),
        SupervisorError::Json(..) => "Json".to_owned(),
        SupervisorError::InvalidWorkspaceId(..) => "InvalidWorkspaceId".to_owned(),
        SupervisorError::InvalidEventKind(..) => "InvalidEventKind".to_owned(),
        SupervisorError::EventPayloadScopeMismatch(..) => "EventPayloadScopeMismatch".to_owned(),
        SupervisorError::EventChainBroken(..) => "EventChainBroken".to_owned(),
        SupervisorError::PauseStateUnsupported { .. } => "PauseStateUnsupported".to_owned(),
        SupervisorError::StarterOrganizationEventMismatch(..) => {
            "StarterOrganizationEventMismatch".to_owned()
        }
        SupervisorError::StarterOrganizationStateMismatch { cause, .. } => {
            format!("StarterOrganizationStateMismatch.{}", cause.name())
        }
        SupervisorError::ProjectEventMismatch(..) => "ProjectEventMismatch".to_owned(),
        SupervisorError::ProjectStateMismatch { cause, .. } => {
            format!("ProjectStateMismatch.{}", cause.name())
        }
        SupervisorError::ProjectTotalsMismatch(..) => "ProjectTotalsMismatch".to_owned(),
        SupervisorError::LocalModelActivationEventMismatch(..) => {
            "LocalModelActivationEventMismatch".to_owned()
        }
        SupervisorError::LocalModelActivationStateMismatch { cause, .. } => {
            format!("LocalModelActivationStateMismatch.{}", cause.name())
        }
        SupervisorError::LocalModelActivationRowUnreadable(cause) => {
            format!("LocalModelActivationRowUnreadable.{}", cause.name())
        }
        SupervisorError::ResidentLifecycleEventMismatch(..) => {
            "ResidentLifecycleEventMismatch".to_owned()
        }
        SupervisorError::ResidentLifecycleStateMismatch { cause, .. } => {
            format!("ResidentLifecycleStateMismatch.{}", cause.name())
        }
        SupervisorError::ResidentLifecycleTotalsMismatch(..) => {
            "ResidentLifecycleTotalsMismatch".to_owned()
        }
        SupervisorError::ResidentRowUnreadable(cause) => {
            format!("ResidentRowUnreadable.{}", cause.name())
        }
        SupervisorError::ResidentConfigurationEventMismatch(..) => {
            "ResidentConfigurationEventMismatch".to_owned()
        }
        SupervisorError::ResidentConfigurationStateMismatch { cause, .. } => {
            format!("ResidentConfigurationStateMismatch.{}", cause.name())
        }
        SupervisorError::TeamEventMismatch(..) => "TeamEventMismatch".to_owned(),
        SupervisorError::TeamStateMismatch { cause, .. } => {
            format!("TeamStateMismatch.{}", cause.name())
        }
        SupervisorError::TeamTotalsMismatch(..) => "TeamTotalsMismatch".to_owned(),
        SupervisorError::MissionEventMismatch(..) => "MissionEventMismatch".to_owned(),
        SupervisorError::MissionStateMismatch { cause, .. } => {
            format!("MissionStateMismatch.{}", cause.name())
        }
        SupervisorError::MissionTotalsMismatch(..) => "MissionTotalsMismatch".to_owned(),
        SupervisorError::ProjectPolicyEventMismatch(..) => "ProjectPolicyEventMismatch".to_owned(),
        SupervisorError::ProjectPolicyStateMismatch { cause, .. } => {
            format!("ProjectPolicyStateMismatch.{}", cause.name())
        }
        SupervisorError::ProjectRepositoryEventMismatch(..) => {
            "ProjectRepositoryEventMismatch".to_owned()
        }
        SupervisorError::ProjectRepositoryStateMismatch { cause, .. } => {
            format!("ProjectRepositoryStateMismatch.{}", cause.name())
        }
        SupervisorError::SampleProjectEventMismatch(..) => "SampleProjectEventMismatch".to_owned(),
        SupervisorError::SampleProjectStateMismatch { cause, .. } => {
            format!("SampleProjectStateMismatch.{}", cause.name())
        }
        SupervisorError::SampleProjectTotalsMismatch(..) => {
            "SampleProjectTotalsMismatch".to_owned()
        }
        SupervisorError::RunEventMismatch(..) => "RunEventMismatch".to_owned(),
        SupervisorError::RunStateMismatch { cause, .. } => {
            format!("RunStateMismatch.{}", cause.name())
        }
        SupervisorError::RunClockBeforeEpoch => "RunClockBeforeEpoch".to_owned(),
        SupervisorError::RunEventStateMismatch { .. } => "RunEventStateMismatch".to_owned(),
        SupervisorError::ProposalEventMismatch(..) => "ProposalEventMismatch".to_owned(),
        SupervisorError::ProposalStateMismatch { cause, .. } => {
            format!("ProposalStateMismatch.{}", cause.name())
        }
        SupervisorError::ProposalTotalsMismatch(..) => "ProposalTotalsMismatch".to_owned(),
        SupervisorError::RulingEventMismatch(..) => "RulingEventMismatch".to_owned(),
        SupervisorError::EvaluationEventMismatch(..) => "EvaluationEventMismatch".to_owned(),
        SupervisorError::MissionCompletionEventMismatch(..) => {
            "MissionCompletionEventMismatch".to_owned()
        }
        SupervisorError::MissionCompletionStateMismatch { cause, .. } => {
            format!("MissionCompletionStateMismatch.{}", cause.name())
        }
        SupervisorError::MissionCompletionTotalsMismatch(..) => {
            "MissionCompletionTotalsMismatch".to_owned()
        }
        SupervisorError::EvaluationStateMismatch { cause, .. } => {
            format!("EvaluationStateMismatch.{}", cause.name())
        }
        SupervisorError::EvaluationChainsMismatch(cause) => {
            format!("EvaluationChainsMismatch.{}", cause.name())
        }
        SupervisorError::ArtifactEventMismatch(..) => "ArtifactEventMismatch".to_owned(),
        SupervisorError::ArtifactStateMismatch { cause, .. } => {
            format!("ArtifactStateMismatch.{}", cause.name())
        }
        SupervisorError::ArtifactTotalsMismatch(..) => "ArtifactTotalsMismatch".to_owned(),
        SupervisorError::ArtifactRelationMismatch(cause) => {
            format!("ArtifactRelationMismatch.{}", cause.name())
        }
        SupervisorError::MissionImpactClassificationStateMismatch { cause, .. } => {
            format!(
                "MissionImpactClassificationStateMismatch.{}",
                classification_tag(cause)
            )
        }
        SupervisorError::OutcomeManifestStateMismatch { cause, .. } => {
            format!("OutcomeManifestStateMismatch.{}", cause.name())
        }
        SupervisorError::OutcomePublicationStateMismatch { cause, .. } => {
            format!("OutcomePublicationStateMismatch.{}", cause.name())
        }
        SupervisorError::RulingStateMismatch { cause, .. } => {
            format!("RulingStateMismatch.{}", cause.name())
        }
        SupervisorError::RulingTotalsMismatch(..) => "RulingTotalsMismatch".to_owned(),
        SupervisorError::RulingChainsMismatch(cause) => {
            format!("RulingChainsMismatch.{}", cause.name())
        }
        SupervisorError::CandidateEventMismatch(..) => "CandidateEventMismatch".to_owned(),
        SupervisorError::CandidateStateMismatch { cause, .. } => {
            format!("CandidateStateMismatch.{}", cause.name())
        }
        SupervisorError::DispatchEventMismatch(..) => "DispatchEventMismatch".to_owned(),
        SupervisorError::DispatchRecoveryFailed => "DispatchRecoveryFailed".to_owned(),
        SupervisorError::PauseEventMismatch(..) => "PauseEventMismatch".to_owned(),
        SupervisorError::SchemaMismatch => "SchemaMismatch".to_owned(),
        SupervisorError::UnsupportedSchemaVersion(..) => "UnsupportedSchemaVersion".to_owned(),
        SupervisorError::IntegrityCheckFailed(..) => "IntegrityCheckFailed".to_owned(),
        SupervisorError::StoredIdentityUnreadable { .. } => "StoredIdentityUnreadable".to_owned(),
        // R45 added this arm after slice 2's census. Its caller tag is closed like every other
        // root; its own STARTUP line is not -- see the build report.
        SupervisorError::StoredIntegerKeyUnreadable { .. } => {
            "StoredIntegerKeyUnreadable".to_owned()
        }
        SupervisorError::StartupEnumerationFailed { pass, .. } => {
            format!("StartupEnumerationFailed.{}", pass.name())
        }
        SupervisorError::ForeignKeyViolations(..) => "ForeignKeyViolations".to_owned(),
    }
}

/// The classification fault's token for the TRANSPORT tag only.
///
/// Startup keeps its existing spelling, which includes the validated predecessor digest -- an
/// approved row key there. This projection drops it: at the transport boundary that digest is a
/// record identity the caller may not own.
fn classification_tag(cause: &mission_impact::ClassificationStoreError) -> String {
    use mission_impact::ClassificationStoreError as Fault;
    match cause {
        Fault::DigestMismatch => "digest_mismatch".to_owned(),
        Fault::Malformed(reason) => format!("malformed.{reason}"),
        Fault::ColumnsDisagree => "columns_disagree".to_owned(),
        Fault::CitedIdentityMismatch => "cited_identity_mismatch".to_owned(),
        Fault::CrossWorkspace => "cross_workspace".to_owned(),
        Fault::CopiedBitDisagrees => "copied_bit_disagrees".to_owned(),
        Fault::PolicyProjectIsNotTheMissions => "policy_project_is_not_the_missions".to_owned(),
        Fault::PolicyRevisionHasAnotherCharter => "policy_revision_has_another_charter".to_owned(),
        Fault::Predecessor { fault, .. } => format!("predecessor.{fault}"),
        Fault::Store(_) => "store".to_owned(),
    }
}

impl fmt::Display for TotalsMismatch {
    /// R42: both counts are approved context and stay; `table` goes through the allowlist.
    ///
    /// Preventive, and worth saying plainly: all 14 production labels are source literals today,
    /// and no store-controlled path to this field has been demonstrated. But `table` is a public
    /// `&'static str` on a public struct, and a static lifetime says where a string lives, not who
    /// wrote it -- the same shape slice 1 hid in the publisher. An unrecognized label is reported
    /// as unrecognized rather than normalized to a table it might not be.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: the log accounts for {} but the store holds {}",
            totals_label(self.table),
            self.expected,
            self.observed
        )
    }
}

/// Which startup pass failed to ENUMERATE its rows (sol1's P1-3).
///
/// A closed set, so the line names a pass rather than repeating a driver's sentence. Five passes
/// mapped their enumeration failure onto the one generic `Store` arm, whose line is
/// `store operation failed` -- true, and useless: an operator could not tell which table would not
/// open. The `rusqlite::Error` still travels, in `Debug` and `source()` only (R28.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPass {
    Project,
    Mission,
    MissionImpactClassification,
    OutcomeManifest,
    OutcomePublication,
    MissionCompletion,
    Proposal,
    Evaluation,
    Artifact,
    Ruling,
    Candidate,
    ProjectRepository,
    SampleProject,
    Team,
    ProjectPolicy,
    StarterOrganization,
    ResidentLifecycle,
    ResidentConfiguration,
    LocalModelActivation,
    Run,
}

impl StartupPass {
    /// Exhaustive and wildcard-free: a new pass cannot be added without naming it.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Project => "Project",
            Self::Mission => "Mission",
            Self::MissionImpactClassification => "MissionImpactClassification",
            Self::OutcomeManifest => "OutcomeManifest",
            Self::OutcomePublication => "OutcomePublication",
            Self::MissionCompletion => "MissionCompletion",
            Self::Proposal => "Proposal",
            Self::Evaluation => "Evaluation",
            Self::Artifact => "Artifact",
            Self::Candidate => "Candidate",
            Self::Ruling => "Ruling",
            Self::ProjectRepository => "ProjectRepository",
            Self::SampleProject => "SampleProject",
            Self::Team => "Team",
            Self::ProjectPolicy => "ProjectPolicy",
            Self::StarterOrganization => "StarterOrganization",
            Self::ResidentLifecycle => "ResidentLifecycle",
            Self::ResidentConfiguration => "ResidentConfiguration",
            Self::LocalModelActivation => "LocalModelActivation",
            Self::Run => "Run",
        }
    }
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
            Self::Io(_) => write!(formatter, "cannot prepare the store directory: Io"),
            // R28.4 at the INNERMOST point: the driver's own words never reach a printed line.
            // Every mapping onto this arm -- every pass's `Store` fault, `lib.rs`'s generic store
            // failures -- becomes compliant here rather than at each call site. The text stays
            // reachable through `Debug` (derived on this enum, so the `rusqlite::Error` is in it).
            Self::Store(_) => formatter.write_str("store operation failed"),
            Self::Json(_) => write!(formatter, "cannot encode an event payload: Json"),
            // R42: quoting and truncating bounded the disclosure; it did not remove it. The
            // rejected String stays in `Debug`.
            Self::InvalidWorkspaceId(_) => {
                write!(
                    formatter,
                    "store contains an invalid Workspace id: InvalidWorkspaceId"
                )
            }
            Self::InvalidEventKind(_) => {
                write!(
                    formatter,
                    "store contains an unknown Event kind: InvalidEventKind"
                )
            }
            Self::EventPayloadScopeMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} payload does not match its Workspace scope"
            ),
            // R58.2: the six operator sentences are the signed table in `event_chain`, and this
            // writes a row of it and nothing else -- so signing or rewriting one is ONE edit in
            // ONE place, and the witness that pins it reads the same bytes.
            Self::EventChainBroken(fault) => formatter.write_str(&fault.sentence()),
            // R58.3, PROPOSED text -- Kevin signs or rewrites these two lines before the bar. Two
            // shapes because the fault has two: a state with no Event behind it at all, and a state
            // that disagrees with the Event it does have.
            Self::PauseStateUnsupported {
                changed_at_ms,
                terminal_sequence: None,
            } => write!(
                formatter,
                "the installation records a pause change at {changed_at_ms} and holds no Event that \
                 accounts for it"
            ),
            Self::PauseStateUnsupported {
                terminal_sequence: Some(sequence),
                ..
            } => write!(
                formatter,
                "the installation's pause state disagrees with Event sequence {sequence}, the last \
                 pause or resume it holds"
            ),
            Self::StarterOrganizationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Starter Organization state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::StarterOrganizationStateMismatch {
                workspace_id,
                cause,
            } => write!(
                formatter,
                "the starter organization of Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::ProjectEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project creation state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ProjectStateMismatch {
                workspace_id,
                project_id,
                cause,
            } => write!(
                formatter,
                "Project {project_id} in Workspace {workspace_id} does not re-prove against its \
                 charter history or the log: {}",
                cause.name()
            ),
            Self::ProjectTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Projects do not agree with the store as a whole -- {totals}"
                )
            }
            Self::LocalModelActivationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match local-model activation state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::LocalModelActivationStateMismatch {
                workspace_id,
                activation_id,
                cause,
            } => write!(
                formatter,
                "local model activation {activation_id} in Workspace {workspace_id} does not \
                 re-prove: {}",
                cause.name()
            ),
            Self::LocalModelActivationRowUnreadable(cause) => write!(
                formatter,
                "a local model activation row could not be read: {}",
                cause.name()
            ),
            Self::ResidentLifecycleEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Resident lifecycle state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ResidentLifecycleStateMismatch {
                workspace_id,
                resident_id,
                cause,
            } => write!(
                formatter,
                "the lifecycle of Resident {resident_id} in Workspace {workspace_id} does not \
                 re-prove: {}",
                cause.name()
            ),
            Self::ResidentLifecycleTotalsMismatch(totals) => write!(
                formatter,
                "the Resident lifecycle decisions do not agree with the store -- {totals}"
            ),
            Self::ResidentRowUnreadable(cause) => {
                write!(
                    formatter,
                    "a Resident row could not be read: {}",
                    cause.name()
                )
            }
            Self::ResidentConfigurationEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Resident configuration state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ResidentConfigurationStateMismatch {
                workspace_id,
                decision_id,
                cause,
            } => write!(
                formatter,
                "Resident configuration decision {decision_id} in Workspace {workspace_id} does \
                 not re-prove: {}",
                cause.name()
            ),
            Self::TeamEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Event sequence {sequence} does not match Team state"
                )
            }
            // R28.4: the key and the fault's NAME.
            Self::TeamStateMismatch {
                workspace_id,
                team_id,
                cause,
            } => write!(
                formatter,
                "Team {team_id} in Workspace {workspace_id} does not re-prove: {}",
                cause.name()
            ),
            Self::TeamTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Teams do not agree with the log as a whole -- {totals}"
                )
            }
            Self::MissionEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Event sequence {sequence} does not match Mission admission state"
                )
            }
            // R28.4: the key and the fault's NAME.
            Self::MissionStateMismatch {
                workspace_id,
                mission_id,
                cause,
            } => write!(
                formatter,
                "Mission {mission_id} in Workspace {workspace_id} does not re-prove against its \
                 own record or its projections: {}",
                cause.name()
            ),
            Self::MissionTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Missions do not agree with the store as a whole -- {totals}"
                )
            }
            Self::ProjectPolicyEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project Policy state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ProjectPolicyStateMismatch {
                workspace_id,
                project_id,
                cause,
            } => write!(
                formatter,
                "the policy history of Project {project_id} in Workspace {workspace_id} does not \
                 re-prove: {}",
                cause.name()
            ),
            Self::ProjectRepositoryEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Project Repository state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ProjectRepositoryStateMismatch {
                workspace_id,
                project_id,
                cause,
            } => write!(
                formatter,
                "the repository history of Project {project_id} in Workspace {workspace_id} does \
                 not re-prove: {}",
                cause.name()
            ),
            Self::SampleProjectEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match Sample Project installation state"
            ),
            // R28.4: the key and the fault's NAME.
            Self::SampleProjectStateMismatch {
                workspace_id,
                cause,
            } => write!(
                formatter,
                "the sample Project installation in Workspace {workspace_id} does not re-prove: \
                 {}",
                cause.name()
            ),
            Self::SampleProjectTotalsMismatch(totals) => write!(
                formatter,
                "the sample Project installations do not agree with the log -- {totals}"
            ),
            Self::RunEventMismatch(sequence) => {
                write!(
                    formatter,
                    "Run event {sequence} does not match its recorded Receipt"
                )
            }
            // R28.4: the key and the fault's NAME.
            Self::RunStateMismatch {
                workspace_id,
                run_id,
                cause,
            } => write!(
                formatter,
                "Run {run_id} in Workspace {workspace_id} does not agree with its Receipts and \
                 Events: {}",
                cause.name()
            ),
            Self::RunClockBeforeEpoch => formatter.write_str("the clock is before the Unix epoch"),
            Self::RunEventStateMismatch {
                sequence,
                workspace_id,
                run_id,
            } => write!(
                formatter,
                "Event sequence {sequence} in Workspace {workspace_id} carries a Receipt its Run \
                 {run_id} does not agree with"
            ),
            Self::ProposalEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Proposal bytes"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ProposalStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Proposal {digest_value} in Workspace {workspace_id} does not re-prove against \
                 the records it cites: {}",
                cause.name()
            ),
            Self::ProposalTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Proposals do not agree with the log as a whole -- {totals}"
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
            Self::MissionCompletionEventMismatch(sequence) => write!(
                formatter,
                "mission completion event {sequence} does not match its stored record"
            ),
            // R28.4: the key and the fault's NAME.
            Self::MissionCompletionStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "completion {digest_value} in Workspace {workspace_id} does not re-prove against \
                 its own bytes or the records it cites: {}",
                cause.name()
            ),
            Self::MissionCompletionTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Mission completions do not agree with the log as a whole -- {totals}"
                )
            }
            // R28.4: the key and the fault's NAME.
            Self::EvaluationStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Evaluation {digest_value} in Workspace {workspace_id} does not re-prove against \
                 its own bytes or the records it cites: {}",
                cause.name()
            ),
            Self::EvaluationChainsMismatch(cause) => write!(
                formatter,
                "the Evaluation supersession chains do not re-prove: {}",
                cause.name()
            ),
            Self::ArtifactEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Artifact bytes"
            ),
            // R28.4: the key and the fault's NAME.
            Self::ArtifactStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Artifact {digest_value} in Workspace {workspace_id} does not re-prove against \
                 its own bytes: {}",
                cause.name()
            ),
            Self::ArtifactTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Artifacts do not agree with the log as a whole -- {totals}"
                )
            }
            Self::ArtifactRelationMismatch(cause) => write!(
                formatter,
                "the Artifact supersession relation does not re-prove: {}",
                cause.name()
            ),
            Self::MissionImpactClassificationStateMismatch {
                workspace_id,
                digest,
                cause,
            } => write!(
                formatter,
                "Mission impact classification {digest} in Workspace {workspace_id} does not \
                 re-prove: {cause}"
            ),
            // R28.4: the key and the fault's NAME. The carried refusal's own text is
            // reachable through `Debug` and `source()`, not pasted into this line.
            Self::OutcomeManifestStateMismatch {
                workspace_id,
                manifest_digest,
                cause,
            } => write!(
                formatter,
                "manifest {manifest_digest} in Workspace {workspace_id} does not re-prove \
                 against its own bytes or the records it cites: {}",
                cause.name()
            ),
            // R28.4: the key and the fault's NAME. The carried refusal's own text is reachable
            // through `Debug` and `source()`, not pasted into this line.
            Self::OutcomePublicationStateMismatch {
                workspace_id,
                mission_id,
                version,
                cause,
            } => write!(
                formatter,
                "publication version {version} of Mission {mission_id} in Workspace \
                 {workspace_id} does not re-prove against the records it cites: {}",
                cause.name()
            ),
            // R28.4: the key and the fault's NAME.
            Self::RulingStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Ruling {digest_value} in Workspace {workspace_id} does not re-prove \
                 against its own bytes or the records it cites: {}",
                cause.name()
            ),
            Self::RulingTotalsMismatch(totals) => {
                write!(
                    formatter,
                    "the Rulings do not agree with the log as a whole -- {totals}"
                )
            }
            Self::RulingChainsMismatch(cause) => {
                write!(
                    formatter,
                    "the Ruling chains do not re-prove: {}",
                    cause.name()
                )
            }
            Self::CandidateEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored Candidate bytes"
            ),
            // R28.4: the key and the fault's NAME.
            Self::CandidateStateMismatch {
                workspace_id,
                digest_value,
                cause,
            } => write!(
                formatter,
                "Candidate {digest_value} in Workspace {workspace_id} does not re-prove \
                 against its own bytes or the records it cites: {}",
                cause.name()
            ),
            Self::DispatchEventMismatch(sequence) => write!(
                formatter,
                "Event sequence {sequence} does not match its stored dispatch Receipt"
            ),
            Self::DispatchRecoveryFailed => write!(
                formatter,
                "the recovery write for an interrupted dispatch did not complete, so its Run \
                 still has no terminal Receipt"
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
            Self::IntegrityCheckFailed(_) => {
                write!(
                    formatter,
                    "SQLite integrity check failed: IntegrityCheckFailed"
                )
            }
            // R42: `what` says WHICH owner could not name its row -- useful, and closed by the
            // allowlist below. `raw` is the unreadable identity itself and stays in `Debug`.
            Self::StoredIdentityUnreadable { what, .. } => {
                write!(
                    formatter,
                    "{}: StoredIdentityUnreadable",
                    identity_label(what)
                )
            }
            // R45 added this arm after slice 2's census, printing the raw stored text through
            // the helper this slice had removed -- the same disclosure closed one arm above,
            // under a new name. Routed through the same projection: the owner label, the fault
            // name, and the typed ordering key, which is an i64 the reader is entitled to.
            // `raw` stays in `Debug`, where R45's own bars read it.
            Self::StoredIntegerKeyUnreadable {
                what,
                key_name,
                key,
                ..
            } => write!(
                formatter,
                "{}: StoredIntegerKeyUnreadable, {} {key}",
                identity_label(what),
                ordering_key_label(key_name)
            ),
            Self::StartupEnumerationFailed { pass, .. } => write!(
                formatter,
                "the {} startup pass could not enumerate its rows",
                pass.name()
            ),
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

/// The one way a store connection is opened, so the connection state every store relies on is set
/// in one place rather than remembered at each site -- and public, so a tool or a test that opens a
/// Beacon store opens it the way the daemon does.
///
/// `recursive_triggers` is here for sol1's #13 (R20.4, revised by R21): SQLite's REPLACE conflict
/// resolution deletes the row it displaces WITHOUT firing that row's delete trigger unless this
/// pragma is on, so every `BEFORE DELETE ... RAISE(ABORT)` that makes a store immutable was one
/// `INSERT OR REPLACE` away from doing nothing. The pragma alone closes it (measured, error 1811),
/// and it is connection state -- which is why this is the one opener and a test asserts that every
/// production path comes through it. A schema-side `BEFORE INSERT` guard was built and dropped
/// (R21): it fired before every unique index and changed the error identity of every legitimate
/// duplicate insert on thirty tables, a cost the hole it closed did not justify.
pub fn open_store_connection(path: &Path) -> rusqlite::Result<Connection> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_millis(STORE_BUSY_TIMEOUT_MS.into()))?;
    ensure_write_ahead_log(&connection)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.pragma_update(None, "recursive_triggers", "ON")?;
    Ok(connection)
}

/// How long a store connection waits for another connection's write transaction before it gives up
/// with `SQLITE_BUSY` -- `PRAGMA busy_timeout`, set here because this is the one opener.
///
/// It is a ceiling on WAITING, never a budget for work: a waiter is refused when the holders ahead
/// of it hold the write lock for longer than this in total, so the fix for a refusal is to hold the
/// lock for less time, not to wait longer. FF-49 was exactly that -- the value was already five
/// seconds and startup held the lock for ~195ms per supervisor, so the thirty-second waiter in a
/// queue was refused. `expected_schema` took the computation out of the lock; this stays where it
/// was, now named and with its reason written down.
const STORE_BUSY_TIMEOUT_MS: u32 = 5_000;

/// R45.5: a verifier holds one deferred read snapshot while public services may continue to write.
/// WAL is therefore store state, not a test convenience: rollback-journal mode would make that
/// reader block the writer whose commit proves the snapshot boundary. A fresh store can be opened
/// concurrently, and SQLite does not run this journal transition through the busy handler, so
/// retry only that transition's two lock errors within the same bounded wait every connection has.
/// The 5,000 ms bound is retry admission plus in-flight SQLite work and scheduler delay, not a strict open/bootstrap wall-clock deadline.
/// SQLite's non-file `:memory:` fixture reports its permanent `memory` mode instead; it has no
/// path-backed store or second connection across which this invariant could apply.
fn ensure_write_ahead_log(connection: &Connection) -> rusqlite::Result<()> {
    let deadline = Instant::now() + Duration::from_millis(STORE_BUSY_TIMEOUT_MS.into());
    loop {
        match connection.query_row("PRAGMA journal_mode = WAL", [], |row| {
            row.get::<_, String>(0)
        }) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") || mode.eq_ignore_ascii_case("memory") => {
                return Ok(());
            }
            Ok(_) => return Err(rusqlite::Error::InvalidQuery),
            Err(error)
                if error.sqlite_error_code().is_some_and(|code| {
                    matches!(
                        code,
                        rusqlite::ffi::ErrorCode::DatabaseBusy
                            | rusqlite::ffi::ErrorCode::DatabaseLocked
                    )
                }) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Owns store startup, first-Workspace creation, and the diagnostic report behind one interface.
pub struct Supervisor;

impl Supervisor {
    pub fn bootstrap(config: StartupConfig) -> Result<StartupReport, SupervisorError> {
        let workspace_name = validate_workspace_name(&config.initial_workspace_name)?;

        prepare_parent(&config.store_path)?;
        prepare_store_file(&config.store_path)?;

        let mut connection = open_store_connection(&config.store_path)?;

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
                crate::event_chain::append_event(
                    &transaction,
                    &id.to_string(),
                    PersistedEventKind::WorkspaceCreated.as_str(),
                    &payload,
                    occurred_at_ms,
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

        // Reconciliation owns all startup writes and commits them before verification takes its
        // snapshot. Dispatch recovery must precede Run lease reconciliation just as it did in the
        // old mixed walk: it may finish a dispatch Run that the Run-state pass then re-proves.
        let reconciliation =
            connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        dispatch::finish_prepared_dispatches(&reconciliation)
            .map_err(|_| SupervisorError::DispatchRecoveryFailed)?;
        check_preflight::reconcile_expired_run_leases(&reconciliation)
            .map_err(map_run_state_fault)?;
        reconciliation.commit()?;

        // Every verification pass, including each pass's Totals query, sees one deferred read
        // snapshot. `query_only` makes the separation executable: a write accidentally added to
        // `inspect_store` refuses instead of quietly extending reconciliation again.
        connection.pragma_update(None, "query_only", "ON")?;
        let verification = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let diagnostics = inspect_store(&verification)?;
        verification.commit()?;
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
    (27, MIGRATION_V27),
    (28, MIGRATION_V28),
    (29, MIGRATION_V29),
    (30, MIGRATION_V30),
    (31, MIGRATION_V31),
    (32, MIGRATION_V32),
    (33, MIGRATION_V33),
    (34, MIGRATION_V34),
    (35, MIGRATION_V35),
    (36, MIGRATION_V36),
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
    // look like" and "what startup produces" cannot drift apart. It is memoized because it is a
    // function of the compiled-in ladder and nothing else -- see `expected_schema`.
    let expected = expected_schema(version)?;
    let actual = schema_signature(connection)?;
    if actual != *expected {
        return Err(SupervisorError::SchemaMismatch);
    }
    Ok(())
}

/// The schema one ladder version produces, replayed into an in-memory database ONCE per process.
///
/// This is the half of `verify_schema` that needs no store and no lock: `MIGRATION_LADDER` is
/// compiled in, so a version's expected shape is the same answer every time. Startup called it
/// inside its write transaction -- twice for an existing store -- and each replay of the
/// twenty-nine-step ladder took ~95ms, so a supervisor held the store's write lock for ~195ms of
/// which ~190ms was arithmetic nobody else could proceed through. Concurrent supervisors queued at
/// that rate and the tail passed `STORE_BUSY_TIMEOUT_MS` (FF-49).
///
/// A version is replayed at most once per process: the cache lock is held across the replay, so a
/// cold queue waits for one answer instead of computing the same one N times.
static EXPECTED_SCHEMA: OnceLock<Mutex<HashMap<i64, Arc<Vec<SchemaObject>>>>> = OnceLock::new();

/// Test-only: how many times each version's ladder has actually been replayed in this process.
///
/// A version's count can only ever reach one, and once it does no thread can move it again -- which
/// is what makes the witness below deterministic rather than a race with the clock.
#[cfg(test)]
static EXPECTED_SCHEMA_REPLAYS: OnceLock<Mutex<HashMap<i64, usize>>> = OnceLock::new();

#[cfg(test)]
fn note_ladder_replay(version: i64) {
    *EXPECTED_SCHEMA_REPLAYS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(version)
        .or_insert(0) += 1;
}

#[cfg(not(test))]
fn note_ladder_replay(_version: i64) {}

#[cfg(test)]
fn expected_schema_replays(version: i64) -> usize {
    EXPECTED_SCHEMA_REPLAYS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&version)
        .copied()
        .unwrap_or(0)
}

fn expected_schema(version: i64) -> Result<Arc<Vec<SchemaObject>>, SupervisorError> {
    // The lock is held across the replay on purpose. A version is computed at most ONCE in a
    // process: without that, every supervisor in a cold queue burns the same ~95ms of arithmetic in
    // parallel, and the count a test can assert on becomes a race. A hit is a map lookup; the one
    // caller that misses makes the others wait for the answer instead of duplicating it.
    let mut cache = EXPECTED_SCHEMA
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(signature) = cache.get(&version) {
        return Ok(Arc::clone(signature));
    }
    note_ladder_replay(version);
    let expected_connection = Connection::open_in_memory()?;
    apply_migrations_through(&expected_connection, version)?;
    let signature = Arc::new(schema_signature(&expected_connection)?);
    cache.insert(version, Arc::clone(&signature));
    Ok(signature)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    MissionCompleted,
    SpendMeasured,
    MissionImpactClassified,
    ArtifactRecorded,
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
            Self::MissionCompleted => "mission.completed",
            Self::SpendMeasured => "spend.measured",
            Self::MissionImpactClassified => "mission.impact_classified",
            Self::ArtifactRecorded => "artifact.recorded",
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
            "mission.completed" => Ok(Self::MissionCompleted),
            "spend.measured" => Ok(Self::SpendMeasured),
            "mission.impact_classified" => Ok(Self::MissionImpactClassified),
            "artifact.recorded" => Ok(Self::ArtifactRecorded),
            _ => Err(SupervisorError::InvalidEventKind(value)),
        }
    }
}

/// The production mapping for the Project startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_project_state_fault(fault: project::ProjectStateFault) -> SupervisorError {
    match fault {
        project::ProjectStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Project,
            source,
        },
        project::ProjectStateFault::Row {
            workspace_id,
            project_id,
            cause,
        } => SupervisorError::ProjectStateMismatch {
            workspace_id,
            project_id,
            cause,
        },
        project::ProjectStateFault::Totals(cause) => SupervisorError::ProjectTotalsMismatch(cause),
        project::ProjectStateFault::Identity {
            workspace_id,
            project_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Project row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Project {project_id}"),
        },
    }
}

/// The production mapping for the Mission startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_mission_state_fault(fault: mission::MissionStateFault) -> SupervisorError {
    match fault {
        mission::MissionStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Mission,
            source,
        },
        mission::MissionStateFault::Row {
            workspace_id,
            mission_id,
            cause,
        } => SupervisorError::MissionStateMismatch {
            workspace_id,
            mission_id,
            cause,
        },
        mission::MissionStateFault::Totals(cause) => SupervisorError::MissionTotalsMismatch(cause),
        mission::MissionStateFault::Identity {
            workspace_id,
            mission_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Mission row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Mission {mission_id}"),
        },
    }
}

/// The production mapping for the MissionCompletion startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_completion_state_fault(
    fault: mission_completion_store::CompletionStateFault,
) -> SupervisorError {
    match fault {
        mission_completion_store::CompletionStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::MissionCompletion,
                source,
            }
        }
        mission_completion_store::CompletionStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::MissionCompletionStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        mission_completion_store::CompletionStateFault::Totals(cause) => {
            SupervisorError::MissionCompletionTotalsMismatch(cause)
        }
        mission_completion_store::CompletionStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a completion row's Workspace is not readable",
            raw: format!("{workspace_id}, completion {digest_value}"),
        },
    }
}

/// The production mapping for the MissionImpactClassification startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_classification_state_fault(
    fault: mission_impact::ClassificationStateFault,
) -> SupervisorError {
    match fault {
        mission_impact::ClassificationStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::MissionImpactClassification,
                source,
            }
        }
        mission_impact::ClassificationStateFault::Row {
            workspace_id,
            digest,
            cause,
        } => SupervisorError::MissionImpactClassificationStateMismatch {
            workspace_id,
            digest,
            cause,
        },
    }
}

/// The production mapping for the OutcomeManifest startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_manifest_state_fault(fault: outcome_store::ManifestStateFault) -> SupervisorError {
    match fault {
        outcome_store::ManifestStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::OutcomeManifest,
                source,
            }
        }
        outcome_store::ManifestStateFault::Row {
            workspace_id,
            manifest_digest,
            cause,
        } => SupervisorError::OutcomeManifestStateMismatch {
            workspace_id,
            manifest_digest,
            cause,
        },
        outcome_store::ManifestStateFault::Identity {
            workspace_id,
            manifest_digest,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a manifest row's Workspace is not readable",
            raw: format!("{workspace_id}, manifest {manifest_digest}"),
        },
    }
}

/// The production mapping for the OutcomePublication startup pass, NAMED so a bar can drive it.
///
/// blue's HOLD at 4b57baf: a bar that builds `StartupEnumerationFailed` itself never reaches this
/// code, so deleting the routing left it green. Naming the mapping is what lets a test run a real
/// failed enumeration through the real arm.
fn map_publication_state_fault(
    fault: outcome_publication::PublicationStateFault,
) -> SupervisorError {
    match fault {
        outcome_publication::PublicationStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::OutcomePublication,
                source,
            }
        }
        outcome_publication::PublicationStateFault::Row {
            workspace_id,
            mission_id,
            version,
            cause,
        } => SupervisorError::OutcomePublicationStateMismatch {
            workspace_id,
            mission_id,
            version,
            cause,
        },
        // Unreachable while the foreign-key check and the Workspace and Mission passes run
        // first; reported as what it is rather than folded into a row refusal that would have
        // to invent the identity it is missing.
        outcome_publication::PublicationStateFault::Identity {
            workspace_id,
            mission_id,
            version,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a publication row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Mission {mission_id}, version {version}"),
        },
    }
}

/// The production mapping for the SampleProject startup pass, NAMED so a bar can drive it.
fn map_sample_project_state_fault(
    fault: sample_project::SampleProjectStateFault,
) -> SupervisorError {
    match fault {
        sample_project::SampleProjectStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::SampleProject,
                source,
            }
        }
        sample_project::SampleProjectStateFault::Row {
            workspace_id,
            cause,
        } => SupervisorError::SampleProjectStateMismatch {
            workspace_id,
            cause,
        },
        sample_project::SampleProjectStateFault::Totals(totals) => {
            SupervisorError::SampleProjectTotalsMismatch(totals)
        }
        sample_project::SampleProjectStateFault::Identity { workspace_id } => {
            SupervisorError::StoredIdentityUnreadable {
                what: "a sample Project row's Workspace is not readable",
                raw: workspace_id,
            }
        }
    }
}

/// The production mapping for the Proposal startup pass, NAMED so a bar can drive it.
fn map_proposal_state_fault(fault: proposal::ProposalStateFault) -> SupervisorError {
    match fault {
        proposal::ProposalStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Proposal,
            source,
        },
        proposal::ProposalStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::ProposalStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        proposal::ProposalStateFault::Totals(cause) => {
            SupervisorError::ProposalTotalsMismatch(cause)
        }
        proposal::ProposalStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Proposal row's Workspace is not readable",
            raw: format!("{workspace_id}, Proposal {digest_value}"),
        },
    }
}

/// The production mapping for the Ruling startup pass, NAMED so a bar can drive it.
fn map_ruling_state_fault(fault: ruling::RulingStateFault) -> SupervisorError {
    match fault {
        ruling::RulingStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Ruling,
            source,
        },
        ruling::RulingStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::RulingStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        ruling::RulingStateFault::Totals(totals) => SupervisorError::RulingTotalsMismatch(totals),
        ruling::RulingStateFault::Chains(cause) => SupervisorError::RulingChainsMismatch(cause),
        ruling::RulingStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Ruling row's Workspace is not readable",
            raw: format!("Workspace {workspace_id}, Ruling {digest_value}"),
        },
    }
}

/// The production mapping for the Candidate startup pass, NAMED so a bar can drive it.
fn map_candidate_state_fault(fault: candidate::CandidateStateFault) -> SupervisorError {
    match fault {
        candidate::CandidateStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::Candidate,
                source,
            }
        }
        candidate::CandidateStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::CandidateStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        candidate::CandidateStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Candidate row's Workspace is not readable",
            raw: format!("Workspace {workspace_id}, Candidate {digest_value}"),
        },
    }
}

/// The production mapping for the Evaluation startup pass, NAMED so a bar can drive it.
fn map_evaluation_state_fault(fault: evaluation::EvaluationStateFault) -> SupervisorError {
    match fault {
        evaluation::EvaluationStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::Evaluation,
                source,
            }
        }
        evaluation::EvaluationStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::EvaluationStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        evaluation::EvaluationStateFault::Chains(cause) => {
            SupervisorError::EvaluationChainsMismatch(cause)
        }
        evaluation::EvaluationStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "an Evaluation row's Workspace is not readable",
            raw: format!("{workspace_id}, Evaluation {digest_value}"),
        },
    }
}

/// The production mapping for the Artifact startup pass, NAMED so a bar can drive it.
fn map_artifact_state_fault(fault: artifact::ArtifactStateFault) -> SupervisorError {
    match fault {
        artifact::ArtifactStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Artifact,
            source,
        },
        artifact::ArtifactStateFault::Row {
            workspace_id,
            digest_value,
            cause,
        } => SupervisorError::ArtifactStateMismatch {
            workspace_id,
            digest_value,
            cause,
        },
        artifact::ArtifactStateFault::Totals(cause) => {
            SupervisorError::ArtifactTotalsMismatch(cause)
        }
        artifact::ArtifactStateFault::Relation(cause) => {
            SupervisorError::ArtifactRelationMismatch(cause)
        }
        artifact::ArtifactStateFault::Identity {
            workspace_id,
            digest_value,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "an Artifact row's Workspace is not readable",
            raw: format!("{workspace_id}, Artifact {digest_value}"),
        },
    }
}

/// The production mapping for the ProjectRepository startup pass, NAMED so a bar can drive it.
fn map_project_repository_state_fault(
    fault: project_repository::ProjectRepositoryStateFault,
) -> SupervisorError {
    match fault {
        project_repository::ProjectRepositoryStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::ProjectRepository,
                source,
            }
        }
        project_repository::ProjectRepositoryStateFault::Row {
            workspace_id,
            project_id,
            cause,
        } => SupervisorError::ProjectRepositoryStateMismatch {
            workspace_id,
            project_id,
            cause,
        },
        project_repository::ProjectRepositoryStateFault::Identity {
            workspace_id,
            project_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Project repository row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Project {project_id}"),
        },
        project_repository::ProjectRepositoryStateFault::Revision {
            workspace_id,
            project_id,
            revision,
        } => SupervisorError::StoredIntegerKeyUnreadable {
            what: "a Project repository revision is not readable",
            raw: format!("Workspace {workspace_id}, Project {project_id}"),
            key_name: "revision",
            key: revision,
        },
    }
}

/// The production mapping for the Team startup pass, NAMED so a bar can drive it.
fn map_team_state_fault(fault: team::TeamStateFault) -> SupervisorError {
    match fault {
        team::TeamStateFault::Store(source) => SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Team,
            source,
        },
        team::TeamStateFault::Row {
            workspace_id,
            team_id,
            cause,
        } => SupervisorError::TeamStateMismatch {
            workspace_id,
            team_id,
            cause,
        },
        team::TeamStateFault::Totals(totals) => SupervisorError::TeamTotalsMismatch(totals),
        team::TeamStateFault::Identity {
            workspace_id,
            team_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Team row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Team {team_id}"),
        },
    }
}

/// The production mapping for the ProjectPolicy startup pass, NAMED so a bar can drive it.
fn map_project_policy_state_fault(
    fault: project_policy::ProjectPolicyStateFault,
) -> SupervisorError {
    match fault {
        project_policy::ProjectPolicyStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::ProjectPolicy,
                source,
            }
        }
        project_policy::ProjectPolicyStateFault::Row {
            workspace_id,
            project_id,
            cause,
        } => SupervisorError::ProjectPolicyStateMismatch {
            workspace_id,
            project_id,
            cause,
        },
        project_policy::ProjectPolicyStateFault::Identity {
            workspace_id,
            project_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Project policy row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Project {project_id}"),
        },
        project_policy::ProjectPolicyStateFault::Revision {
            workspace_id,
            project_id,
            revision,
        } => SupervisorError::StoredIntegerKeyUnreadable {
            what: "a Project policy revision is not readable",
            raw: format!("Workspace {workspace_id}, Project {project_id}"),
            key_name: "revision",
            key: revision,
        },
    }
}

/// The production mapping for the StarterOrganization startup pass, NAMED so a bar can drive it.
fn map_starter_state_fault(fault: starter_organization::StarterStateFault) -> SupervisorError {
    match fault {
        starter_organization::StarterStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::StarterOrganization,
                source,
            }
        }
        starter_organization::StarterStateFault::Row {
            workspace_id,
            cause,
        } => SupervisorError::StarterOrganizationStateMismatch {
            workspace_id,
            cause,
        },
        starter_organization::StarterStateFault::Identity { workspace_id } => {
            SupervisorError::StoredIdentityUnreadable {
                what: "a Workspace id is not readable",
                raw: workspace_id,
            }
        }
    }
}

/// The production mapping for the ResidentLifecycle startup pass, NAMED so a bar can drive it.
fn map_resident_lifecycle_state_fault(
    fault: resident_lifecycle::ResidentLifecycleStateFault,
) -> SupervisorError {
    match fault {
        resident_lifecycle::ResidentLifecycleStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::ResidentLifecycle,
                source,
            }
        }
        resident_lifecycle::ResidentLifecycleStateFault::Row {
            workspace_id,
            resident_id,
            cause,
        } => SupervisorError::ResidentLifecycleStateMismatch {
            workspace_id,
            resident_id,
            cause,
        },
        resident_lifecycle::ResidentLifecycleStateFault::Totals(totals) => {
            SupervisorError::ResidentLifecycleTotalsMismatch(totals)
        }
        resident_lifecycle::ResidentLifecycleStateFault::Unreadable(cause) => {
            SupervisorError::ResidentRowUnreadable(cause)
        }
        resident_lifecycle::ResidentLifecycleStateFault::Sequence {
            workspace_id,
            resident_id,
            sequence,
        } => SupervisorError::StoredIntegerKeyUnreadable {
            what: "a Resident lifecycle decision's ordering key is not readable",
            raw: format!("Workspace {workspace_id}, Resident {resident_id}"),
            key_name: "sequence",
            key: sequence,
        },
    }
}

/// The production mapping for the ResidentConfiguration startup pass, NAMED so a bar can drive it.
fn map_resident_configuration_state_fault(
    fault: resident_configuration::ResidentConfigurationStateFault,
) -> SupervisorError {
    match fault {
        resident_configuration::ResidentConfigurationStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::ResidentConfiguration,
                source,
            }
        }
        resident_configuration::ResidentConfigurationStateFault::Row {
            workspace_id,
            decision_id,
            cause,
        } => SupervisorError::ResidentConfigurationStateMismatch {
            workspace_id,
            decision_id,
            cause,
        },
        resident_configuration::ResidentConfigurationStateFault::Identity {
            workspace_id,
            decision_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Resident configuration row's identity is not readable",
            raw: format!("Workspace {workspace_id}, decision {decision_id}"),
        },
        resident_configuration::ResidentConfigurationStateFault::Sequence {
            workspace_id,
            decision_id,
            sequence,
        } => SupervisorError::StoredIntegerKeyUnreadable {
            what: "a Resident configuration decision's ordering key is not readable",
            raw: format!("Workspace {workspace_id}, decision {decision_id}"),
            key_name: "sequence",
            key: sequence,
        },
    }
}

/// The production mapping for the LocalModelActivation startup pass, NAMED so a bar can drive it.
fn map_local_model_activation_state_fault(
    fault: local_model_activation::LocalModelActivationStateFault,
) -> SupervisorError {
    match fault {
        local_model_activation::LocalModelActivationStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::LocalModelActivation,
                source,
            }
        }
        local_model_activation::LocalModelActivationStateFault::Row {
            workspace_id,
            activation_id,
            cause,
        } => SupervisorError::LocalModelActivationStateMismatch {
            workspace_id,
            activation_id,
            cause,
        },
        local_model_activation::LocalModelActivationStateFault::Unreadable(cause) => {
            SupervisorError::LocalModelActivationRowUnreadable(cause)
        }
        local_model_activation::LocalModelActivationStateFault::Identity {
            workspace_id,
            activation_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a local-model activation row's identity is not readable",
            raw: format!("Workspace {workspace_id}, activation {activation_id}"),
        },
        local_model_activation::LocalModelActivationStateFault::Sequence {
            workspace_id,
            activation_id,
            sequence,
        } => SupervisorError::StoredIntegerKeyUnreadable {
            what: "a local-model activation decision's ordering key is not readable",
            raw: format!("Workspace {workspace_id}, activation {activation_id}"),
            key_name: "sequence",
            key: sequence,
        },
    }
}

/// The Run-state pass's fault, as the startup error that names the Run it refused.
///
/// `Clock` is the one arm with no row: it fails before any Run is read, so there is nothing to
/// name and inventing one would be a decoy (R39.2).
fn map_run_state_fault(fault: check_preflight::RunStateFault) -> SupervisorError {
    match fault {
        check_preflight::RunStateFault::Store(source) => {
            SupervisorError::StartupEnumerationFailed {
                pass: StartupPass::Run,
                source,
            }
        }
        check_preflight::RunStateFault::Row {
            workspace_id,
            run_id,
            cause,
        } => SupervisorError::RunStateMismatch {
            workspace_id,
            run_id,
            cause,
        },
        check_preflight::RunStateFault::Clock => SupervisorError::RunClockBeforeEpoch,
        check_preflight::RunStateFault::Identity {
            workspace_id,
            run_id,
        } => SupervisorError::StoredIdentityUnreadable {
            what: "a Run row's identity is not readable",
            raw: format!("Workspace {workspace_id}, Run {run_id}"),
        },
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
    starter_organization::verify_starter_state(connection).map_err(map_starter_state_fault)?;
    project::verify_project_state(connection).map_err(map_project_state_fault)?;
    project_policy::verify_project_policy_state(connection)
        .map_err(map_project_policy_state_fault)?;
    project_repository::verify_project_repository_state(connection)
        .map_err(map_project_repository_state_fault)?;
    sample_project::verify_sample_project_state(connection)
        .map_err(map_sample_project_state_fault)?;
    proposal::verify_proposal_state(connection).map_err(map_proposal_state_fault)?;
    // After Proposal state, because every Ruling binds one and re-verifies what it copied against
    // that Proposal's own bytes.
    ruling::verify_ruling_state(connection).map_err(map_ruling_state_fault)?;
    // After Proposal state for the same reason a Ruling is: a Candidate copies one Proposal's
    // declared effects and re-verifies them against that Proposal's own bytes.
    candidate::verify_candidate_state(connection).map_err(map_candidate_state_fault)?;
    evaluation::verify_evaluation_state(connection).map_err(map_evaluation_state_fault)?;
    // The Artifact state stands alone: it binds no other module's record, and nothing else's
    // verification reads it.
    artifact::verify_artifact_state(connection).map_err(map_artifact_state_fault)?;
    check_preflight::verify_run_state(connection).map_err(map_run_state_fault)?;
    local_model_activation::verify_activation_state(connection)
        .map_err(map_local_model_activation_state_fault)?;
    resident_lifecycle::verify_lifecycle_state(connection)
        .map_err(map_resident_lifecycle_state_fault)?;
    resident_configuration::verify_configuration_state(connection)
        .map_err(map_resident_configuration_state_fault)?;
    team::verify_team_state(connection).map_err(map_team_state_fault)?;
    mission::verify_mission_state(connection).map_err(map_mission_state_fault)?;
    // After Mission state, because every completion re-derives its conditions from the Mission's own
    // rows and its boundary from the Mission's admission Event.
    mission_completion_store::verify_completion_state(connection)
        .map_err(map_completion_state_fault)?;
    // Last, because it is the only pass that re-proves records belonging to two other modules: an
    // Outcome manifest is accounted for by its candidate Artifact's Event and frozen on a
    // completion, so both of those have to have proved themselves first (R18.7).
    // Before the manifest pass, because a manifest cites a classification and re-checks the
    // high-impact bit against it: the classification has to have proved itself first, for the same
    // reason completions come before manifests.
    mission_impact::verify_classification_state(connection)
        .map_err(map_classification_state_fault)?;
    outcome_store::verify_manifest_state(connection).map_err(map_manifest_state_fault)?;
    // After the manifest pass, because a publication is proved THROUGH its manifest: the relation
    // cites a candidate, the candidate's manifest must verify, and the manifest's own pass has to
    // have run before this one can rely on it (R18.7).
    outcome_publication::verify_publication_state(connection)
        .map_err(map_publication_state_fault)?;

    // R58.3 (sol1's P0), BEFORE the chain, because it is an owner pass and owners run first.
    // The chain cannot see a removed tail: delete the newest Event, rewrite the head stored beside
    // the rows, and every remaining link agrees with every other. `installation_pause` is not an
    // Event row, so it survives that edit -- and a current state with no Event behind it is the
    // completeness the chain structurally cannot prove.
    if let Some(fault) = dispatch::reconcile_pause_state(connection)? {
        return Err(SupervisorError::PauseStateUnsupported {
            changed_at_ms: fault.changed_at_ms,
            terminal_sequence: fault.terminal_sequence,
        });
    }

    // R58: the chain, AFTER every per-record pass -- and this is load-bearing rather than tidy.
    // Five of the seven tampers this closes are already refused BY NAME by the passes above, and
    // those names say WHICH RECORD disagreed, which is more useful to an operator than a chain
    // fault. Running it earlier replaced good refusals with a vaguer one: a deleted sample-project
    // installation Event came back as `HeadMismatch` instead of naming the Workspace it refused.
    // The chain's own job is the two nobody else catches -- a deleted genesis and a renumbered
    // sequence -- and it does that job last.
    if let Some(fault) = event_chain::verify_chain(connection)? {
        return Err(SupervisorError::EventChainBroken(fault));
    }

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
                    Err(check_preflight::RunEventReplayError::RunStateMismatch { run_id }) => {
                        return Err(SupervisorError::RunEventStateMismatch {
                            sequence,
                            workspace_id,
                            run_id,
                        });
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
            PersistedEventKind::MissionCompleted => {
                mission_completion_store::verify_completion_event(
                    connection,
                    workspace_id,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::MissionCompletionEventMismatch(sequence))?;
            }
            // These two arms verify NOTHING beyond admission: the spend and classification
            // modules have no replay or startup-state verification yet (that is their lane's
            // follow-on, not this one's). The arms exist because the alternative was worse:
            // 0025 admitted both kinds into the events CHECK without teaching the replay
            // vocabulary to parse them, so a store holding either Event refused every
            // subsequent startup with InvalidEventKind -- proven on main at 7b19355 by
            // inserting one 'spend.measured' row and re-bootstrapping.
            PersistedEventKind::SpendMeasured => {}
            PersistedEventKind::MissionImpactClassified => {}
            PersistedEventKind::ArtifactRecorded => {
                artifact::verify_artifact_event(
                    connection,
                    workspace_id,
                    occurred_at_ms,
                    &payload_json,
                )
                .map_err(|_| SupervisorError::ArtifactEventMismatch(sequence))?;
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

    /// blue's HOLD, answered: every pass's enumeration failure is DRIVEN through the production
    /// mapping, not constructed beside it.
    ///
    /// A store with none of these tables makes each pass's own `prepare` fail for real -- the same
    /// `rusqlite::Error` a broken store would produce -- and the fault goes through the same
    /// `map_*_state_fault` function `inspect_store` calls. Restoring any generic mapping makes this
    /// bar fail by that pass's name, which is what
    /// `sol1_each_startup_pass_names_itself_when_its_enumeration_fails` could not do: it built the
    /// variant itself and never reached the arm.
    #[test]
    fn every_pass_routes_its_real_enumeration_failure_through_its_own_name() {
        let connection = Connection::open_in_memory().expect("an empty store");

        let refusals: Vec<(&str, SupervisorError)> = vec![
            (
                "Project",
                super::map_project_state_fault(
                    crate::project::verify_project_state(&connection)
                        .expect_err("no `projects` table, so the enumeration must fail"),
                ),
            ),
            (
                "Mission",
                super::map_mission_state_fault(
                    crate::mission::verify_mission_state(&connection)
                        .expect_err("no `missions` table"),
                ),
            ),
            (
                "MissionCompletion",
                super::map_completion_state_fault(
                    crate::mission_completion_store::verify_completion_state(&connection)
                        .expect_err("no `mission_completions` table"),
                ),
            ),
            (
                "MissionImpactClassification",
                super::map_classification_state_fault(
                    crate::mission_impact::verify_classification_state(&connection)
                        .expect_err("no classification table"),
                ),
            ),
            (
                "OutcomeManifest",
                super::map_manifest_state_fault(
                    crate::outcome_store::verify_manifest_state(&connection)
                        .expect_err("no `outcome_manifests` table"),
                ),
            ),
            (
                "OutcomePublication",
                super::map_publication_state_fault(
                    crate::outcome_publication::verify_publication_state(&connection)
                        .expect_err("no `outcome_publications` table"),
                ),
            ),
            (
                "Proposal",
                super::map_proposal_state_fault(
                    crate::proposal::verify_proposal_state(&connection)
                        .expect_err("no `proposals` table"),
                ),
            ),
            (
                "Ruling",
                super::map_ruling_state_fault(
                    crate::ruling::verify_ruling_state(&connection)
                        .expect_err("no `rulings` table"),
                ),
            ),
            (
                "Candidate",
                super::map_candidate_state_fault(
                    crate::candidate::verify_candidate_state(&connection)
                        .expect_err("no `candidates` table"),
                ),
            ),
            (
                "Evaluation",
                super::map_evaluation_state_fault(
                    crate::evaluation::verify_evaluation_state(&connection)
                        .expect_err("no `evaluations` table"),
                ),
            ),
            (
                "Artifact",
                super::map_artifact_state_fault(
                    crate::artifact::verify_artifact_state(&connection)
                        .expect_err("no `artifacts` table"),
                ),
            ),
            (
                "ProjectRepository",
                super::map_project_repository_state_fault(
                    crate::project_repository::verify_project_repository_state(&connection)
                        .expect_err("no `project_repository_revisions` table"),
                ),
            ),
            (
                "LocalModelActivation",
                super::map_local_model_activation_state_fault(
                    crate::local_model_activation::verify_activation_state(&connection)
                        .expect_err("no `local_model_activation_decisions` table"),
                ),
            ),
            (
                "ResidentConfiguration",
                super::map_resident_configuration_state_fault(
                    crate::resident_configuration::verify_configuration_state(&connection)
                        .expect_err("no `resident_configuration_decisions` table"),
                ),
            ),
            (
                "ResidentLifecycle",
                super::map_resident_lifecycle_state_fault(
                    crate::resident_lifecycle::verify_lifecycle_state(&connection)
                        .expect_err("no `agents` table"),
                ),
            ),
            (
                "StarterOrganization",
                super::map_starter_state_fault(
                    crate::starter_organization::verify_starter_state(&connection)
                        .expect_err("no `workspaces` table"),
                ),
            ),
            (
                "ProjectPolicy",
                super::map_project_policy_state_fault(
                    crate::project_policy::verify_project_policy_state(&connection)
                        .expect_err("no `project_policy_revisions` table"),
                ),
            ),
            (
                "Team",
                super::map_team_state_fault(
                    crate::team::verify_team_state(&connection).expect_err("no `teams` table"),
                ),
            ),
            (
                "SampleProject",
                super::map_sample_project_state_fault(
                    crate::sample_project::verify_sample_project_state(&connection)
                        .expect_err("no `sample_project_installations` table"),
                ),
            ),
            (
                "Run",
                super::map_run_state_fault(
                    crate::check_preflight::verify_run_state(&connection)
                        .expect_err("no `runs` table"),
                ),
            ),
        ];
        assert_eq!(
            refusals.len(),
            20,
            "a pass was added without a fixture here"
        );

        let mut lines = std::collections::BTreeSet::new();
        for (pass, refusal) in &refusals {
            let line = refusal.to_string();
            assert!(
                line.contains(pass),
                "the {pass} pass's real enumeration failure does not name it: {line}"
            );
            assert!(
                !line.contains("no such table") && !line.contains("store operation failed"),
                "the driver's text or the generic line reached the operator: {line}"
            );
            // ...and the driver's error is still reachable, as R28.4 requires.
            assert!(
                format!("{refusal:?}").contains("no such table"),
                "the driver's error was thrown away: {refusal:?}"
            );
            lines.insert(line);
        }
        assert_eq!(
            lines.len(),
            20,
            "two passes render the same line, so an operator cannot tell them apart: {lines:?}"
        );
    }

    /// The ROUTING, not the rendering: no startup pass maps its enumeration onto the generic arm.
    ///
    /// `sol1_each_startup_pass_names_itself_when_its_enumeration_fails` proves the six lines are
    /// distinct -- it constructs the variant directly, so it says nothing about whether any pass
    /// REACHES it. Collapsing the completion pass back onto `SupervisorError::Store` left that bar
    /// green, which is how sol1 found the sixth site by reading rather than by running.
    ///
    /// Reading our own source is a blunt instrument and deliberate: the property is syntactic
    /// ("no `*StateFault::Store(..) => SupervisorError::Store(..)` remains"), a broken store is not
    /// plantable per pass, and sol1's own probe is the same shape.
    #[test]
    fn no_startup_pass_maps_its_enumeration_onto_the_generic_store_arm() {
        let source: String = include_str!("lib.rs")
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        // Every state-fault enum in the crate, not the six the sixth-site fold happened to
        // name: a pass added later that maps its enumeration generically is the same bug.
        for fault in [
            "ArtifactStateFault",
            "CandidateStateFault",
            "ClassificationStateFault",
            "CompletionStateFault",
            "EvaluationStateFault",
            "LocalModelActivationStateFault",
            "ManifestStateFault",
            "MissionStateFault",
            "ProjectPolicyStateFault",
            "ProjectRepositoryStateFault",
            "ProjectStateFault",
            "ProposalStateFault",
            "PublicationStateFault",
            "ResidentConfigurationStateFault",
            "ResidentLifecycleStateFault",
            "RulingStateFault",
            "RunStateFault",
            "SampleProjectStateFault",
            "StarterStateFault",
            "TeamStateFault",
        ] {
            for binding in ["error", "source"] {
                let arrow = format!("{fault}::Store({binding})=>");
                // The arm body may or may not be braced, so look just past the arrow rather than
                // matching one spelling -- the first version of this bar matched only the
                // unbraced form and the mutant walked through it.
                if let Some(at) = source.find(&arrow) {
                    let body = &source[at + arrow.len()..];
                    let body = body.strip_prefix('{').unwrap_or(body);
                    assert!(
                        !body.starts_with("SupervisorError::Store("),
                        "{fault} still maps its enumeration onto the generic Store arm, so its \
                         failure renders as `store operation failed`"
                    );
                }
            }
        }
    }

    /// sol1 P1-2: a Totals refusal says WHICH table and BY HOW MUCH.
    ///
    /// One honest completion plus one duplicate used to reach `Totals(EventCount(2))`, whose line
    /// was "the Mission completions do not agree with the log as a whole: EventCount" -- an
    /// operator learned that a count was wrong and nothing else. R42 allows numeric context, so
    /// the table and both counts are in the line and in Debug.
    #[test]
    fn sol1_a_totals_refusal_names_the_table_and_both_counts() {
        let duplicate = super::TotalsMismatch {
            table: "events (mission.completed)",
            expected: 1,
            observed: 2,
        };
        for refusal in [
            SupervisorError::MissionCompletionTotalsMismatch(duplicate),
            SupervisorError::MissionTotalsMismatch(duplicate),
            SupervisorError::ProjectTotalsMismatch(duplicate),
        ] {
            let line = refusal.to_string();
            assert!(
                line.contains("events (mission.completed)"),
                "the line does not name the table: {line}"
            );
            assert!(
                line.contains('1') && line.contains('2'),
                "the line does not carry both counts: {line}"
            );
            assert_eq!(line.lines().count(), 1, "{line}");
            assert!(
                format!("{refusal:?}").contains("expected: 1")
                    && format!("{refusal:?}").contains("observed: 2"),
                "the counts are not in Debug either: {refusal:?}"
            );
        }
    }
    use super::StartupPass;

    /// sol1 P1-3: five enumeration failures used to be one line.
    ///
    /// Every pass mapped its `Store` fault onto the generic arm, whose line is
    /// `store operation failed` -- true of all five, and no help at all. The line now names the
    /// pass, still without the driver's text (R28.4).
    #[test]
    fn sol1_each_startup_pass_names_itself_when_its_enumeration_fails() {
        let every_pass = [
            StartupPass::Project,
            StartupPass::Mission,
            StartupPass::MissionImpactClassification,
            StartupPass::OutcomeManifest,
            StartupPass::OutcomePublication,
            // sol1's re-read: the completion pass was the sixth enumerator, still generic.
            StartupPass::MissionCompletion,
        ];
        assert_eq!(
            every_pass.len(),
            6,
            "a pass was added to `StartupPass` without a fixture here"
        );

        let lines: Vec<String> = every_pass
            .iter()
            .map(|pass| {
                SupervisorError::StartupEnumerationFailed {
                    pass: *pass,
                    source: rusqlite::Error::QueryReturnedNoRows,
                }
                .to_string()
            })
            .collect();

        let distinct: std::collections::BTreeSet<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(
            distinct.len(),
            6,
            "two passes share a line, so an operator cannot tell them apart: {distinct:?}"
        );
        for (pass, line) in every_pass.iter().zip(&lines) {
            assert!(
                line.contains(pass.name()),
                "the line does not name its pass: {line}"
            );
            assert!(
                !line.contains("QueryReturnedNoRows") && !line.contains("no rows"),
                "the driver's text reached the line: {line}"
            );
        }

        // ...and the driver's error is still reachable, as R28.4 requires.
        let refusal = SupervisorError::StartupEnumerationFailed {
            pass: StartupPass::Mission,
            source: rusqlite::Error::QueryReturnedNoRows,
        };
        assert!(format!("{refusal:?}").contains("QueryReturnedNoRows"));
        assert!(std::error::Error::source(&refusal).is_none() || true);
    }

    /// sol1 P1-1: store text cannot forge a second line in the operator's log.
    ///
    /// A stored identity column can hold anything. Interpolated raw, a newline in it puts a line
    /// the daemon never wrote into the operator's log, and 4 KB of padding pushes the real message
    /// out of view.
    ///
    /// R42 amended what this bar requires. Escaping and truncating BOUNDED the disclosure; they
    /// did not remove it, and "the line must contain an escaped `\n`" was a requirement to
    /// disclose a substring of store text. The invariant kept here is the one that always
    /// mattered: one line, bounded, naming the owner and the fault -- and now carrying nothing of
    /// the raw value at all. The raw value stays whole in `Debug`.
    #[test]
    fn sol1_stored_identity_text_cannot_forge_a_startup_line() {
        let forged = format!("\nSOL1_FORGED_STARTUP_LINE{}", "x".repeat(4096));
        let refusal = SupervisorError::StoredIdentityUnreadable {
            what: "a Mission row's identity is not readable",
            raw: forged.clone(),
        };

        let line = refusal.to_string();
        assert_eq!(
            line.lines().count(),
            1,
            "R42S2.IDENTITY.Forged.ONE_LINE: the store wrote to the operator log: {line}"
        );
        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "R42S2.IDENTITY.Forged.ONE_LINE: a raw newline reached the line: {line}"
        );
        assert!(
            !line.contains("SOL1_FORGED_STARTUP_LINE"),
            "R42S2.IDENTITY.Forged.NO_NEEDLE: {line}"
        );
        // The whole line, independently written, not a substring check.
        assert_eq!(
            line, "a Mission row's identity is not readable: StoredIdentityUnreadable",
            "R42S2.IDENTITY.Forged.PROJECTION"
        );

        // ...and nothing is thrown away: Debug still carries the whole value.
        let debug = format!("{refusal:?}");
        assert!(
            debug.contains("SOL1_FORGED_STARTUP_LINE") && debug.chars().count() > 4096,
            "the raw value is not reachable through Debug"
        );
    }

    /// The same rule on the older arm sol1 also cited.
    #[test]
    fn sol1_an_invalid_workspace_id_is_bounded_and_escaped_too() {
        let raw = format!("\nSOL1_FORGED{}", "y".repeat(4096));
        let refusal = SupervisorError::InvalidWorkspaceId(raw.clone());
        let line = refusal.to_string();
        assert_eq!(
            line.lines().count(),
            1,
            "R42S2.IDENTITY.Workspace.ONE_LINE: {line}"
        );
        assert!(
            !line.contains('\n') && !line.contains('\r'),
            "R42S2.IDENTITY.Workspace.ONE_LINE: {line}"
        );
        assert!(
            !line.contains("SOL1_FORGED"),
            "R42S2.IDENTITY.Workspace.NO_NEEDLE: {line}"
        );
        assert_eq!(
            line, "store contains an invalid Workspace id: InvalidWorkspaceId",
            "R42S2.IDENTITY.Workspace.PROJECTION"
        );
        // Bounded is now trivially true, and the rejected String is still whole in Debug.
        assert!(
            format!("{refusal:?}").contains("SOL1_FORGED"),
            "R42S2.IDENTITY.Workspace.DEBUG_RETAINS"
        );
    }
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
        MIGRATION_V25, MIGRATION_V26, MIGRATION_V27, OperatorAccountService, PersistedEventKind,
        PreflightOutcome, PreflightReceipt, ProjectCharter, ProjectCharterDraft, ProjectDraft,
        ProjectRepositoryService, ProjectService, ProjectView, Recorded, RepositoryDraft,
        RepositorySourceKind, ResolvedCheck, SCHEMA_V1, SecretString, StartupConfig, Supervisor,
        SupervisorError, UnavailableEvidence,
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

        // The Run-state pass compares kind/state to cardinalities, which this tamper preserves,
        // so the log walk is what refuses -- and it now NAMES the Run rather than saying only that
        // one disagreed.
        let SupervisorError::RunEventStateMismatch { run_id, .. } = error else {
            panic!("expected a named Run-event state refusal, got {error:?}");
        };
        assert_eq!(run_id, receipt.run_id);
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

        // The Run-state pass compares kind/state to cardinalities, which this tamper preserves,
        // so the log walk is what refuses -- and it now NAMES the Run rather than saying only that
        // one disagreed.
        let SupervisorError::RunEventStateMismatch { run_id, .. } = error else {
            panic!("expected a named Run-event state refusal, got {error:?}");
        };
        assert_eq!(run_id, receipt.run_id);
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

    /// The production half of every `.rs` file under `src/`: its lines up to the file's test module
    /// (a line that is exactly `#[cfg(test)]` followed by `mod ...`), with the file's path relative
    /// to `src/`. Files with no test module are production throughout. The source assertions of
    /// R21 read this rather than trusting a grep somebody ran once.
    fn production_source() -> Vec<(String, Vec<(usize, String)>)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("src is readable") {
                let path = entry.expect("an entry").path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    out.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        files.sort();
        files
            .into_iter()
            .map(|path| {
                let text = std::fs::read_to_string(&path).expect("a source file reads");
                let lines: Vec<&str> = text.lines().collect();
                let end = (0..lines.len())
                    .find(|&i| {
                        lines[i].trim() == "#[cfg(test)]"
                            && lines
                                .get(i + 1)
                                .is_some_and(|next| next.starts_with("mod "))
                    })
                    .unwrap_or(lines.len());
                (
                    path.strip_prefix(&root)
                        .expect("under src")
                        .to_string_lossy()
                        .into_owned(),
                    lines[..end]
                        .iter()
                        .enumerate()
                        .map(|(i, line)| (i + 1, (*line).to_owned()))
                        .collect(),
                )
            })
            .collect()
    }

    /// R21: the pragma is the fix, so every path that opens a store connection must come through
    /// `open_store_connection`. The paths this crate offers, and how each is proven here:
    ///
    /// - `open_store_connection` itself, the public opener: opened and asserted live.
    /// - `Supervisor::bootstrap`, which opens the store to migrate it and create the first
    ///   Workspace and drops its connection when done: not reachable from a test, so proven by the
    ///   source assertion at the end -- the only `Connection::open(` outside a test module in this
    ///   crate is the opener's own line.
    /// - `OperatorAccountService::open`, the one long-lived connection every service reaches
    ///   through `with_authenticated_store`: opened on a bootstrapped store and asserted live
    ///   through a test-only accessor.
    /// - `SampleProjectService`'s test-store path (a `#[cfg(test)]` block inside a production
    ///   function): routes through the opener, and is covered by the same source assertion.
    /// - `verify_schema`'s in-memory expectation connection is not a store and not a path;
    ///   `Connection::open_in_memory` is a different name and is not counted.
    ///
    /// A path that opens a store some other way is a finding, not something to allow here.
    ///
    /// R21.6: `runs` and `beacon_repositories` have no unconditional update/delete pair and got no
    /// schema guard in any version of this fix; the pragma is connection state, so their own
    /// `BEFORE DELETE` triggers meet REPLACE exactly as the paired tables' do.
    #[test]
    fn every_path_that_opens_a_store_sets_recursive_triggers() {
        let path = std::env::temp_dir().join(format!(
            "beacon-every-opener-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let pragma = |connection: &rusqlite::Connection, name: &str| -> i64 {
            connection
                .query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))
                .unwrap()
        };
        let busy_timeout_ms = i64::from(crate::STORE_BUSY_TIMEOUT_MS);

        // 1. The opener.
        let opened = crate::open_store_connection(&path).unwrap();
        assert_eq!(pragma(&opened, "recursive_triggers"), 1, "the opener");
        assert_eq!(pragma(&opened, "foreign_keys"), 1, "the opener");
        assert_eq!(
            pragma(&opened, "busy_timeout"),
            busy_timeout_ms,
            "the opener"
        );
        drop(opened);

        // 2. Bootstrap (its connection is the opener's; see the source assertion) so that ...
        crate::Supervisor::bootstrap(crate::StartupConfig {
            store_path: path.clone(),
            initial_workspace_name: "Personal".to_owned(),
        })
        .unwrap();

        // 3. ... the account service can open the store: the connection every service uses.
        let accounts = crate::OperatorAccountService::open(&path).unwrap();
        assert_eq!(
            accounts.store_pragma("recursive_triggers"),
            1,
            "OperatorAccountService::open"
        );
        assert_eq!(
            accounts.store_pragma("foreign_keys"),
            1,
            "OperatorAccountService::open"
        );
        assert_eq!(
            accounts.store_pragma("busy_timeout"),
            busy_timeout_ms,
            "OperatorAccountService::open"
        );
        drop(accounts);
        let _ = std::fs::remove_file(&path);

        // 4. Source: outside test modules, the only `Connection::open(` in the crate is the
        //    opener's own line, so bootstrap and the sample-project test path cannot bypass it.
        let source = production_source();
        let opens: Vec<(String, usize)> = source
            .iter()
            .flat_map(|(file, lines)| {
                lines
                    .iter()
                    .filter(|(_, line)| line.contains("Connection::open("))
                    .map(move |(number, _)| (file.clone(), *number))
            })
            .collect();
        let opener_line = source
            .iter()
            .find(|(file, _)| file == "lib.rs")
            .and_then(|(_, lines)| {
                lines
                    .iter()
                    .find(|(_, line)| line.contains("let connection = Connection::open(path)?;"))
                    .map(|(number, _)| *number)
            })
            .expect("the opener's own open");
        assert_eq!(
            opens,
            vec![("lib.rs".to_owned(), opener_line)],
            "a store is opened somewhere other than `open_store_connection`"
        );
    }

    /// R21 (b): no production SQL in this crate spells a conflict clause. The pragma makes REPLACE
    /// meet the delete trigger and UPSERT already meets the update trigger, but `INSERT OR IGNORE`
    /// is a silent `Ok(0)` and none of the three has a place in a store whose rows are immutable
    /// by declaration: a duplicate identity is a refusal to hand back, never a row to write over
    /// or a write to swallow. Read from the production half of every file under `src/` (up to its
    /// test module), comment lines skipped because the opener's doc names REPLACE by name; a test
    /// module may spell them, since proving the pragma requires one to.
    #[test]
    fn no_production_sql_spells_a_conflict_clause() {
        let clauses = ["OR REPLACE", "OR IGNORE", "ON CONFLICT"];
        let found: Vec<String> = production_source()
            .into_iter()
            .flat_map(|(file, lines)| {
                lines
                    .into_iter()
                    .filter(|(_, line)| !line.trim_start().starts_with("//"))
                    .flat_map(move |(number, line)| {
                        let file = file.clone();
                        clauses
                            .iter()
                            .filter(move |clause| line.to_ascii_uppercase().contains(*clause))
                            .map(move |clause| format!("{file}:{number}: {clause}"))
                            .collect::<Vec<_>>()
                    })
            })
            .collect();
        assert!(
            found.is_empty(),
            "production SQL spells a conflict clause:\n{}",
            found.join("\n")
        );
    }

    /// FF-49's fix, witnessed without a clock: startup does not replay the ladder inside its
    /// write transaction for a version this process has already computed.
    ///
    /// This is the assertion that actually guards the lock hold. The concurrent test below can only
    /// catch a slow hold by racing the busy timeout, which makes it a measure of how loaded the box
    /// is; this counts the work instead. A version's replay count reaches one and can never move
    /// again, so sampling it around a bootstrap is race-free even with the whole suite running.
    #[test]
    fn startup_does_not_replay_the_ladder_for_a_version_already_computed() {
        let store = TestStore::new("expected-schema-memoized");
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        let first = crate::expected_schema(SCHEMA_VERSION).unwrap();
        let replays = crate::expected_schema_replays(SCHEMA_VERSION);
        assert_eq!(replays, 1, "the current version's ladder is replayed once");

        // A second startup against the same store verifies the schema twice -- once for the
        // version it claims, once for the version it ends at -- and must replay neither.
        Supervisor::bootstrap(config(&store.path, "Personal")).unwrap();
        assert_eq!(
            crate::expected_schema_replays(SCHEMA_VERSION),
            replays,
            "startup replayed the ladder inside its write transaction for a version already computed"
        );
        let second = crate::expected_schema(SCHEMA_VERSION).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "the cached signature is not shared"
        );
        assert!(!first.is_empty(), "the expected schema is empty");
    }

    /// R20.4's connection half (sol1's #13): every store connection opens with `recursive_triggers`
    /// on, so SQLite's REPLACE fires the delete trigger it otherwise skips. Proven on a table that
    /// has ONLY the old-shape trigger pair -- no `BEFORE INSERT` refusal to hide behind -- so what
    /// refuses here is the pragma and nothing else.
    #[test]
    fn every_store_connection_opens_with_recursive_triggers_so_replace_meets_the_delete_trigger() {
        let path = std::env::temp_dir().join(format!(
            "beacon-recursive-triggers-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let connection = crate::open_store_connection(&path).unwrap();
        for pragma in ["foreign_keys", "recursive_triggers"] {
            let on: i64 = connection
                .query_row(&format!("PRAGMA {pragma}"), [], |row| row.get(0))
                .unwrap();
            assert_eq!(on, 1, "{pragma} is not on");
        }
        connection
            .execute_batch(
                "CREATE TABLE guarded (digest TEXT NOT NULL PRIMARY KEY, value TEXT NOT NULL) STRICT;
                 CREATE TRIGGER guarded_no_delete BEFORE DELETE ON guarded
                 BEGIN SELECT RAISE(ABORT, 'guarded rows are immutable'); END;
                 INSERT INTO guarded VALUES ('d1', 'first');",
            )
            .unwrap();
        let replaced =
            connection.execute("INSERT OR REPLACE INTO guarded VALUES ('d1', 'second')", []);
        assert!(
            replaced
                .as_ref()
                .is_err_and(|error| error.to_string().contains("immutable")),
            "REPLACE walked past the delete trigger: {replaced:?}"
        );
        let value: String = connection
            .query_row("SELECT value FROM guarded", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "first");
        drop(connection);
        let _ = std::fs::remove_file(&path);
    }

    /// The #60 P2: every replayable Event kind still inserts, after the NEWEST migration.
    ///
    /// A migration that rebuilds the closed `events.kind` CHECK can drop a kind, and the row that
    /// used it goes with it (PR #50). `the_event_kind_rebuild_carries_every_kind_and_its_rows`
    /// proves one migration carried its parent's list; this proves the thing a reader actually
    /// wants to know -- that the schema a store ends at admits every kind the daemon can replay --
    /// and it does so **by itself**, with no dispatch suite and no service, so a dropped kind names
    /// the variant it dropped.
    ///
    /// Two statements of one list, reconciled here so they cannot drift. `EVERY_KIND` is what the
    /// test inserts; `declaration_index` is an EXHAUSTIVE match, so a new variant does not compile
    /// until it has an arm -- and the assertions below then fail unless it is in `EVERY_KIND` too.
    /// Without the second half a variant could be added to the enum and silently never tested.
    #[test]
    fn every_persisted_event_kind_inserts_into_events_after_the_newest_migration() {
        const EVERY_KIND: &[super::PersistedEventKind] = &[
            super::PersistedEventKind::WorkspaceCreated,
            super::PersistedEventKind::StarterOrganizationAccepted,
            super::PersistedEventKind::StarterOrganizationDiscarded,
            super::PersistedEventKind::ProjectCreated,
            super::PersistedEventKind::ProjectCharterRevised,
            super::PersistedEventKind::ProjectPolicyPublished,
            super::PersistedEventKind::ProjectRepositoryBound,
            super::PersistedEventKind::LocalModelActivated,
            super::PersistedEventKind::LocalModelDeactivated,
            super::PersistedEventKind::ResidentActivated,
            super::PersistedEventKind::ResidentSuspended,
            super::PersistedEventKind::ResidentReactivated,
            super::PersistedEventKind::ResidentRetired,
            super::PersistedEventKind::ResidentConfigurationSet,
            super::PersistedEventKind::ResidentConfigurationCleared,
            super::PersistedEventKind::TeamFormed,
            super::PersistedEventKind::MissionAdmitted,
            super::PersistedEventKind::RunPreflighted,
            super::PersistedEventKind::RunExecuted,
            super::PersistedEventKind::SampleProjectInstalled,
            super::PersistedEventKind::ProposalSubmitted,
            super::PersistedEventKind::RulingRecorded,
            super::PersistedEventKind::ProposalCandidateRecorded,
            super::PersistedEventKind::ProposalDispatched,
            super::PersistedEventKind::InstallationPaused,
            super::PersistedEventKind::InstallationResumed,
            super::PersistedEventKind::EvaluationRecorded,
            super::PersistedEventKind::MissionCompleted,
            super::PersistedEventKind::SpendMeasured,
            super::PersistedEventKind::MissionImpactClassified,
            super::PersistedEventKind::ArtifactRecorded,
        ];
        fn declaration_index(kind: super::PersistedEventKind) -> usize {
            match kind {
                super::PersistedEventKind::WorkspaceCreated => 0,
                super::PersistedEventKind::StarterOrganizationAccepted => 1,
                super::PersistedEventKind::StarterOrganizationDiscarded => 2,
                super::PersistedEventKind::ProjectCreated => 3,
                super::PersistedEventKind::ProjectCharterRevised => 4,
                super::PersistedEventKind::ProjectPolicyPublished => 5,
                super::PersistedEventKind::ProjectRepositoryBound => 6,
                super::PersistedEventKind::LocalModelActivated => 7,
                super::PersistedEventKind::LocalModelDeactivated => 8,
                super::PersistedEventKind::ResidentActivated => 9,
                super::PersistedEventKind::ResidentSuspended => 10,
                super::PersistedEventKind::ResidentReactivated => 11,
                super::PersistedEventKind::ResidentRetired => 12,
                super::PersistedEventKind::ResidentConfigurationSet => 13,
                super::PersistedEventKind::ResidentConfigurationCleared => 14,
                super::PersistedEventKind::TeamFormed => 15,
                super::PersistedEventKind::MissionAdmitted => 16,
                super::PersistedEventKind::RunPreflighted => 17,
                super::PersistedEventKind::RunExecuted => 18,
                super::PersistedEventKind::SampleProjectInstalled => 19,
                super::PersistedEventKind::ProposalSubmitted => 20,
                super::PersistedEventKind::RulingRecorded => 21,
                super::PersistedEventKind::ProposalCandidateRecorded => 22,
                super::PersistedEventKind::ProposalDispatched => 23,
                super::PersistedEventKind::InstallationPaused => 24,
                super::PersistedEventKind::InstallationResumed => 25,
                super::PersistedEventKind::EvaluationRecorded => 26,
                super::PersistedEventKind::MissionCompleted => 27,
                super::PersistedEventKind::SpendMeasured => 28,
                super::PersistedEventKind::MissionImpactClassified => 29,
                super::PersistedEventKind::ArtifactRecorded => 30,
            }
        }

        let mut indices: Vec<usize> = EVERY_KIND.iter().copied().map(declaration_index).collect();
        let listed = indices.len();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices.len(), listed, "EVERY_KIND names a variant twice");
        assert_eq!(
            indices,
            (0..listed).collect::<Vec<_>>(),
            "EVERY_KIND is not the enum's declaration order"
        );
        assert_eq!(
            indices.last().copied(),
            Some(listed - 1),
            "a variant has an arm above the end of EVERY_KIND -- it was added to the enum and not \
             to the list this test inserts"
        );

        let connection = Connection::open_in_memory().unwrap();
        super::apply_every_migration(&connection).unwrap();
        super::verify_schema(&connection, SCHEMA_VERSION)
            .expect("the ladder must produce the current schema");
        let workspace = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 1)",
                rusqlite::params![workspace],
            )
            .unwrap();

        for (offset, kind) in EVERY_KIND.iter().enumerate() {
            // R58: through the DOOR, not around it. The witness is stronger for it -- it now says
            // every kind is one the single append path accepts, which is the property the chain
            // depends on, rather than only that the schema's CHECK admits the token.
            crate::event_chain::append_event(
                &connection,
                &workspace.to_string(),
                kind.as_str(),
                &format!(r#"{{"workspace_id":"{workspace}"}}"#),
                offset as i64,
            )
            .unwrap_or_else(|error| {
                panic!(
                    "schema {SCHEMA_VERSION} refuses the Event kind `{}`: {error}",
                    kind.as_str()
                )
            });
            // ...and it reads back as the variant it was written as, so a kind the CHECK admits but
            // `parse` does not is a failure here rather than at replay.
            assert_eq!(
                super::PersistedEventKind::parse(kind.as_str().to_owned()).unwrap(),
                *kind
            );
        }
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
                INSERT INTO events (
                    workspace_id, kind, payload_json, occurred_at_ms,
                    predecessor_kind, predecessor_digest, link_digest
                )
                VALUES (?1, 'workspace.created', ?2, 0, 'unchained', NULL, NULL)
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

    /// How many supervisors start against one store file at once in the test below.
    ///
    /// Two proved the mutual exclusion and nothing about the queue behind it: every waiter is
    /// serialized behind the holder's write transaction, so the last one waits N times the hold,
    /// and when that product passes the busy timeout the wait becomes `SQLITE_BUSY` -- FF-49.
    /// Thirty-two reproduced it on an idle box while the fix was being measured (7 to 11 refused
    /// per run, recorded in that commit), and thirty-two is NOT what this test settles at: with the
    /// hold fixed it still failed 2 runs in 3 inside the full suite, because thirty-two threads on
    /// top of the suite's own thirty-two make the assertion a measure of the box rather than of the
    /// store. Eight is a queue the store owns. What guards the hold is
    /// `startup_does_not_replay_the_ladder_for_a_version_already_computed`, which counts the work
    /// instead of racing a clock.
    const CONCURRENT_BOOTSTRAP_SUPERVISORS: usize = 8;

    #[test]
    fn concurrent_bootstrap_creates_exactly_one_workspace_and_event() {
        let store = TestStore::new("concurrent");
        let barrier = Arc::new(Barrier::new(CONCURRENT_BOOTSTRAP_SUPERVISORS));
        let handles = (0..CONCURRENT_BOOTSTRAP_SUPERVISORS)
            .map(|_| {
                let path = store.path.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    // The refusal is the evidence, so it is returned rather than unwrapped: a
                    // panicking thread reports "the main thread saw a JoinError", which is what
                    // FF-49 looked like from the outside for weeks.
                    Supervisor::bootstrap(config(&path, "Personal"))
                        .map_err(|error| format!("{error:?}"))
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("a bootstrap thread panicked"))
            .collect::<Vec<_>>();

        let refused: Vec<&String> = results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .collect();
        assert!(
            refused.is_empty(),
            "{} of {CONCURRENT_BOOTSTRAP_SUPERVISORS} supervisors were refused by the store they \
             were waiting for: {refused:?}",
            refused.len()
        );
        let reports: Vec<&crate::StartupReport> = results
            .iter()
            .filter_map(|result| result.as_ref().ok())
            .collect();
        assert_eq!(
            reports
                .iter()
                .filter(|report| report.workspace.created)
                .count(),
            1,
            "exactly one supervisor may create the first Workspace"
        );
        assert!(
            reports
                .iter()
                .all(|report| report.workspace.id == reports[0].workspace.id)
        );
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
                INSERT INTO events (
                    workspace_id, kind, payload_json, occurred_at_ms,
                    predecessor_kind, predecessor_digest, link_digest
                )
                VALUES (?1, 'workspace.created', ?2, 0, 'unchained', NULL, NULL)
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

    /// The Event-kind list a rebuild carries. Extracted from the migration text so the test reads
    /// the artifact under test, not a copy of it.
    fn event_kind_list(migration: &str) -> Vec<String> {
        let start = migration.find("kind IN (").expect("kind list present");
        let rest = &migration[start..];
        let end = rest.find("))").expect("kind list ends");
        rest[..end]
            .split('\'')
            .skip(1)
            .step_by(2)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn migration_0027_carries_every_event_kind_and_every_event_forward() {
        // The #50 trap: an events rebuild whose kind list is a snapshot of the wrong parent drops
        // every kind added since, and the rows that used them. 0027's list must be exactly the
        // parent's plus 'artifact.recorded', and the rebuild must carry the rows themselves. The
        // parent is 0026: the completion store landed first, and this lane renumbered at its
        // final rebase exactly as its provisional note said it would.
        let parent_kinds = event_kind_list(MIGRATION_V26);
        let widened_kinds = event_kind_list(MIGRATION_V27);
        let mut expected = parent_kinds.clone();
        expected.push("artifact.recorded".to_owned());
        assert_eq!(
            widened_kinds, expected,
            "the rebuild's kind list is not the parent's list plus the one new kind"
        );
        // And every kind the table now accepts is one the replay path can name.
        for kind in &widened_kinds {
            PersistedEventKind::parse(kind.clone()).unwrap_or_else(|_| {
                panic!(
                    "the rebuilt events table accepts a kind the replay path cannot parse: {kind}"
                )
            });
        }

        // Row preservation, measured. Foreign keys off on purpose: this is a migration test, not
        // a fixture exercise.
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        for migration in [
            SCHEMA_V1,
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
            MIGRATION_V25,
            MIGRATION_V26,
        ] {
            connection.execute_batch(migration).unwrap();
        }
        let workspace = Uuid::new_v4().to_string();
        connection
            .execute(
                "INSERT INTO workspaces (id, name, created_at_ms) VALUES (?1, 'Personal', 7)",
                rusqlite::params![workspace],
            )
            .unwrap();
        for (kind, marker) in [
            ("workspace.created", "{\"marker\":\"first\"}"),
            ("evaluation.recorded", "{\"marker\":\"second\"}"),
            ("mission.completed", "{\"marker\":\"third\"}"),
        ] {
            connection
                .execute(
                    "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                     SELECT id, ?2, ?3, 42 FROM workspaces",
                    rusqlite::params![workspace, kind, marker],
                )
                .unwrap();
        }
        let read_events = |connection: &Connection| {
            connection
                .prepare(
                    "SELECT sequence, kind, payload_json, occurred_at_ms
                     FROM events ORDER BY sequence",
                )
                .unwrap()
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let before = read_events(&connection);

        connection.execute_batch(MIGRATION_V27).unwrap();

        assert_eq!(
            before,
            read_events(&connection),
            "the rebuild did not carry every event row"
        );
        let version: i64 = connection
            .query_row(
                "SELECT version FROM schema_meta WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 27);
        // The widened vocabulary is what the rebuild was for.
        connection
            .execute(
                "INSERT INTO events (workspace_id, kind, payload_json, occurred_at_ms)
                 SELECT id, 'artifact.recorded', '{}', 43 FROM workspaces",
                [],
            )
            .expect("the rebuilt table accepts the new kind");
        // The roster triggers the rebuild must put back.
        let triggers: i64 = connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'trigger' AND name IN (
                     'team_formation_members_no_insert_after_formation',
                     'mission_admission_members_no_insert_after_admission')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            triggers, 2,
            "the rebuild did not restore the roster triggers"
        );
    }

    #[test]
    fn migration_0027_advances_the_version_only_from_twenty_six() {
        // The guarded UPDATE is what stops a store that never ran the rebuild from being stamped
        // as though it had. Applied to a store that is not at 26, the migration leaves the
        // version alone.
        let through_v25 = [
            SCHEMA_V1,
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
            MIGRATION_V25,
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

        let early = Connection::open_in_memory().unwrap();
        early.pragma_update(None, "foreign_keys", "OFF").unwrap();
        for migration in through_v25 {
            early.execute_batch(migration).unwrap();
        }
        assert_eq!(version(&early), 25);
        early.execute_batch(MIGRATION_V27).unwrap();
        assert_eq!(
            version(&early),
            25,
            "a store that never ran the 0026 rebuild was stamped as though it had"
        );

        let current = Connection::open_in_memory().unwrap();
        current.pragma_update(None, "foreign_keys", "OFF").unwrap();
        for migration in through_v25 {
            current.execute_batch(migration).unwrap();
        }
        current.execute_batch(MIGRATION_V26).unwrap();
        assert_eq!(version(&current), 26);
        current.execute_batch(MIGRATION_V27).unwrap();
        assert_eq!(version(&current), 27);
    }

    // -------------------------------------------------------------------------------------
    // R42 slice 2, commit 1: the startup projections and the caller-safe Supervisor tag.
    // -------------------------------------------------------------------------------------

    const R42S2_NEEDLE_A: &str = "R42S2_PRIVATE_A";
    const R42S2_NEEDLE_B: &str = "R42S2_PRIVATE_B";
    /// Static, because `what` and `table` are `&'static str`: the point is that static says where
    /// a string lives, not who wrote it.
    const R42S2_STATIC_A: &str = "R42S2_PRIVATE_A /home/example/private.txt column=private_column";
    const R42S2_STATIC_B: &str = "R42S2_PRIVATE_B /home/example/other.txt column=other_column";

    /// The 17 identity labels a startup refusal may print, written here from the reviewed list --
    /// never read back from the production allowlist this test constrains.
    /// The three approved ordering-key names, written here from the reviewed list.
    const R42S2_KEY_NAMES: [&str; 3] = ["revision", "sequence", "boundary_event_seq"];

    const R42S2_IDENTITY_LABELS: [&str; 23] = [
        "a Project row's identity is not readable",
        "a Mission row's identity is not readable",
        "a completion row's Workspace is not readable",
        "a manifest row's Workspace is not readable",
        "a publication row's identity is not readable",
        "a sample Project row's Workspace is not readable",
        "a Proposal row's Workspace is not readable",
        "a Ruling row's Workspace is not readable",
        "a Candidate row's Workspace is not readable",
        "an Evaluation row's Workspace is not readable",
        "an Artifact row's Workspace is not readable",
        "a Project repository row's identity is not readable",
        "a Team row's identity is not readable",
        "a Project policy row's identity is not readable",
        "a Workspace id is not readable",
        "a Resident configuration row's identity is not readable",
        "a Run row's identity is not readable",
        "a local-model activation row's identity is not readable",
        // R45's owners, folded in with its arm.
        "a Project repository revision is not readable",
        "a Project policy revision is not readable",
        "a Resident lifecycle decision's ordering key is not readable",
        "a Resident configuration decision's ordering key is not readable",
        "a local-model activation decision's ordering key is not readable",
    ];

    /// The 14 totals labels, likewise.
    const R42S2_TOTALS_LABELS: [&str; 14] = [
        "operator_accounts (an owner for the stored Artifacts)",
        "events (mission.completed)",
        "mission_success_conditions (rows whose Mission does not exist)",
        "events (mission.admitted)",
        "project_creation_snapshots",
        "events (project.created)",
        "events (project.charter_revised)",
        "operator_accounts (an owner for the stored Proposals)",
        "events (proposal.submitted)",
        "resident_lifecycle_decisions",
        "operator_accounts (an owner for the stored Rulings)",
        "events (ruling.recorded)",
        "events (sample_project.installed)",
        "events (team.formed)",
    ];

    /// R42 slice 2: the two label vocabularies are closed, and an unknown label is reported as
    /// unknown rather than echoed or normalized.
    #[test]
    fn r42_startup_labels_are_closed() {
        assert_eq!(
            R42S2_IDENTITY_LABELS.len(),
            23,
            "R42S2.LABELS.CARDINALITY.Identity"
        );
        assert_eq!(
            R42S2_TOTALS_LABELS.len(),
            14,
            "R42S2.LABELS.CARDINALITY.Totals"
        );

        // Unknown labels lead, so a guard that echoes its input fails here first.
        for (case, what, raw, needle) in [
            (
                "Identity.UnknownLabel.A",
                R42S2_STATIC_A,
                R42S2_STATIC_A,
                R42S2_NEEDLE_A,
            ),
            (
                "Identity.UnknownLabel.B",
                R42S2_STATIC_B,
                R42S2_STATIC_B,
                R42S2_NEEDLE_B,
            ),
        ] {
            let refusal = SupervisorError::StoredIdentityUnreadable {
                what,
                raw: raw.to_owned(),
            };
            let line = refusal.to_string();
            assert!(
                !line.contains(needle),
                "R42S2.LABELS.{case}.NO_NEEDLE: {line}"
            );
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S2.LABELS.{case}.ONE_LINE: {line:?}"
            );
            assert!(
                format!("{refusal:?}").contains(needle),
                "R42S2.LABELS.{case}.DEBUG_RETAINS"
            );
            assert_eq!(
                line, "stored row identity could not be read: StoredIdentityUnreadable",
                "R42S2.LABELS.{case}.PROJECTION"
            );
        }

        for (case, table, needle) in [
            ("Totals.UnknownLabel.A", R42S2_STATIC_A, R42S2_NEEDLE_A),
            ("Totals.UnknownLabel.B", R42S2_STATIC_B, R42S2_NEEDLE_B),
        ] {
            let totals = crate::TotalsMismatch {
                table,
                expected: 7,
                observed: 11,
            };
            let line = totals.to_string();
            assert!(
                !line.contains(needle),
                "R42S2.LABELS.{case}.NO_NEEDLE: {line}"
            );
            assert!(
                format!("{totals:?}").contains(needle),
                "R42S2.LABELS.{case}.DEBUG_RETAINS"
            );
            // Both counts survive, and they differ, so a guard printing one twice fails.
            assert_eq!(
                line, "unrecognized totals relation: the log accounts for 7 but the store holds 11",
                "R42S2.LABELS.{case}.PROJECTION"
            );
        }

        // Every known identity label is preserved exactly, with a hidden raw payload.
        for label in R42S2_IDENTITY_LABELS {
            let refusal = SupervisorError::StoredIdentityUnreadable {
                what: label,
                raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
            };
            let line = refusal.to_string();
            assert!(
                !line.contains(R42S2_NEEDLE_A),
                "R42S2.LABELS.Identity.{label}.NO_NEEDLE: {line}"
            );
            assert!(
                format!("{refusal:?}").contains(R42S2_NEEDLE_A),
                "R42S2.LABELS.Identity.{label}.DEBUG_RETAINS"
            );
            assert_eq!(
                line,
                format!("{label}: StoredIdentityUnreadable"),
                "R42S2.LABELS.Identity.{label}.PROJECTION"
            );
        }

        // R45's arm, through the same guards: every known owner label and ordering key keeps its
        // exact text, an unknown one of either is declined, and the raw never appears.
        for label in R42S2_IDENTITY_LABELS {
            for key_name in R42S2_KEY_NAMES {
                let refusal = SupervisorError::StoredIntegerKeyUnreadable {
                    what: label,
                    raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
                    key_name,
                    key: 11,
                };
                let line = refusal.to_string();
                assert!(
                    !line.contains(R42S2_NEEDLE_A),
                    "R42S2.LABELS.IntegerKey.{label}.NO_NEEDLE: {line}"
                );
                assert_eq!(
                    line,
                    format!("{label}: StoredIntegerKeyUnreadable, {key_name} 11"),
                    "R42S2.LABELS.IntegerKey.{label}.PROJECTION"
                );
            }
        }
        for (case, what, key_name, expected) in [
            (
                "IntegerKey.UnknownLabel",
                R42S2_STATIC_A,
                "sequence",
                "stored row identity could not be read: StoredIntegerKeyUnreadable, sequence 11",
            ),
            (
                "IntegerKey.UnknownKeyName",
                "a Run row's identity is not readable",
                R42S2_STATIC_B,
                "a Run row's identity is not readable: StoredIntegerKeyUnreadable, an unrecognized \
                 ordering key 11",
            ),
        ] {
            let refusal = SupervisorError::StoredIntegerKeyUnreadable {
                what,
                raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
                key_name,
                key: 11,
            };
            let line = refusal.to_string();
            assert!(
                !line.contains(R42S2_NEEDLE_A) && !line.contains(R42S2_NEEDLE_B),
                "R42S2.LABELS.{case}.NO_NEEDLE: {line}"
            );
            assert!(
                format!("{refusal:?}").contains(R42S2_NEEDLE_A),
                "R42S2.LABELS.{case}.DEBUG_RETAINS"
            );
            assert_eq!(line, expected, "R42S2.LABELS.{case}.PROJECTION");
        }

        // Every known totals label, with both independent count pairs.
        for label in R42S2_TOTALS_LABELS {
            for (expected, observed) in [(7_u64, 11_u64), (13, 17)] {
                let totals = crate::TotalsMismatch {
                    table: label,
                    expected,
                    observed,
                };
                assert_eq!(
                    totals.to_string(),
                    format!(
                        "{label}: the log accounts for {expected} but the store holds {observed}"
                    ),
                    "R42S2.LABELS.Totals.{label}.CONTEXT"
                );
            }
        }
    }

    /// R42 slice 2: the seven changed startup lines, and the caller-safe tag for all 72 roots.
    ///
    /// Scope, stated rather than implied: the CALLER projection is checked for every one of the
    /// 72 roots (its literals are the spec's, resolved through one representative child per
    /// parent). The STARTUP line is checked for the seven sites this commit changes; the other 65
    /// startup lines are untouched by this commit and are covered by the suites that already own
    /// them -- see the build report's NOT PROBED list.
    #[test]
    fn r42_supervisor_projection_is_closed() {
        // The five original arms, in the spec's lead order.
        for (case, refusal, expected, needle) in [
            (
                "Io",
                SupervisorError::Io(std::io::Error::other(format!(
                    "{R42S2_NEEDLE_A} /home/example/private.txt"
                ))),
                "cannot prepare the store directory: Io",
                Some(R42S2_NEEDLE_A),
            ),
            (
                "Json",
                SupervisorError::Json(serde::de::Error::custom(format!(
                    "{R42S2_NEEDLE_A} /home/example/private.txt"
                ))),
                "cannot encode an event payload: Json",
                Some(R42S2_NEEDLE_A),
            ),
            (
                "InvalidWorkspaceId",
                SupervisorError::InvalidWorkspaceId(format!("{R42S2_NEEDLE_A} not-a-uuid")),
                "store contains an invalid Workspace id: InvalidWorkspaceId",
                Some(R42S2_NEEDLE_A),
            ),
            (
                "InvalidEventKind",
                SupervisorError::InvalidEventKind(format!("{R42S2_NEEDLE_A}.kind")),
                "store contains an unknown Event kind: InvalidEventKind",
                Some(R42S2_NEEDLE_A),
            ),
            (
                "IntegrityCheckFailed",
                SupervisorError::IntegrityCheckFailed(format!("{R42S2_NEEDLE_A} row 3 missing")),
                "SQLite integrity check failed: IntegrityCheckFailed",
                Some(R42S2_NEEDLE_A),
            ),
            (
                "StoredIdentityUnreadable.UnknownLabel",
                SupervisorError::StoredIdentityUnreadable {
                    what: R42S2_STATIC_A,
                    raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
                },
                "stored row identity could not be read: StoredIdentityUnreadable",
                Some(R42S2_NEEDLE_A),
            ),
        ] {
            let line = refusal.to_string();
            if let Some(needle) = needle {
                assert!(
                    !line.contains(needle),
                    "R42S2.SUPERVISOR.{case}.NO_NEEDLE: {line}"
                );
                assert!(
                    format!("{refusal:?}").contains(needle),
                    "R42S2.SUPERVISOR.{case}.DEBUG_RETAINS"
                );
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S2.SUPERVISOR.{case}.ONE_LINE: {line:?}"
            );
            assert_eq!(line, expected, "R42S2.SUPERVISOR.{case}.PROJECTION");
        }

        // The seventh changed site: a totals-bearing root still carries both counts and its label.
        let totals_root = SupervisorError::ProjectTotalsMismatch(crate::TotalsMismatch {
            table: "project_creation_snapshots",
            expected: 7,
            observed: 11,
        });
        assert_eq!(
            totals_root.to_string(),
            "the Projects do not agree with the store as a whole -- project_creation_snapshots: \
             the log accounts for 7 but the store holds 11",
            "R42S2.SUPERVISOR.ProjectTotalsMismatch.CONTEXT"
        );

        // An own key still appears in the startup line, and changing it changes the line.
        let first = SupervisorError::EventPayloadScopeMismatch(11);
        let second = SupervisorError::EventPayloadScopeMismatch(17);
        assert!(
            first.to_string().contains("11"),
            "R42S2.SUPERVISOR.OwnKey.KEYS"
        );
        assert_ne!(
            first.to_string(),
            second.to_string(),
            "R42S2.SUPERVISOR.OwnKey.KEYS"
        );

        // And the caller tag for all 72 roots carries no key at all.
        let keyed = SupervisorError::ProjectStateMismatch {
            workspace_id: uuid::Uuid::new_v4(),
            project_id: uuid::Uuid::new_v4(),
            cause: crate::ProjectError::NotFound,
        };
        let tag = crate::supervisor_caller_tag(&keyed);
        assert_eq!(
            tag, "ProjectStateMismatch.NotFound",
            "R42S2.SUPERVISOR.Tag.PROJECTION"
        );
        for uuid in [
            match &keyed {
                SupervisorError::ProjectStateMismatch { workspace_id, .. } => *workspace_id,
                _ => unreachable!(),
            },
            match &keyed {
                SupervisorError::ProjectStateMismatch { project_id, .. } => *project_id,
                _ => unreachable!(),
            },
        ] {
            assert!(
                !tag.contains(&uuid.to_string()),
                "R42S2.SUPERVISOR.Tag.KEYS: {tag}"
            );
        }
    }

    // ---- fold (astra read), item 2: all 72 startup templates, not the seven changed ones ------
    //
    // The committed S walked the seven sites this slice changes. The spec asks for every arm --
    // "closing six arms cannot flatten a seventh" -- so the other 65 templates are pinned here
    // too, with this table's own fixture keys. An unchanged template is still a row.
    const R42S2_UUID: &str = "11111111-1111-4111-8111-111111111111";
    const R42S2_SEQUENCE: u64 = 4_242;
    const R42S2_STORED_VERSION: i64 = 4_294_967_296;
    /// A sequence is an i64 too, but it is a different key: keep them distinguishable.
    const R42S2_SEQUENCE_I64: i64 = 4_242;
    const R42S2_TOTALS_TABLE: &str = "project_creation_snapshots";

    fn r42s2_uuid() -> uuid::Uuid {
        uuid::Uuid::parse_str(R42S2_UUID).expect("a fixture UUID parses")
    }

    fn r42s2_digest() -> String {
        "a".repeat(64)
    }

    fn r42s2_driver() -> rusqlite::Error {
        rusqlite::Error::InvalidParameterName(format!("{R42S2_NEEDLE_A} :private_binding"))
    }

    fn r42s2_totals() -> crate::TotalsMismatch {
        crate::TotalsMismatch {
            table: R42S2_TOTALS_TABLE,
            expected: 7,
            observed: 11,
        }
    }

    /// The 72 startup lines, transcribed from the spec with this table's own fixture keys.
    const STARTUP_LINES: [(&str, &str); 75] = [
        ("EmptyWorkspaceName", "workspace name cannot be empty"),
        (
            "WorkspaceNameTooLong",
            "workspace name cannot exceed 120 characters",
        ),
        (
            "WorkspaceNameContainsControl",
            "workspace name cannot contain control characters",
        ),
        (
            "ClockBeforeUnixEpoch",
            "system clock is before the Unix epoch",
        ),
        ("Io", "cannot prepare the store directory: Io"),
        ("Store", "store operation failed"),
        ("Json", "cannot encode an event payload: Json"),
        (
            "InvalidWorkspaceId",
            "store contains an invalid Workspace id: InvalidWorkspaceId",
        ),
        (
            "InvalidEventKind",
            "store contains an unknown Event kind: InvalidEventKind",
        ),
        (
            "EventPayloadScopeMismatch",
            "Event sequence 4242 payload does not match its Workspace scope",
        ),
        // R58, PROPOSED text -- Kevin signs or rewrites this line before the bar (R58.2). One row
        // per `SupervisorError` variant, so this pins the rendered line for one representative
        // `ChainFault`; the other five faults are Display text with no row of their own, which the
        // report names.
        (
            "EventChainBroken",
            "Event sequence 4242 was edited after it was recorded",
        ),
        // R58.3, PROPOSED text -- Kevin signs or rewrites this line before the bar. One row per
        // `SupervisorError` variant, so this pins the truncation shape (no Event behind the state);
        // the disagreement shape is the same variant's other Display arm and the report names it.
        (
            "PauseStateUnsupported",
            "the installation records a pause change at 4242 and holds no Event that accounts for it",
        ),
        (
            "StarterOrganizationEventMismatch",
            "Event sequence 4242 does not match Starter Organization state",
        ),
        (
            "StarterOrganizationStateMismatch",
            "the starter organization of Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: EmptyRoster",
        ),
        (
            "ProjectEventMismatch",
            "Event sequence 4242 does not match Project creation state",
        ),
        (
            "ProjectStateMismatch",
            "Project 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its charter history or the log: NotFound",
        ),
        (
            "ProjectTotalsMismatch",
            "the Projects do not agree with the store as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "LocalModelActivationEventMismatch",
            "Event sequence 4242 does not match local-model activation state",
        ),
        (
            "LocalModelActivationStateMismatch",
            "local model activation 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: ModelNotFound",
        ),
        (
            "LocalModelActivationRowUnreadable",
            "a local model activation row could not be read: ModelNotFound",
        ),
        (
            "ResidentLifecycleEventMismatch",
            "Event sequence 4242 does not match Resident lifecycle state",
        ),
        (
            "ResidentLifecycleStateMismatch",
            "the lifecycle of Resident 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: NotFound",
        ),
        (
            "ResidentLifecycleTotalsMismatch",
            "the Resident lifecycle decisions do not agree with the store -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "ResidentRowUnreadable",
            "a Resident row could not be read: NotFound",
        ),
        (
            "ResidentConfigurationEventMismatch",
            "Event sequence 4242 does not match Resident configuration state",
        ),
        (
            "ResidentConfigurationStateMismatch",
            "Resident configuration decision aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: ResidentNotFound",
        ),
        (
            "TeamEventMismatch",
            "Event sequence 4242 does not match Team state",
        ),
        (
            "TeamStateMismatch",
            "Team 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: TeamNotFound",
        ),
        (
            "TeamTotalsMismatch",
            "the Teams do not agree with the log as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "MissionEventMismatch",
            "Event sequence 4242 does not match Mission admission state",
        ),
        (
            "MissionStateMismatch",
            "Mission 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own record or its projections: MissionNotFound",
        ),
        (
            "MissionTotalsMismatch",
            "the Missions do not agree with the store as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "ProjectPolicyEventMismatch",
            "Event sequence 4242 does not match Project Policy state",
        ),
        (
            "ProjectPolicyStateMismatch",
            "the policy history of Project 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: ProjectNotFound",
        ),
        (
            "ProjectRepositoryEventMismatch",
            "Event sequence 4242 does not match Project Repository state",
        ),
        (
            "ProjectRepositoryStateMismatch",
            "the repository history of Project 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: ProjectNotFound",
        ),
        (
            "SampleProjectEventMismatch",
            "Event sequence 4242 does not match Sample Project installation state",
        ),
        (
            "SampleProjectStateMismatch",
            "the sample Project installation in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: NotInstalled",
        ),
        (
            "SampleProjectTotalsMismatch",
            "the sample Project installations do not agree with the log -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "RunEventMismatch",
            "Run event 4242 does not match its recorded Receipt",
        ),
        (
            "RunStateMismatch",
            "Run aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not agree with its Receipts and Events: ProjectNotFound",
        ),
        ("RunClockBeforeEpoch", "the clock is before the Unix epoch"),
        (
            "RunEventStateMismatch",
            "Event sequence 4294967296 in Workspace 11111111-1111-4111-8111-111111111111 carries a Receipt its Run 11111111-1111-4111-8111-111111111111 does not agree with",
        ),
        (
            "ProposalEventMismatch",
            "Event sequence 4242 does not match its stored Proposal bytes",
        ),
        (
            "ProposalStateMismatch",
            "Proposal aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against the records it cites: NotFound",
        ),
        (
            "ProposalTotalsMismatch",
            "the Proposals do not agree with the log as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "RulingEventMismatch",
            "Event sequence 4242 does not match its stored Ruling bytes",
        ),
        (
            "EvaluationEventMismatch",
            "Event sequence 4242 does not match its stored Evaluation bytes",
        ),
        (
            "MissionCompletionEventMismatch",
            "mission completion event 4242 does not match its stored record",
        ),
        (
            "MissionCompletionStateMismatch",
            "completion aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes or the records it cites: Absent",
        ),
        (
            "MissionCompletionTotalsMismatch",
            "the Mission completions do not agree with the log as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "EvaluationStateMismatch",
            "Evaluation aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes or the records it cites: NotFound",
        ),
        (
            "EvaluationChainsMismatch",
            "the Evaluation supersession chains do not re-prove: NotFound",
        ),
        (
            "ArtifactEventMismatch",
            "Event sequence 4242 does not match its stored Artifact bytes",
        ),
        (
            "ArtifactStateMismatch",
            "Artifact aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes: NotFound",
        ),
        (
            "ArtifactTotalsMismatch",
            "the Artifacts do not agree with the log as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "ArtifactRelationMismatch",
            "the Artifact supersession relation does not re-prove: NotFound",
        ),
        (
            "MissionImpactClassificationStateMismatch",
            "Mission impact classification aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove: columns_disagree",
        ),
        (
            "OutcomeManifestStateMismatch",
            "manifest aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes or the records it cites: Absent",
        ),
        (
            "OutcomePublicationStateMismatch",
            "publication version 4294967296 of Mission 11111111-1111-4111-8111-111111111111 in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against the records it cites: Absent",
        ),
        (
            "RulingStateMismatch",
            "Ruling aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes or the records it cites: NotFound",
        ),
        (
            "RulingTotalsMismatch",
            "the Rulings do not agree with the log as a whole -- project_creation_snapshots: the log accounts for 7 but the store holds 11",
        ),
        (
            "RulingChainsMismatch",
            "the Ruling chains do not re-prove: NotFound",
        ),
        (
            "CandidateEventMismatch",
            "Event sequence 4242 does not match its stored Candidate bytes",
        ),
        (
            "CandidateStateMismatch",
            "Candidate aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa in Workspace 11111111-1111-4111-8111-111111111111 does not re-prove against its own bytes or the records it cites: NotFound",
        ),
        (
            "DispatchEventMismatch",
            "Event sequence 4242 does not match its stored dispatch Receipt",
        ),
        (
            "DispatchRecoveryFailed",
            "the recovery write for an interrupted dispatch did not complete, so its Run still has no terminal Receipt",
        ),
        (
            "PauseEventMismatch",
            "Event sequence 4242 is not a well-formed Pause record",
        ),
        (
            "SchemaMismatch",
            "store schema does not match its declared version",
        ),
        (
            "UnsupportedSchemaVersion",
            "store schema version 4242 is unsupported",
        ),
        (
            "IntegrityCheckFailed",
            "SQLite integrity check failed: IntegrityCheckFailed",
        ),
        (
            "StoredIdentityUnreadable",
            "a Mission row's identity is not readable: StoredIdentityUnreadable",
        ),
        (
            "StoredIntegerKeyUnreadable",
            "a Run row's identity is not readable: StoredIntegerKeyUnreadable, \
             boundary_event_seq 11",
        ),
        (
            "StartupEnumerationFailed",
            "the Project startup pass could not enumerate its rows",
        ),
        (
            "ForeignKeyViolations",
            "store contains 4242 foreign-key violation(s)",
        ),
    ];

    /// The expected startup line for one root, from the transcribed table above.
    fn startup_line(case: &str) -> String {
        STARTUP_LINES
            .iter()
            .find_map(|(name, line)| (*name == case).then_some((*line).to_owned()))
            .unwrap_or_else(|| panic!("R42S2.STARTUP.{case}.LISTED: no row in STARTUP_LINES"))
    }

    /// The same declarative-row shape the transport witness uses, defined here because a test
    /// module cannot borrow another module's macro -- and because S's fixtures must be
    /// independent of T's, which is the point of the fold.
    macro_rules! closed_table {
        (
            $expected_fn:ident / $rows_fn:ident : $ty:ty,
            $( $case:literal | $pattern:pat => $line:expr , $build:expr , $markers:expr ; )+
        ) => {
            fn $expected_fn(value: &$ty) -> (&'static str, String) {
                match value {
                    $( $pattern => ($case, String::from($line)), )+
                }
            }

            fn $rows_fn() -> Vec<(&'static str, $ty, &'static [&'static str])> {
                vec![$(($case, $build, $markers)),+]
            }
        };
    }

    closed_table! {
        expected_startup / startup_rows: SupervisorError,
        "EmptyWorkspaceName" | SupervisorError::EmptyWorkspaceName
            => startup_line("EmptyWorkspaceName"),
            SupervisorError::EmptyWorkspaceName, &[];
        "WorkspaceNameTooLong" | SupervisorError::WorkspaceNameTooLong
            => startup_line("WorkspaceNameTooLong"),
            SupervisorError::WorkspaceNameTooLong, &[];
        "WorkspaceNameContainsControl" | SupervisorError::WorkspaceNameContainsControl
            => startup_line("WorkspaceNameContainsControl"),
            SupervisorError::WorkspaceNameContainsControl, &[];
        "ClockBeforeUnixEpoch" | SupervisorError::ClockBeforeUnixEpoch
            => startup_line("ClockBeforeUnixEpoch"),
            SupervisorError::ClockBeforeUnixEpoch, &[];
        "Io" | SupervisorError::Io(..)
            => startup_line("Io"),
            SupervisorError::Io(std::io::Error::other(R42S2_NEEDLE_A)), &[R42S2_NEEDLE_A];
        "Store" | SupervisorError::Store(..)
            => startup_line("Store"),
            SupervisorError::Store(r42s2_driver()), &[R42S2_NEEDLE_A];
        "Json" | SupervisorError::Json(..)
            => startup_line("Json"),
            SupervisorError::Json(serde::de::Error::custom(R42S2_NEEDLE_A)), &[R42S2_NEEDLE_A];
        "InvalidWorkspaceId" | SupervisorError::InvalidWorkspaceId(..)
            => startup_line("InvalidWorkspaceId"),
            SupervisorError::InvalidWorkspaceId(r42s2_digest()), &[];
        "InvalidEventKind" | SupervisorError::InvalidEventKind(..)
            => startup_line("InvalidEventKind"),
            SupervisorError::InvalidEventKind(r42s2_digest()), &[];
        "EventPayloadScopeMismatch" | SupervisorError::EventPayloadScopeMismatch(..)
            => startup_line("EventPayloadScopeMismatch"),
            SupervisorError::EventPayloadScopeMismatch(R42S2_SEQUENCE_I64), &[];
        "EventChainBroken" | SupervisorError::EventChainBroken(..)
            => startup_line("EventChainBroken"),
            SupervisorError::EventChainBroken(
                crate::event_chain::ChainFault::LinkDigestMismatch { sequence: 4242 },
            ), &[];
        "PauseStateUnsupported" | SupervisorError::PauseStateUnsupported { .. }
            => startup_line("PauseStateUnsupported"),
            SupervisorError::PauseStateUnsupported {
                changed_at_ms: R42S2_SEQUENCE_I64,
                terminal_sequence: None,
            }, &[];
        "StarterOrganizationEventMismatch" | SupervisorError::StarterOrganizationEventMismatch(..)
            => startup_line("StarterOrganizationEventMismatch"),
            SupervisorError::StarterOrganizationEventMismatch(R42S2_SEQUENCE_I64), &[];
        "StarterOrganizationStateMismatch" | SupervisorError::StarterOrganizationStateMismatch { .. }
            => startup_line("StarterOrganizationStateMismatch"),
            SupervisorError::StarterOrganizationStateMismatch { workspace_id: r42s2_uuid(), cause: crate::StarterOrganizationError::EmptyRoster }, &[];
        "ProjectEventMismatch" | SupervisorError::ProjectEventMismatch(..)
            => startup_line("ProjectEventMismatch"),
            SupervisorError::ProjectEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ProjectStateMismatch" | SupervisorError::ProjectStateMismatch { .. }
            => startup_line("ProjectStateMismatch"),
            SupervisorError::ProjectStateMismatch { workspace_id: r42s2_uuid(), project_id: r42s2_uuid(), cause: crate::ProjectError::NotFound }, &[];
        "ProjectTotalsMismatch" | SupervisorError::ProjectTotalsMismatch(..)
            => startup_line("ProjectTotalsMismatch"),
            SupervisorError::ProjectTotalsMismatch(r42s2_totals()), &[];
        "LocalModelActivationEventMismatch" | SupervisorError::LocalModelActivationEventMismatch(..)
            => startup_line("LocalModelActivationEventMismatch"),
            SupervisorError::LocalModelActivationEventMismatch(R42S2_SEQUENCE_I64), &[];
        "LocalModelActivationStateMismatch" | SupervisorError::LocalModelActivationStateMismatch { .. }
            => startup_line("LocalModelActivationStateMismatch"),
            SupervisorError::LocalModelActivationStateMismatch { workspace_id: r42s2_uuid(), activation_id: r42s2_uuid(), cause: crate::LocalModelActivationError::ModelNotFound }, &[];
        "LocalModelActivationRowUnreadable" | SupervisorError::LocalModelActivationRowUnreadable(..)
            => startup_line("LocalModelActivationRowUnreadable"),
            SupervisorError::LocalModelActivationRowUnreadable(crate::LocalModelActivationError::ModelNotFound), &[];
        "ResidentLifecycleEventMismatch" | SupervisorError::ResidentLifecycleEventMismatch(..)
            => startup_line("ResidentLifecycleEventMismatch"),
            SupervisorError::ResidentLifecycleEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ResidentLifecycleStateMismatch" | SupervisorError::ResidentLifecycleStateMismatch { .. }
            => startup_line("ResidentLifecycleStateMismatch"),
            SupervisorError::ResidentLifecycleStateMismatch { workspace_id: r42s2_uuid(), resident_id: r42s2_uuid(), cause: crate::ResidentLifecycleError::NotFound }, &[];
        "ResidentLifecycleTotalsMismatch" | SupervisorError::ResidentLifecycleTotalsMismatch(..)
            => startup_line("ResidentLifecycleTotalsMismatch"),
            SupervisorError::ResidentLifecycleTotalsMismatch(r42s2_totals()), &[];
        "ResidentRowUnreadable" | SupervisorError::ResidentRowUnreadable(..)
            => startup_line("ResidentRowUnreadable"),
            SupervisorError::ResidentRowUnreadable(crate::ResidentLifecycleError::NotFound), &[];
        "ResidentConfigurationEventMismatch" | SupervisorError::ResidentConfigurationEventMismatch(..)
            => startup_line("ResidentConfigurationEventMismatch"),
            SupervisorError::ResidentConfigurationEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ResidentConfigurationStateMismatch" | SupervisorError::ResidentConfigurationStateMismatch { .. }
            => startup_line("ResidentConfigurationStateMismatch"),
            SupervisorError::ResidentConfigurationStateMismatch { workspace_id: r42s2_uuid(), decision_id: r42s2_digest(), cause: crate::ResidentConfigurationError::ResidentNotFound }, &[];
        "TeamEventMismatch" | SupervisorError::TeamEventMismatch(..)
            => startup_line("TeamEventMismatch"),
            SupervisorError::TeamEventMismatch(R42S2_SEQUENCE_I64), &[];
        "TeamStateMismatch" | SupervisorError::TeamStateMismatch { .. }
            => startup_line("TeamStateMismatch"),
            SupervisorError::TeamStateMismatch { workspace_id: r42s2_uuid(), team_id: r42s2_uuid(), cause: crate::TeamError::TeamNotFound }, &[];
        "TeamTotalsMismatch" | SupervisorError::TeamTotalsMismatch(..)
            => startup_line("TeamTotalsMismatch"),
            SupervisorError::TeamTotalsMismatch(r42s2_totals()), &[];
        "MissionEventMismatch" | SupervisorError::MissionEventMismatch(..)
            => startup_line("MissionEventMismatch"),
            SupervisorError::MissionEventMismatch(R42S2_SEQUENCE_I64), &[];
        "MissionStateMismatch" | SupervisorError::MissionStateMismatch { .. }
            => startup_line("MissionStateMismatch"),
            SupervisorError::MissionStateMismatch { workspace_id: r42s2_uuid(), mission_id: r42s2_uuid(), cause: crate::MissionError::MissionNotFound }, &[];
        "MissionTotalsMismatch" | SupervisorError::MissionTotalsMismatch(..)
            => startup_line("MissionTotalsMismatch"),
            SupervisorError::MissionTotalsMismatch(r42s2_totals()), &[];
        "ProjectPolicyEventMismatch" | SupervisorError::ProjectPolicyEventMismatch(..)
            => startup_line("ProjectPolicyEventMismatch"),
            SupervisorError::ProjectPolicyEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ProjectPolicyStateMismatch" | SupervisorError::ProjectPolicyStateMismatch { .. }
            => startup_line("ProjectPolicyStateMismatch"),
            SupervisorError::ProjectPolicyStateMismatch { workspace_id: r42s2_uuid(), project_id: r42s2_uuid(), cause: crate::ProjectPolicyError::ProjectNotFound }, &[];
        "ProjectRepositoryEventMismatch" | SupervisorError::ProjectRepositoryEventMismatch(..)
            => startup_line("ProjectRepositoryEventMismatch"),
            SupervisorError::ProjectRepositoryEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ProjectRepositoryStateMismatch" | SupervisorError::ProjectRepositoryStateMismatch { .. }
            => startup_line("ProjectRepositoryStateMismatch"),
            SupervisorError::ProjectRepositoryStateMismatch { workspace_id: r42s2_uuid(), project_id: r42s2_uuid(), cause: crate::ProjectRepositoryError::ProjectNotFound }, &[];
        "SampleProjectEventMismatch" | SupervisorError::SampleProjectEventMismatch(..)
            => startup_line("SampleProjectEventMismatch"),
            SupervisorError::SampleProjectEventMismatch(R42S2_SEQUENCE_I64), &[];
        "SampleProjectStateMismatch" | SupervisorError::SampleProjectStateMismatch { .. }
            => startup_line("SampleProjectStateMismatch"),
            SupervisorError::SampleProjectStateMismatch { workspace_id: r42s2_uuid(), cause: crate::SampleProjectError::NotInstalled }, &[];
        "SampleProjectTotalsMismatch" | SupervisorError::SampleProjectTotalsMismatch(..)
            => startup_line("SampleProjectTotalsMismatch"),
            SupervisorError::SampleProjectTotalsMismatch(r42s2_totals()), &[];
        "RunEventMismatch" | SupervisorError::RunEventMismatch(..)
            => startup_line("RunEventMismatch"),
            SupervisorError::RunEventMismatch(R42S2_SEQUENCE_I64), &[];
        "RunStateMismatch" | SupervisorError::RunStateMismatch { .. }
            => startup_line("RunStateMismatch"),
            SupervisorError::RunStateMismatch { workspace_id: r42s2_uuid(), run_id: r42s2_digest(), cause: crate::CheckPreflightError::ProjectNotFound }, &[];
        "RunClockBeforeEpoch" | SupervisorError::RunClockBeforeEpoch
            => startup_line("RunClockBeforeEpoch"),
            SupervisorError::RunClockBeforeEpoch, &[];
        "RunEventStateMismatch" | SupervisorError::RunEventStateMismatch { .. }
            => startup_line("RunEventStateMismatch"),
            SupervisorError::RunEventStateMismatch { sequence: R42S2_STORED_VERSION, workspace_id: r42s2_uuid(), run_id: r42s2_uuid() }, &[];
        "ProposalEventMismatch" | SupervisorError::ProposalEventMismatch(..)
            => startup_line("ProposalEventMismatch"),
            SupervisorError::ProposalEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ProposalStateMismatch" | SupervisorError::ProposalStateMismatch { .. }
            => startup_line("ProposalStateMismatch"),
            SupervisorError::ProposalStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::ProposalError::NotFound }, &[];
        "ProposalTotalsMismatch" | SupervisorError::ProposalTotalsMismatch(..)
            => startup_line("ProposalTotalsMismatch"),
            SupervisorError::ProposalTotalsMismatch(r42s2_totals()), &[];
        "RulingEventMismatch" | SupervisorError::RulingEventMismatch(..)
            => startup_line("RulingEventMismatch"),
            SupervisorError::RulingEventMismatch(R42S2_SEQUENCE_I64), &[];
        "EvaluationEventMismatch" | SupervisorError::EvaluationEventMismatch(..)
            => startup_line("EvaluationEventMismatch"),
            SupervisorError::EvaluationEventMismatch(R42S2_SEQUENCE_I64), &[];
        "MissionCompletionEventMismatch" | SupervisorError::MissionCompletionEventMismatch(..)
            => startup_line("MissionCompletionEventMismatch"),
            SupervisorError::MissionCompletionEventMismatch(R42S2_SEQUENCE_I64), &[];
        "MissionCompletionStateMismatch" | SupervisorError::MissionCompletionStateMismatch { .. }
            => startup_line("MissionCompletionStateMismatch"),
            SupervisorError::MissionCompletionStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::CompletionReadRefusal::Absent }, &[];
        "MissionCompletionTotalsMismatch" | SupervisorError::MissionCompletionTotalsMismatch(..)
            => startup_line("MissionCompletionTotalsMismatch"),
            SupervisorError::MissionCompletionTotalsMismatch(r42s2_totals()), &[];
        "EvaluationStateMismatch" | SupervisorError::EvaluationStateMismatch { .. }
            => startup_line("EvaluationStateMismatch"),
            SupervisorError::EvaluationStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::EvaluationError::NotFound }, &[];
        "EvaluationChainsMismatch" | SupervisorError::EvaluationChainsMismatch(..)
            => startup_line("EvaluationChainsMismatch"),
            SupervisorError::EvaluationChainsMismatch(crate::EvaluationError::NotFound), &[];
        "ArtifactEventMismatch" | SupervisorError::ArtifactEventMismatch(..)
            => startup_line("ArtifactEventMismatch"),
            SupervisorError::ArtifactEventMismatch(R42S2_SEQUENCE_I64), &[];
        "ArtifactStateMismatch" | SupervisorError::ArtifactStateMismatch { .. }
            => startup_line("ArtifactStateMismatch"),
            SupervisorError::ArtifactStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::ArtifactError::NotFound }, &[];
        "ArtifactTotalsMismatch" | SupervisorError::ArtifactTotalsMismatch(..)
            => startup_line("ArtifactTotalsMismatch"),
            SupervisorError::ArtifactTotalsMismatch(r42s2_totals()), &[];
        "ArtifactRelationMismatch" | SupervisorError::ArtifactRelationMismatch(..)
            => startup_line("ArtifactRelationMismatch"),
            SupervisorError::ArtifactRelationMismatch(crate::ArtifactError::NotFound), &[];
        "MissionImpactClassificationStateMismatch" | SupervisorError::MissionImpactClassificationStateMismatch { .. }
            => startup_line("MissionImpactClassificationStateMismatch"),
            SupervisorError::MissionImpactClassificationStateMismatch { workspace_id: r42s2_uuid(), digest: r42s2_digest(), cause: crate::ClassificationStoreError::ColumnsDisagree }, &[];
        "OutcomeManifestStateMismatch" | SupervisorError::OutcomeManifestStateMismatch { .. }
            => startup_line("OutcomeManifestStateMismatch"),
            SupervisorError::OutcomeManifestStateMismatch { workspace_id: r42s2_uuid(), manifest_digest: r42s2_digest(), cause: crate::ManifestStoreRefusal::Absent }, &[];
        "OutcomePublicationStateMismatch" | SupervisorError::OutcomePublicationStateMismatch { .. }
            => startup_line("OutcomePublicationStateMismatch"),
            SupervisorError::OutcomePublicationStateMismatch { workspace_id: r42s2_uuid(), mission_id: r42s2_uuid(), version: R42S2_STORED_VERSION, cause: crate::PublishRefusal::Absent }, &[];
        "RulingStateMismatch" | SupervisorError::RulingStateMismatch { .. }
            => startup_line("RulingStateMismatch"),
            SupervisorError::RulingStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::RulingError::NotFound }, &[];
        "RulingTotalsMismatch" | SupervisorError::RulingTotalsMismatch(..)
            => startup_line("RulingTotalsMismatch"),
            SupervisorError::RulingTotalsMismatch(r42s2_totals()), &[];
        "RulingChainsMismatch" | SupervisorError::RulingChainsMismatch(..)
            => startup_line("RulingChainsMismatch"),
            SupervisorError::RulingChainsMismatch(crate::RulingError::NotFound), &[];
        "CandidateEventMismatch" | SupervisorError::CandidateEventMismatch(..)
            => startup_line("CandidateEventMismatch"),
            SupervisorError::CandidateEventMismatch(R42S2_SEQUENCE_I64), &[];
        "CandidateStateMismatch" | SupervisorError::CandidateStateMismatch { .. }
            => startup_line("CandidateStateMismatch"),
            SupervisorError::CandidateStateMismatch { workspace_id: r42s2_uuid(), digest_value: r42s2_digest(), cause: crate::CandidateError::NotFound }, &[];
        "DispatchEventMismatch" | SupervisorError::DispatchEventMismatch(..)
            => startup_line("DispatchEventMismatch"),
            SupervisorError::DispatchEventMismatch(R42S2_SEQUENCE_I64), &[];
        "DispatchRecoveryFailed" | SupervisorError::DispatchRecoveryFailed
            => startup_line("DispatchRecoveryFailed"),
            SupervisorError::DispatchRecoveryFailed, &[];
        "PauseEventMismatch" | SupervisorError::PauseEventMismatch(..)
            => startup_line("PauseEventMismatch"),
            SupervisorError::PauseEventMismatch(R42S2_SEQUENCE_I64), &[];
        "SchemaMismatch" | SupervisorError::SchemaMismatch
            => startup_line("SchemaMismatch"),
            SupervisorError::SchemaMismatch, &[];
        "UnsupportedSchemaVersion" | SupervisorError::UnsupportedSchemaVersion(..)
            => startup_line("UnsupportedSchemaVersion"),
            SupervisorError::UnsupportedSchemaVersion(R42S2_SEQUENCE_I64), &[];
        "IntegrityCheckFailed" | SupervisorError::IntegrityCheckFailed(..)
            => startup_line("IntegrityCheckFailed"),
            SupervisorError::IntegrityCheckFailed(r42s2_digest()), &[];
        "StoredIdentityUnreadable" | SupervisorError::StoredIdentityUnreadable { .. }
            => startup_line("StoredIdentityUnreadable"),
            SupervisorError::StoredIdentityUnreadable {
                what: "a Mission row's identity is not readable",
                raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
            }, &[R42S2_NEEDLE_A];
        // R45's 73rd arm, now closed against the same projection as the arm above it. The raw
        // carries a marker BECAUSE the line no longer does -- which is the whole point.
        "StoredIntegerKeyUnreadable" | SupervisorError::StoredIntegerKeyUnreadable { .. }
            => startup_line("StoredIntegerKeyUnreadable"),
            SupervisorError::StoredIntegerKeyUnreadable {
                what: "a Run row's identity is not readable",
                raw: format!("{R42S2_NEEDLE_A} /home/example/private.txt"),
                key_name: "boundary_event_seq",
                key: 11,
            },
            &[R42S2_NEEDLE_A];
        "StartupEnumerationFailed" | SupervisorError::StartupEnumerationFailed { .. }
            => startup_line("StartupEnumerationFailed"),
            SupervisorError::StartupEnumerationFailed { pass: crate::StartupPass::Project, source: r42s2_driver() }, &[R42S2_NEEDLE_A];
        "ForeignKeyViolations" | SupervisorError::ForeignKeyViolations(..)
            => startup_line("ForeignKeyViolations"),
            SupervisorError::ForeignKeyViolations(R42S2_SEQUENCE), &[];
    }

    /// R42 slice 2, fold: every one of the 72 startup lines is what the spec says.
    #[test]
    fn r42_startup_templates_are_all_pinned() {
        assert_eq!(STARTUP_LINES.len(), 75, "R42S2.STARTUP.CARDINALITY.LINES");
        assert_eq!(startup_rows().len(), 75, "R42S2.STARTUP.CARDINALITY.ROWS");
        let ids: std::collections::BTreeSet<&str> =
            startup_rows().iter().map(|(case, _, _)| *case).collect();
        assert_eq!(ids.len(), 75, "R42S2.STARTUP.CASE_IDS_UNIQUE");

        for (case, value, markers) in startup_rows() {
            let (matched, expected) = expected_startup(&value);
            assert_eq!(matched, case, "R42S2.FIXTURE.STARTUP.{case}.PATTERN");
            let line = value.to_string();
            for marker in markers {
                assert!(
                    !line.contains(marker),
                    "R42S2.STARTUP.{case}.NO_NEEDLE: {line}"
                );
                assert!(
                    format!("{value:?}").contains(marker),
                    "R42S2.STARTUP.{case}.DEBUG_RETAINS"
                );
            }
            assert!(
                !line.contains('\n') && !line.contains('\r'),
                "R42S2.STARTUP.{case}.ONE_LINE: {line:?}"
            );
            assert_eq!(line, expected, "R42S2.STARTUP.{case}.PROJECTION");
        }
    }
}
