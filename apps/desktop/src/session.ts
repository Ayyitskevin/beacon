import { invoke } from "@tauri-apps/api/core";

type LockedPhase = "operator_setup_required" | "locked";


/** One declared success condition's recorded result. `ordinal` is its 0-based index in the
 *  Mission's own declaration order, which is how it pairs with `successConditions`. */
export type CompletionConditionResult = {
  ordinal: number;
  outcome: "met" | "not_met" | "not_assessed" | "assessment_inconclusive";
  supportCount: number;
  note: string | null;
};

/** What a Mission's verified completion RECORDED. Neither disposition is a success verdict, and
 *  the counts are counts of recorded and cited things -- this reader verifies none of them. */
export type CompletionView = {
  missionId: string;
  completionId: string;
  actor: "operator";
  accountableResidentId: string;
  accountableResidentConfigurationId: string;
  disposition: "concluded" | "abandoned";
  conditionResults: CompletionConditionResult[];
  deliveredArtifactCount: number;
  unfinishedWorkCount: number;
};


/** One manually entered supporting reference. Externally tagged, exactly as the C4 contract fixes
 *  it -- and deliberately distinct from the daemon's outer command tag. */
export type SupportRefDto =
  | { artifact: string }
  | { receipt: { runId: string; receiptVersion: number; receiptBytesSha256: string } }
  | { evaluation: string }
  | { ruling: string };

export type CompletionConditionInput = {
  ordinal: number;
  outcome: "met" | "not_met" | "not_assessed" | "assessment_inconclusive";
  support: SupportRefDto[];
  note: string | null;
};

export type CompletionNarrativeInput<K extends string> = {
  ordinal: number;
  kind: K;
  conditionOrdinal: number | null;
  summary: string;
};

export type UnfinishedKindToken =
  | "condition_not_met"
  | "deferred_scope"
  | "known_defect"
  | "blocked_dependency";
export type UncertaintyKindToken =
  | "unverified_claim"
  | "unmeasured_quantity"
  | "conflicting_evidence"
  | "outside_scope_of_checks";

export type SpendReferenceDto =
  | { recorded: { spendId: string; spendVersion: number; spendSha256: string } }
  | { notMeasured: string };

/** Everything the Operator authored. Identity, service time and the boundary are NOT here: Beacon
 *  assigns them when it records. */
export type CompletionIntentRequest = {
  missionId: string;
  accountableResidentId: string;
  disposition: "concluded" | "abandoned";
  conditionResults: CompletionConditionInput[];
  deliveredArtifacts: string[];
  unfinishedWork: CompletionNarrativeInput<UnfinishedKindToken>[];
  uncertainty: CompletionNarrativeInput<UncertaintyKindToken>[];
  spend: SpendReferenceDto;
  summary: string;
};

/** The owner's normalized echo of the request, plus the bindings it verified. */
export type CompletionPreview = {
  request: CompletionIntentRequest;
  intentSha256: string;
  workspaceId: string;
  projectId: string;
  actor: "operator";
  accountableResidentConfigurationId: string;
  declaredConditionsSha256: string;
  conditions: string[];
};

export type DesktopSessionStatus =
  | {
      phase: LockedPhase;
      workspaceAccess: false;
      operatorCommands: false;
    }
  | {
      phase: "unlocked";
      workspaceAccess: true;
      operatorCommands: true;
    };

export type AuthenticationCapabilities = {
  preferredMethod: "passkey";
  passkey: {
    available: false;
    reason: "unsupported_linux_webkitgtk" | "native_adapter_not_available";
  };
  passwordFallback: {
    available: true;
    scope: "same_machine";
  };
};

export type RecoveryKeyOutput = { recoveryKey: string };

export type SessionPolicy = { inactivityTimeoutMinutes: number };

type PlatformMonitorState = "starting" | "active" | "unavailable";
export type PlatformSessionMonitorStatus = {
  screenLock: PlatformMonitorState;
  suspend: PlatformMonitorState;
};

export type StarterRole = "lead" | "builder" | "reviewer" | "researcher";
export type StarterResidentDraft = { role: StarterRole; displayName: string | null };
export type StarterRoleTemplate = {
  role: StarterRole;
  description: string;
  recommended: boolean;
};
export type ResidentAgentSummary = {
  id: string;
  role: StarterRole;
  displayName: string | null;
  lifecycle: "inactive";
};
export type ResidentLifecycle = "inactive" | "active" | "suspended" | "retired";
export type ResidentAgentView = {
  id: string;
  workspaceId: string;
  role: StarterRole;
  displayName: string | null;
  lifecycle: ResidentLifecycle;
  latestDecisionId: string | null;
  createdAtMs: number;
  updatedAtMs: number;
};
export type StarterOrganizationView = {
  workspaceId: string;
  proposalVersion: 1;
  decision: "proposed" | "accepted" | "discarded";
  templates: StarterRoleTemplate[];
  residents: ResidentAgentSummary[];
};

export type AutonomyProfile = "observe" | "supervised";
export type ProjectCharterDraft = {
  purpose: string;
  desiredOutcomes: string[];
  constraints: string[];
  prohibitions: string[];
  autonomyProfile: AutonomyProfile;
};
export type ProjectDraft = { name: string; charter: ProjectCharterDraft };
export type ProjectCharter = ProjectCharterDraft & {
  revision: number;
  budgetBoundary: "no_spend";
  externalEffects: "prohibited";
  authoredAtMs: number;
};
export type ProjectView = {
  id: string;
  workspaceId: string;
  name: string;
  charter: ProjectCharter;
  createdAtMs: number;
};
export type ProjectSummary = {
  id: string;
  name: string;
  charterRevision: number;
  autonomyProfile: AutonomyProfile;
  createdAtMs: number;
};

export type SampleProjectDigest = {
  algorithm: "sha256";
  canonicalizer: "beacon.sample-project.bundle.v1";
  value: string;
};
export type SampleProjectInstallation = {
  workspaceId: string;
  projectId: string;
  charterRevision: 1;
  repositoryRevision: 1;
  repositoryPath: string;
  baseRevision: string;
  provenance: {
    source: "beacon_bundled_sample_project";
    bundleVersion: 1;
    bundleDigest: SampleProjectDigest;
  };
  installedAtMs: number;
};
export type SampleProjectStatus =
  | { state: "available"; bundleVersion: 1; bundleDigest: SampleProjectDigest }
  | { state: "installed"; installation: SampleProjectInstallation };
export type SampleProjectInstallResult = {
  outcome: "installed" | "already_installed";
  installation: SampleProjectInstallation;
};

export type TrackRecordEvidenceKind =
  | "mission_outcome"
  | "evaluation"
  | "policy_compliance"
  | "spend"
  | "receipt";
export type TrackRecordRequirement = {
  evidenceKind: TrackRecordEvidenceKind;
  minimumCount: number;
};
export type TeamLeadSelectionPolicy = {
  requiredCapabilities: BeaconCapability[];
  availabilityRule: "current_team_eligibility";
  trackRecordRequirements: TrackRecordRequirement[];
  conflictRule: "require_no_declared_conflict";
  ambiguityRule: "require_operator_decision";
};
export type ProjectPolicyDraft = { teamLeadSelection: TeamLeadSelectionPolicy };
export type ProjectPolicy = ProjectPolicyDraft & {
  workspaceId: string;
  projectId: string;
  revision: number;
  charterRevision: number;
  authoredBy: "operator";
  authoredAtMs: number;
  appliesToCurrentCharter: boolean;
};

export type RepositorySourceKind = "local_path" | "git_url";
export type DeclaredCheck = {
  name: string;
  argv: string[];
  workingDirectory: string;
};
export type PreflightOutcome =
  | "verified"
  | "source_kind_unsupported"
  | "binding_stale_for_charter"
  | "repository_unreadable"
  | "base_revision_unreachable"
  | "declared_checks_changed"
  | "working_directory_escapes"
  | "sandbox_unavailable"
  | "preflight_failed"
  | "preflight_timed_out";

/** A value the Receipt specification requires that a preflight cannot supply.
 *  Absent is not empty: one was never available, the other was measured. */
export type Recorded<T> = { kind: "present"; value: T } | { kind: "absent"; reason: string };

export type ResolvedCheck = {
  name: string;
  argv: string[];
  declaredWorkingDirectory: string;
  resolvedWorkingDirectory: Recorded<string>;
};

export type UnavailableEvidence = {
  proposalDigest: Recorded<string>;
  rulingDigest: Recorded<string>;
  resultingRevision: Recorded<string>;
  filesystemEffects: Recorded<string>;
  diffDigest: Recorded<string>;
  modelInvocations: Recorded<number>;
  costUsdMicros: Recorded<number>;
};

/** The most Receipts one Project's page may carry, matching the repository
 *  history cap. The daemon applies no LIMIT, so this is the desktop's, and it
 *  is named because Briefing's collection bound is stated in terms of it:
 *  what a reading holds never exceeds its aggregate cap plus ONE page. */
export const MAX_PROJECT_RECEIPT_PAGE = 512;

export type PreflightReceipt = {
  version: number;
  runId: string;
  workspaceId: string;
  projectId: string;
  repositoryRevision: number;
  charterRevision: number;
  outcome: PreflightOutcome;
  sourceKind: RepositorySourceKind;
  source: string;
  baseRevision: string;
  resolvedChecks: ResolvedCheck[];
  sandboxVersion: string;
  policyCharterRevision: number;
  policyRepositoryRevision: number;
  startedAtMs: number;
  endedAtMs: number;
  /** Every step this preflight did not perform. Required: a Receipt that cannot
   *  say what it failed to check is not evidence. */
  notVerified: string[];
  unavailable: UnavailableEvidence;
};

/** Which Run a Receipt belongs to. Closed on both sides of the wire. */
export const RECEIPT_RUN_KINDS = ["check_preflight", "declared_checks"] as const;
export type ReceiptRunKind = (typeof RECEIPT_RUN_KINDS)[number];

/** How an execution ended. Four values, mirroring `ExecutionSummary`. */
export const EXECUTION_SUMMARIES = [
  "not_admissible",
  "worktree_unavailable",
  "completed",
  "incomplete",
] as const;
export type ExecutionSummary = (typeof EXECUTION_SUMMARIES)[number];

/** Closed PER KIND. The stored `outcome` column is one CHECK shared by both
 *  kinds, so a flat union would let a preflight row carry `completed` and an
 *  execution row carry `preflight_failed`. */
export type RecentReceiptOutcome =
  | { kind: "check_preflight"; outcome: PreflightOutcome }
  | { kind: "declared_checks"; summary: ExecutionSummary };

export type RecentReceipt = {
  runId: string;
  projectId: string;
  projectName: string;
  version: number;
  outcome: RecentReceiptOutcome;
  startedAtMs: number;
  endedAtMs: number;
  recordedAtMs: number;
};

/** One bounded reading.
 *
 *  The window is over RUNS, not Receipts: a request for 50 can return fewer
 *  than 50 Receipts while older Receipts exist, and `moreRunsExist` can be
 *  true because an older Run WITHOUT a Receipt exists. Every name says so, and
 *  the panel must never turn one of these counts back into a bare "and more".
 *
 *  `runsUnconfirmedInWindow` is NOT the Resume Work review set: an unconfirmed
 *  Run older than the window is not in it. */
export type RecentReceiptPage = {
  receipts: RecentReceipt[];
  runsInFlightInWindow: number;
  runsUnconfirmedInWindow: number;
  runsVerifiedExcludedInWindow: number;
  runWindowApplied: number;
  moreRunsExist: boolean;
};

/** Rust counts these in a `u32`; larger is a shape the reader cannot produce. */
const MAX_U32_COUNT = 4_294_967_295;

export type RepositoryDraft = {
  sourceKind: RepositorySourceKind;
  source: string;
  baseRevision: string;
  declaredChecks: DeclaredCheck[];
};
export type ProjectRepository = RepositoryDraft & {
  workspaceId: string;
  projectId: string;
  revision: number;
  charterRevision: number;
  authorKind: "operator";
  authoredAtMs: number;
  appliesToCurrentCharter: boolean;
};

export type DiscoveryStatus = "available" | "unavailable" | "invalid_response";
export type LocalModelDetails = {
  format: string;
  family: string;
  parameterSize: string;
  quantizationLevel: string;
};
export type DiscoveredLocalModel = {
  provider: "ollama";
  activation: "inactive";
  name: string;
  digest: string;
  sizeBytes: number;
  modifiedAt: string;
  details: LocalModelDetails;
};
export type LocalModelDiscoveryReport = {
  status: DiscoveryStatus;
  endpoint: string;
  models: DiscoveredLocalModel[];
  observedAtMs: number;
};

/** Three states, closed. Production v1 emits only `unknown`; `enabled` and
 *  `disabled` remain the deferred detector candidate's executable vocabulary.
 *  A state is read off its reason (`stateForReason`), never agreed with it. */
export type LiveDataEncryption = "enabled" | "disabled" | "unknown";

/** Every reason the reader can produce. Closed on both sides of the wire: a
 *  report naming anything else is a refused read, not a rendered surprise. */
export const REASON_CODES = [
  "crypt_covers_every_path",
  "no_crypt_on_any_path",
  "some_paths_uncovered",
  "store_path_unresolved",
  "store_not_inspectable",
  "mount_table_unreadable",
  "mount_table_unparsed",
  "mount_table_empty",
  "no_mount_for_store",
  "mount_and_stat_disagree",
  "memory_backed_filesystem",
  "overlay_filesystem",
  "userspace_filesystem",
  "network_filesystem",
  "no_block_device_for_filesystem",
  "link_resolves_to_nothing",
  "device_unreadable",
  "read_over_budget",
  "stack_over_budget",
  "no_physical_leaf",
  "mapping_without_backing_device",
  "loopback_image",
  "platform_not_supported",
  /** Version 1 records bounded observations but deliberately does not promote
   *  the detector candidate's inference into an encryption claim. */
  "detection_not_evaluated",
  /** The Query itself did not answer. Built here rather than read off the
   *  wire: the reader cannot report that it was never reached, so the client
   *  says so in the same closed vocabulary instead of carrying a free-text
   *  failure beside a missing report. */
  "initial_read_failed",
] as const;
export type ReasonCode = (typeof REASON_CODES)[number];

/** The state a reason means. One direction, mirroring `state_for` in
 *  `crates/beacond/src/disk_encryption.rs`, so the two can never drift. */
export function stateForReason(reason: ReasonCode): LiveDataEncryption {
  if (reason === "crypt_covers_every_path") return "enabled";
  if (reason === "no_crypt_on_any_path" || reason === "some_paths_uncovered") return "disabled";
  return "unknown";
}

/** The roles a reason's sentence names, and therefore the roles a conclusion
 *  carrying it must have a subject for. Mirrors `required_roles`. A conclusion
 *  missing one renders "an unnamed subject" -- a sentence that says nothing --
 *  so it is a refused read rather than a rendered surprise. */
export function requiredRoles(reason: ReasonCode): readonly SubjectRole[] {
  switch (reason) {
    case "crypt_covers_every_path":
      return ["mechanism"];
    case "no_crypt_on_any_path":
      return ["device"];
    case "some_paths_uncovered":
      return ["mapping", "device"];
    case "store_path_unresolved":
    case "initial_read_failed":
      return ["detail"];
    case "no_block_device_for_filesystem":
    case "link_resolves_to_nothing":
    case "device_unreadable":
    case "read_over_budget":
    case "loopback_image":
      return ["location"];
    case "mapping_without_backing_device":
      return ["mapping"];
    default:
      return [];
  }
}

export const SUBJECT_ROLES = ["device", "mapping", "mechanism", "location", "detail"] as const;
export type SubjectRole = (typeof SUBJECT_ROLES)[number];

/** A value inside a conclusion. Bounded where it was made, and it says whether
 *  it was cut -- the panel reads the flag rather than hunting for an ellipsis. */
export type Subject = {
  role: SubjectRole;
  value: string;
  shortened: boolean;
};

export type EncryptionEvidence = {
  source: string;
  observation: string;
  /** True when either field was cut. */
  truncated: boolean;
};

/** The answer: closed state, closed reason, bounded subjects, and a count of
 *  the subjects that did not fit. No free text crosses the wire. */
export type LiveDataEncryptionConclusion = {
  state: LiveDataEncryption;
  reason: ReasonCode;
  subjects: Subject[];
  omittedSubjects: number;
};

export type LiveDataEncryptionReport = {
  conclusion: LiveDataEncryptionConclusion;
  mountPoint: Subject | null;
  evidence: EncryptionEvidence[];
  /** How many readings did not fit. The evidence list is capped like the
   *  subject list, and a capped list that does not say so is a partial answer
   *  wearing a complete one. */
  omittedEvidence: number;
  notVerified: string[];
  observedAtMs: number;
};

/** The bound the wire shape makes structural. */
export const MAX_SUBJECT_CHARS = 24;
export const MAX_SUBJECTS = 3;
export const MAX_EVIDENCE_ENTRIES = 32;
export const MAX_NOT_VERIFIED_ENTRIES = 8;
export const MAX_OBSERVATION_CHARS = 200;

export type ProviderFeature = "completion" | "embedding" | "thinking" | "tools" | "vision";
export type LocalModelBinding = {
  provider: "ollama";
  endpoint: string;
  modelName: string;
  modelDigest: string;
  inspectionDigest: string;
};
export type LocalModelActivationReview = {
  binding: LocalModelBinding;
  providerFeatures: readonly ProviderFeature[];
  unrecognizedFeatures: readonly string[];
};
export type LocalModelActivation = LocalModelActivationReview & {
  id: string;
  workspaceId: string;
  approvedFeatures: readonly ProviderFeature[];
  state: "active" | "inactive";
  predecessorId: string | null;
  decidedAtMs: number;
};

export type BeaconCapability =
  | "work.create_mission"
  | "work.form_team"
  | "work.coordinate_mission"
  | "work.assign_work"
  | "artifact.produce"
  | "artifact.evaluate"
  | "evidence.develop";
export type BeaconCapabilityDescriptor = {
  capability: BeaconCapability;
  label: string;
  description: string;
};
export type TeamIneligibilityReason =
  | "configuration_cleared"
  | "resident_inactive"
  | "resident_suspended"
  | "resident_retired"
  | "model_activation_unavailable";
export type ResidentConfigurationBinding = {
  activationId: string;
  capabilities: readonly BeaconCapability[];
  budgetBoundary: "no_spend";
};
export type ResidentConfiguration = {
  id: string;
  workspaceId: string;
  residentId: string;
  state: "configured" | "cleared";
  binding: ResidentConfigurationBinding | null;
  predecessorId: string | null;
  decidedAtMs: number;
  eligibleForTeam: boolean;
  ineligibilityReasons: readonly TeamIneligibilityReason[];
};
export type TeamFormationActor = "operator";
export type TeamMemberDraft = {
  residentId: string;
  expectedConfigurationId: string;
};
export type TeamDraft = {
  projectId: string;
  name: string;
  members: TeamMemberDraft[];
};
export type TeamMemberView = {
  residentId: string;
  formationConfigurationId: string;
  currentConfigurationId: string;
  eligibleNow: boolean;
  ineligibilityReasons: readonly TeamIneligibilityReason[];
};
export type TeamView = {
  id: string;
  workspaceId: string;
  projectId: string;
  name: string;
  formedBy: TeamFormationActor;
  formedAtMs: number;
  members: TeamMemberView[];
};
export type MissionAdmissionActor = "operator";
export type MissionLifecycle = "admitted";
export type MissionMemberDraft = {
  residentId: string;
  expectedConfigurationId: string;
};
export type MissionDraft = {
  projectId: string;
  expectedCharterRevision: number;
  teamId: string;
  desiredOutcome: string;
  successConditions: string[];
  constraints: string[];
  prohibitions: string[];
  members: MissionMemberDraft[];
};
export type MissionMemberView = {
  residentId: string;
  admissionConfigurationId: string;
};
export type MissionView = {
  id: string;
  workspaceId: string;
  projectId: string;
  charterRevision: number;
  teamId: string;
  desiredOutcome: string;
  successConditions: string[];
  constraints: string[];
  prohibitions: string[];
  lifecycle: MissionLifecycle;
  admittedBy: MissionAdmissionActor;
  admittedAtMs: number;
  members: MissionMemberView[];
};

// Tauri's current frontend command adapter returns a Promise for the Rust value.
// Source: https://v2.tauri.app/develop/calling-rust/#commands
export async function loadSessionStatus(): Promise<DesktopSessionStatus> {
  return invokeAndValidate("session_status", undefined, isDesktopSessionStatus, "session status");
}

export async function loadSessionPolicy(): Promise<SessionPolicy> {
  return invokeAndValidate("session_policy", undefined, isSessionPolicy, "session policy");
}

export async function setLocalSessionTimeout(
  inactivityTimeoutMinutes: number,
): Promise<SessionPolicy> {
  return invokeAndValidate(
    "set_session_timeout",
    { inactivityTimeoutMinutes },
    isSessionPolicy,
    "session policy",
  );
}

export async function recordLocalOperatorActivity(): Promise<void> {
  const value: unknown = await invoke("record_operator_activity");
  if (value !== null) throw new Error("Beacon received an invalid activity response.");
}

export async function loadPlatformSessionMonitorStatus(): Promise<PlatformSessionMonitorStatus> {
  return invokeAndValidate(
    "platform_session_monitor_status",
    undefined,
    isPlatformSessionMonitorStatus,
    "platform session monitor status",
  );
}

export async function loadAuthenticationCapabilities(): Promise<AuthenticationCapabilities> {
  return invokeAndValidate(
    "authentication_capabilities",
    undefined,
    isAuthenticationCapabilities,
    "authentication capabilities",
  );
}

export async function beginLocalPasswordSetup(
  password: string,
  confirmation: string,
): Promise<RecoveryKeyOutput> {
  return invokeAndValidate(
    "begin_password_setup",
    { password, confirmation },
    isRecoveryKeyOutput,
    "Recovery Key",
  );
}

export async function completeLocalOperatorSetup(
  recoverySaved: boolean,
): Promise<DesktopSessionStatus> {
  return invokeAndValidate(
    "complete_operator_setup",
    { recoverySaved },
    isDesktopSessionStatus,
    "session status",
  );
}

export async function signInWithLocalPassword(password: string): Promise<DesktopSessionStatus> {
  return invokeAndValidate(
    "authenticate_password",
    { password },
    isDesktopSessionStatus,
    "session status",
  );
}

export async function recoverLocalOperatorAccount(
  recoveryKey: string,
  newPassword: string,
  confirmation: string,
): Promise<RecoveryKeyOutput> {
  return invokeAndValidate(
    "recover_operator_account",
    { recoveryKey, newPassword, confirmation },
    isRecoveryKeyOutput,
    "Recovery Key",
  );
}

export async function lockLocalSession(): Promise<DesktopSessionStatus> {
  return invokeAndValidate("lock_session", undefined, isDesktopSessionStatus, "session status");
}

export async function loadStarterOrganization(): Promise<StarterOrganizationView> {
  return invokeAndValidate(
    "starter_organization",
    undefined,
    isStarterOrganizationView,
    "Starter Organization",
  );
}

export async function acceptStarterOrganization(
  proposalVersion: number,
  residents: StarterResidentDraft[],
): Promise<StarterOrganizationView> {
  return invokeAndValidate(
    "accept_starter_organization",
    { proposalVersion, residents },
    isStarterOrganizationView,
    "Starter Organization",
  );
}

export async function discardStarterOrganization(
  proposalVersion: number,
): Promise<StarterOrganizationView> {
  return invokeAndValidate(
    "discard_starter_organization",
    { proposalVersion },
    isStarterOrganizationView,
    "Starter Organization",
  );
}

export async function loadResidents(): Promise<ResidentAgentView[]> {
  return invokeAndValidate("residents", undefined, isResidentList, "Resident roster");
}

export async function activateResident(
  residentId: string,
  expectedDecisionId: string | null,
): Promise<ResidentAgentView> {
  return invokeResidentLifecycle("activate_resident", residentId, expectedDecisionId);
}

export async function suspendResident(
  residentId: string,
  expectedDecisionId: string | null,
): Promise<ResidentAgentView> {
  return invokeResidentLifecycle("suspend_resident", residentId, expectedDecisionId);
}

export async function reactivateResident(
  residentId: string,
  expectedDecisionId: string | null,
): Promise<ResidentAgentView> {
  return invokeResidentLifecycle("reactivate_resident", residentId, expectedDecisionId);
}

export async function retireResident(
  residentId: string,
  expectedDecisionId: string | null,
): Promise<ResidentAgentView> {
  return invokeResidentLifecycle("retire_resident", residentId, expectedDecisionId);
}

function invokeResidentLifecycle(
  command: string,
  residentId: string,
  expectedDecisionId: string | null,
): Promise<ResidentAgentView> {
  return invokeAndValidate(
    command,
    { residentId, expectedDecisionId },
    isResidentAgentView,
    "Resident lifecycle response",
  );
}

export async function loadResidentCapabilityCatalog(): Promise<BeaconCapabilityDescriptor[]> {
  return invokeAndValidate(
    "resident_capability_catalog",
    undefined,
    isResidentCapabilityCatalog,
    "Resident Capability catalog",
  );
}

export async function loadResidentConfigurations(): Promise<ResidentConfiguration[]> {
  return invokeAndValidate(
    "resident_configurations",
    undefined,
    isResidentConfigurationList,
    "Resident configuration list",
  );
}

export async function configureResident(
  residentId: string,
  expectedDecisionId: string | null,
  activationId: string,
  capabilities: readonly BeaconCapability[],
): Promise<ResidentConfiguration> {
  return invokeAndValidate(
    "configure_resident",
    { residentId, expectedDecisionId, activationId, capabilities: [...capabilities] },
    isResidentConfiguration,
    "Resident configuration",
  );
}

export async function clearResidentConfiguration(
  residentId: string,
  expectedDecisionId: string,
): Promise<ResidentConfiguration> {
  return invokeAndValidate(
    "clear_resident_configuration",
    { residentId, expectedDecisionId },
    isResidentConfiguration,
    "Resident configuration",
  );
}

export async function loadProjects(): Promise<ProjectSummary[]> {
  return invokeAndValidate("projects", undefined, isProjectSummaries, "Project list");
}

export async function loadSampleProjectStatus(): Promise<SampleProjectStatus> {
  return invokeAndValidate(
    "sample_project_status",
    undefined,
    isSampleProjectStatus,
    "Sample Project status",
  );
}

export async function installSampleProject(): Promise<SampleProjectInstallResult> {
  return invokeAndValidate(
    "install_sample_project",
    undefined,
    isSampleProjectInstallResult,
    "Sample Project installation",
  );
}

export async function loadProject(projectId: string): Promise<ProjectView> {
  return invokeAndValidate("project", { projectId }, isProjectView, "Project");
}

export async function loadProjectCharterHistory(projectId: string): Promise<ProjectCharter[]> {
  return invokeAndValidate(
    "project_charter_history",
    { projectId },
    isProjectCharterHistory,
    "Project Charter history",
  );
}

export async function createProject(draft: ProjectDraft): Promise<ProjectView> {
  return invokeAndValidate("create_project", { draft }, isProjectView, "Project");
}

export async function reviseProjectCharter(
  projectId: string,
  expectedCurrentRevision: number,
  draft: ProjectCharterDraft,
): Promise<ProjectView> {
  return invokeAndValidate(
    "revise_project_charter",
    { projectId, expectedCurrentRevision, draft },
    isProjectView,
    "Project",
  );
}

export async function loadProjectPolicy(projectId: string): Promise<ProjectPolicy | null> {
  return invokeAndValidate(
    "project_policy",
    { projectId },
    isProjectPolicyOrNull,
    "Project Policy",
  );
}

/// One row of a Mission's publication history, as the relation holds it.
export type PublicationSummary = {
  publicationId: string;
  publicationVersion: number;
  candidateArtifactDigest: string;
  evaluationDigest: string;
  isCurrent: boolean;
};

/// One published Outcome's exact stored Markdown. `bytes` is the document byte for byte -- never a
/// re-render, and never Markdown turned into HTML.
export type ExportedOutcome = {
  bytes: number[];
  digest: { algorithm: string; canonicalizer: string; value: string };
  publicationVersion: number;
  publicationVersionCount: number;
  mediaType: string;
};

export async function loadMissionPublications(missionId: string): Promise<PublicationSummary[]> {
  return invokeAndValidate(
    "mission_publications",
    { missionId },
    isPublicationHistory,
    "Outcome publication history",
  );
}

export async function loadOutcomeDocument(
  missionId: string,
  version: number | null,
): Promise<ExportedOutcome> {
  return invokeAndValidate(
    "outcome_document",
    { missionId, version },
    isExportedOutcome,
    "Outcome document",
  );
}

export async function loadProjectPolicyHistory(projectId: string): Promise<ProjectPolicy[]> {
  return invokeAndValidate(
    "project_policy_history",
    { projectId },
    isProjectPolicyHistory,
    "Project Policy history",
  );
}

export async function publishProjectPolicy(
  projectId: string,
  expectedCharterRevision: number,
  expectedPolicyRevision: number | null,
  draft: ProjectPolicyDraft,
): Promise<ProjectPolicy> {
  return invokeAndValidate(
    "publish_project_policy",
    { projectId, expectedCharterRevision, expectedPolicyRevision, draft },
    isProjectPolicy,
    "Project Policy",
  );
}

export async function loadProjectRepository(projectId: string): Promise<ProjectRepository | null> {
  return invokeAndValidate(
    "current_project_repository",
    { projectId },
    isProjectRepositoryOrNull,
    "Project Repository",
  );
}

export async function loadProjectRepositoryHistory(
  projectId: string,
): Promise<ProjectRepository[]> {
  return invokeAndValidate(
    "project_repository_history",
    { projectId },
    isProjectRepositoryHistory,
    "Project Repository history",
  );
}

export async function bindProjectRepository(
  projectId: string,
  expectedCharterRevision: number,
  expectedPriorRepositoryRevision: number | null,
  draft: RepositoryDraft,
): Promise<ProjectRepository> {
  return invokeAndValidate(
    "bind_project_repository",
    { projectId, expectedCharterRevision, expectedPriorRepositoryRevision, draft },
    isProjectRepository,
    "Project Repository",
  );
}

export async function preflightProjectChecks(projectId: string): Promise<PreflightReceipt> {
  return invokeAndValidate(
    "preflight_project_checks",
    { projectId },
    isPreflightReceipt,
    "preflight Receipt",
  );
}

/** Every Receipt kind, for the Workspace, newest first, under a bound the
 *  daemon owns. The Project-scoped Query this replaces made Briefing choose
 *  which Projects to open, in what order, how many rows to hold, and how to
 *  merge them -- a domain join, a bound and an ordering policy in a UI app. */
export async function recentReceipts(window: number): Promise<RecentReceiptPage> {
  return invokeAndValidate(
    "recent_receipts",
    { window },
    isRecentReceiptPage,
    "recent Receipt page",
  );
}

export async function loadTeams(projectId: string): Promise<TeamView[]> {
  return invokeAndValidate("teams", { projectId }, isTeamList, "Team list");
}

export async function loadTeam(teamId: string): Promise<TeamView> {
  return invokeAndValidate("team", { teamId }, isTeamView, "Team");
}

export async function formTeam(draft: TeamDraft): Promise<TeamView> {
  return invokeAndValidate("form_team", { draft }, isTeamView, "Team");
}

export async function loadMissions(projectId: string): Promise<MissionView[]> {
  return invokeAndValidate("missions", { projectId }, isMissionList, "Mission list");
}

export async function loadMission(missionId: string): Promise<MissionView> {
  return invokeAndValidate("mission", { missionId }, isMissionView, "Mission");
}

export async function admitMission(draft: MissionDraft): Promise<MissionView> {
  return invokeAndValidate("admit_mission", { draft }, isMissionView, "Mission");
}

export async function discoverLocalModels(): Promise<LocalModelDiscoveryReport> {
  return invokeAndValidate(
    "discover_local_models",
    undefined,
    isLocalModelDiscoveryReport,
    "local-model discovery report",
  );
}

export async function liveDataEncryptionStatus(): Promise<LiveDataEncryptionReport> {
  return invokeAndValidate(
    "live_data_encryption_status",
    undefined,
    isLiveDataEncryptionReport,
    "live-data encryption report",
  );
}

export async function inspectLocalModel(
  modelName: string,
  expectedDigest: string,
): Promise<LocalModelActivationReview> {
  return invokeAndValidate(
    "inspect_local_model",
    { modelName, expectedDigest },
    isLocalModelActivationReview,
    "local-model activation review",
  );
}

export async function activateLocalModel(
  review: LocalModelActivationReview,
  approvedFeatures: readonly ProviderFeature[],
): Promise<LocalModelActivation> {
  return invokeAndValidate(
    "activate_local_model",
    { review, approvedFeatures: [...approvedFeatures] },
    isLocalModelActivation,
    "local-model activation",
  );
}

export async function loadLocalModelActivations(): Promise<LocalModelActivation[]> {
  return invokeAndValidate(
    "local_model_activations",
    undefined,
    isLocalModelActivationList,
    "local-model activation list",
  );
}

export async function deactivateLocalModel(
  expectedActivationId: string,
): Promise<LocalModelActivation> {
  return invokeAndValidate(
    "deactivate_local_model",
    { expectedActivationId },
    isLocalModelActivation,
    "local-model activation",
  );
}

/** True when the daemon refused with the shared `preflight_unavailable` code.
 *
 *  The recent-Receipts Query reuses that code rather than inventing one, but
 *  its shared sentence -- "Beacon could not record the preflight" -- is false
 *  for a Query that records nothing. Callers that need a truthful sentence
 *  translate at their own seam; the preflight command keeps the specific one. */
export function isPreflightUnavailable(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    !Array.isArray(error) &&
    (error as Record<string, unknown>).code === "preflight_unavailable"
  );
}

/** The failure exactly as it arrived, for a report that must show BOTH the
 *  fallback and the failure that caused it. `describeLocalAccountError` maps
 *  known codes to operator copy and answers "could not complete that local
 *  account action" for everything else -- which is the right default for a
 *  transient action error and the wrong one for a diagnostic whose whole job is
 *  to say what it could not read. */
export function verbatimFailure(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  if (typeof error === "object" && error !== null) {
    // The MESSAGE first. A structured error's `code` is a label for the class;
    // the message is the only text that says what actually failed, and
    // preferring the label discarded it -- the same substitution this
    // diagnostic exists to avoid, one level up.
    const message = (error as Record<string, unknown>).message;
    if (typeof message === "string" && message) return message;
    const code = (error as Record<string, unknown>).code;
    if (typeof code === "string" && code) return code;
  }
  return "no reason was reported";
}

/** The report for a Query that did not answer.
 *
 *  A diagnostic that could not run is not a diagnostic that said "encrypted",
 *  and the operator has to be able to see the failure and retry it. It used to
 *  reach the panel as a free-text `failure` string beside a missing report --
 *  unbounded, outside the closed vocabulary, and rendered by a second code
 *  path. It is a report now: a closed reason, a bounded detail, and the
 *  original failure text as flagged evidence, so fidelity survives where it
 *  fits and the flag says so where it does not.
 */
export function failedReadReport(error: unknown, observedAtMs: number): LiveDataEncryptionReport {
  const [observation, truncated] = boundedText(verbatimFailure(error), MAX_OBSERVATION_CHARS);
  const [detail, shortened] = boundedText(verbatimFailure(error), MAX_SUBJECT_CHARS);
  return {
    conclusion: {
      state: stateForReason("initial_read_failed"),
      reason: "initial_read_failed",
      subjects: [{ role: "detail", value: detail, shortened }],
      omittedSubjects: 0,
    },
    mountPoint: null,
    evidence: [
      { source: "beacon: live-data encryption reading", observation, truncated },
    ],
    omittedEvidence: 0,
    notVerified: [],
    observedAtMs,
  };
}

/** Cleaned and cut the way the reader cuts, so a client-built report satisfies
 *  the same shape a read one does. */
function boundedText(value: string, maxCharacters: number): [string, boolean] {
  const cleaned = value.replace(/\p{Cc}/gu, " ").trim();
  const characters = Array.from(cleaned);
  if (characters.length === 0) return ["(empty)", false];
  if (characters.length <= maxCharacters) return [cleaned, false];
  const window = characters.slice(0, maxCharacters).join("").trim();
  return [window.length === 0 ? "(empty)" : window, true];
}

export function describeLocalAccountError(error: unknown): string {
  if (typeof error !== "object" || error === null || Array.isArray(error)) {
    return "Beacon could not complete that local request.";
  }
  const code = (error as Record<string, unknown>).code;
  const retry = (error as Record<string, unknown>).retryAfterMs;
  switch (code) {
    case "password_too_short":
      return "Use at least 15 characters.";
    case "password_too_long":
      return "Use no more than 1,024 characters.";
    case "password_confirmation_mismatch":
      return "The password confirmation does not match.";
    case "recovery_not_confirmed":
      return "Confirm that the Recovery Key was saved offline.";
    case "authentication_failed":
      return "That credential was not accepted.";
    case "authentication_throttled":
      return typeof retry === "number" && Number.isFinite(retry)
        ? `Wait ${Math.max(1, Math.ceil(retry / 1000))} seconds before trying again.`
        : "Wait before trying again.";
    case "credential_disabled":
      return "Use the other local credential to recover this account.";
    case "invalid_session_timeout":
      return "Choose a timeout from 1 to 1,440 minutes.";
    case "unsupported_starter_proposal":
      return "The proposal changed. Reload it before continuing.";
    case "invalid_starter_roster":
      return "Select each role once and use Display Names from 1 to 80 characters.";
    case "starter_decision_recorded":
      return "This Starter Organization was already decided.";
    case "starter_organization_unavailable":
      return "Beacon could not verify the Starter Organization state.";
    case "resident_not_found":
      return "That Resident is no longer available in this Workspace.";
    case "invalid_resident_transition":
      return "That lifecycle action is not available from the Resident's current state.";
    case "stale_resident_decision":
      return "The Resident changed. Reload the roster before trying again.";
    case "resident_lifecycle_unavailable":
      return "Beacon could not verify Resident lifecycle state.";
    case "invalid_resident_configuration":
      return "Review the Resident, active model, completion approval, and Capability selection.";
    case "stale_resident_configuration":
      return "The Resident configuration changed. Reload it before trying again.";
    case "resident_configuration_unavailable":
      return "Beacon could not verify Resident configuration state.";
    case "invalid_project":
      return "Review the Project name and every Charter field.";
    case "project_not_found":
      return "That Project is no longer available in this Workspace.";
    case "stale_charter_revision":
      return "This Charter changed. Reload the current revision before publishing.";
    case "unchanged_charter":
      return "Change at least one Charter field before publishing a revision.";
    case "project_unavailable":
      return "Beacon could not verify the Project state.";
    case "invalid_project_policy":
      return "Review the Lead capabilities and every Track Record evidence threshold.";
    case "stale_project_policy":
      return "This Policy changed. Reload the current revision before publishing.";
    case "unchanged_project_policy":
      return "Change at least one Team Lead selection criterion before publishing.";
    case "project_policy_unavailable":
      return "Beacon could not verify the Project Policy state.";
    case "invalid_project_repository":
      return "Review the repository source, the base revision, and every declared check.";
    case "stale_project_repository":
      return "This binding changed. Reload the current revision before binding.";
    case "unchanged_project_repository":
      return "Change the source, the base revision, or a declared check before binding.";
    case "project_repository_unavailable":
      return "Beacon could not verify the Project Repository state.";
    case "invalid_team":
      return "Choose a Project, a Team name, and distinct eligible Residents.";
    case "team_not_found":
      return "That Team is no longer available in this Workspace.";
    case "stale_team_member_configuration":
      return "A Resident configuration changed. Reload the Team form before trying again.";
    case "team_member_ineligible":
      return "A selected Resident is no longer eligible for Team membership.";
    case "team_unavailable":
      return "Beacon could not verify Team state.";
    case "repository_not_bound":
      return "Bind a repository to this Project before preflighting its checks.";
    case "preflight_unavailable":
      return "Beacon could not record the preflight.";
    case "invalid_mission":
      return "Review the Mission outcome, success conditions, Team, and exact member evidence.";
    case "mission_not_found":
      return "That Mission is no longer available in this Workspace.";
    case "not_authenticated":
      return "Unlock Beacon to continue.";
    case "account_service_unavailable":
      return "The local account service is unavailable.";
    case "outcome_absent":
      return "This Mission has no published Outcome Document.";
    case "outcome_unverified":
      return "Beacon could not verify this Mission's Outcome Document.";
    case "stale_mission_team":
      return "The Team or a Resident configuration changed. Reload before admitting the Mission.";
    case "mission_member_ineligible":
      return "A Team member is no longer eligible for Mission admission.";
    case "mission_unavailable":
      return "Beacon could not verify Mission state.";
    case "local_model_unavailable":
      return "The local model provider could not be verified.";
    case "local_model_not_found":
      return "That exact local model is no longer installed.";
    case "local_model_changed":
      return "The local model changed. Review it again before activation.";
    case "invalid_local_model_approval":
      return "Select at least one advertised feature and review the model again.";
    case "local_model_already_active":
      return "This local model is already active in the Workspace.";
    case "stale_local_model_decision":
      return "The activation changed. Reload it before deactivating.";
    case "local_model_activation_unavailable":
      return "Beacon could not verify local-model activation state.";
    default:
      return "Beacon could not complete that local request.";
  }
}

/** This Mission's verified completion, or `null` when the Workspace recorded none.
 *
 *  Absence is a value the owner returns; a failed or refused read REJECTS and is never converted to
 *  null, because "nothing was recorded" and "we could not tell" are different answers. The returned
 *  `missionId` is compared against the one asked for: a payload about another Mission is a refusal,
 *  not something to render under this heading. */
export async function loadMissionCompletion(missionId: string): Promise<CompletionView | null> {
  const value = await invokeAndValidate(
    "get_mission_completion",
    { missionId },
    isCompletionViewOrAbsent,
    "Mission completion",
  );
  if (value !== null && value.missionId !== missionId) {
    throw new Error("Beacon received an invalid Mission completion.");
  }
  return value;
}

// ---------------------------------------------------------------------------------------------
// C5: the Mission impact classification surface (local candidate; no owner behind it yet)
// ---------------------------------------------------------------------------------------------

/** The Policy revision a classification cites, with that Policy revision's OWN Charter revision --
 *  never the Project's current one (`mission_impact.rs`'s `PolicyRevisionRef`). */
export type PolicyCitation = {
  projectId: string;
  policyRevision: number;
  charterRevision: number;
};

export type ClassificationDigestValue = {
  algorithm: string;
  canonicalizer: string;
  value: string;
};

/** Which record a classification replaces. Tagged, because "replacing nothing" and "replacing this"
 *  are two statements rather than one absence. */
export type ClassificationSupersedes =
  | { kind: "none" }
  | { kind: "record"; predecessorDigest: string };

/** The twelve fields of the owner's `ClassificationView` (C5-interface.md). `basis` and `ruleId` are
 *  PROJECTED from the stored record: the old public store admits a `policy_rule` row with a rule id
 *  (`mission_impact.rs:332-343`), and a verified legacy row must keep its own basis and rule
 *  identity rather than being relabelled. No new write ever sends either. */
export type ClassificationView = {
  missionId: string;
  classificationId: string;
  version: number;
  impact: "high" | "ordinary";
  basis: "operator_declared" | "policy_rule";
  ruleId: string | null;
  reason: string;
  policy: PolicyCitation;
  classifiedBy: "operator";
  classifiedAtMs: number;
  supersedes: ClassificationSupersedes;
  digest: ClassificationDigestValue;
};

/** One snapshot: where it was taken, the Policy a NEW classification would cite, and the highest
 *  proven record at that cursor. No count, no history, no "current impact" verdict. */
export type ClassificationObservation = {
  missionId: string;
  /** A canonical nonnegative decimal STRING in the u64 domain. NEVER converted to a JS number: a
   *  sequence above 2**53 would round, and a cursor that rounds is a cursor that lies. */
  observedAtEventSeq: string;
  publishedPolicy: PolicyCitation | null;
  latest: ClassificationView | null;
};

/** What the Operator decides, and nothing else: five keys, `expectedPolicyRevision` present-nullable,
 *  no basis, no ruleId, no identity, no clock, no Policy provenance. */
export type ClassificationIntentRequest = {
  missionId: string;
  impact: "high" | "ordinary";
  reason: string;
  expectedPolicyRevision: number | null;
  supersedes: ClassificationSupersedes;
};

const CLASSIFICATION_REASON_SCALARS = 2_000;
const U64_MAX = 18_446_744_073_709_551_615n;

/** The cursor's canonical form: digits only, no sign, no space, no leading zero unless the value IS
 *  zero, and inside the u64 domain. Compared with `BigInt` because the whole point is that the value
 *  does not fit a JS number. */
function isCanonicalEventSequence(value: unknown): value is string {
  if (typeof value !== "string" || !/^(?:0|[1-9][0-9]{0,19})$/.test(value)) return false;
  return BigInt(value) <= U64_MAX;
}

/** Classification counts and revisions are lossless `u32` in the owner's codec. Deliberately not a
 *  narrowing of the shared `isNonnegativeInteger`, which also guards millisecond instants. */
function isClassificationU32(value: unknown): value is number {
  return isNonnegativeInteger(value) && value <= MAX_U32_COUNT;
}

function isPolicyCitation(value: unknown): value is PolicyCitation {
  return (
    hasExactKeys(value, ["charterRevision", "policyRevision", "projectId"]) &&
    isUuid(value.projectId) &&
    isClassificationU32(value.policyRevision) &&
    isClassificationU32(value.charterRevision)
  );
}

function isClassificationSupersedes(value: unknown): value is ClassificationSupersedes {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  if (record.kind === "none") return hasExactKeys(value, ["kind"]);
  if (record.kind !== "record") return false;
  return (
    hasExactKeys(value, ["kind", "predecessorDigest"]) && isHex64(record.predecessorDigest)
  );
}

function isClassificationView(value: unknown): value is ClassificationView {
  if (
    !hasExactKeys(value, [
      "basis",
      "classificationId",
      "classifiedAtMs",
      "classifiedBy",
      "digest",
      "impact",
      "missionId",
      "policy",
      "reason",
      "ruleId",
      "supersedes",
      "version",
    ])
  ) {
    return false;
  }
  if (!isUuid(value.missionId) || !isUuid(value.classificationId)) return false;
  if (!isClassificationU32(value.version) || value.version < 1) return false;
  if (value.impact !== "high" && value.impact !== "ordinary") return false;
  // A legacy `policy_rule` row carries its rule id; an operator-declared one must not.
  if (value.basis === "operator_declared") {
    if (value.ruleId !== null) return false;
  } else if (value.basis === "policy_rule") {
    if (typeof value.ruleId !== "string" || value.ruleId === "") return false;
  } else {
    return false;
  }
  if (!isTrimmedText(value.reason, CLASSIFICATION_REASON_SCALARS)) return false;
  if (!isPolicyCitation(value.policy)) return false;
  if (value.classifiedBy !== "operator") return false;
  if (!isNonnegativeInteger(value.classifiedAtMs)) return false;
  if (!isClassificationSupersedes(value.supersedes)) return false;
  const digest = value.digest;
  return (
    hasExactKeys(digest, ["algorithm", "canonicalizer", "value"]) &&
    digest.algorithm === "sha256" &&
    digest.canonicalizer === "beacon.mission-impact.v1" &&
    isHex64(digest.value)
  );
}

function isClassificationObservation(value: unknown): value is ClassificationObservation {
  return (
    hasExactKeys(value, ["latest", "missionId", "observedAtEventSeq", "publishedPolicy"]) &&
    isUuid(value.missionId) &&
    isCanonicalEventSequence(value.observedAtEventSeq) &&
    (value.publishedPolicy === null || isPolicyCitation(value.publishedPolicy)) &&
    (value.latest === null || isClassificationView(value.latest))
  );
}

/** Record one classification. The Operator's five keys go out unchanged; the owner derives identity,
 *  version, the Policy citation, the actor and the clock, and fixes `operator_declared` with no rule.
 *
 *  `classify_mission_impact` is wired to `MissionImpactCommandService::classify`. The COMPONENT tests
 *  for this surface mock this boundary and do not execute the real owner. */
export async function recordMissionClassification(
  request: ClassificationIntentRequest,
): Promise<ClassificationView> {
  const value = await invokeAndValidate(
    "classify_mission_impact",
    { request },
    isClassificationView,
    "Mission classification",
  );
  if (value.missionId !== request.missionId) {
    throw new Error("Beacon received an invalid Mission classification.");
  }
  return value;
}

/** One snapshot of this Mission's classification. A read; it writes nothing.
 *
 *  `get_mission_classification` is wired to `MissionImpactCommandService::observe`. The COMPONENT
 *  tests for this surface mock this boundary and do not execute the real owner; the owner and the
 *  transport have their own bars. */
export async function loadMissionClassification(
  missionId: string,
): Promise<ClassificationObservation> {
  const value = await invokeAndValidate(
    "get_mission_classification",
    { missionId },
    isClassificationObservation,
    "Mission classification",
  );
  // The outer id AND the nested record's: an observation whose `latest` belongs to another Mission is
  // incoherent, and rendering it would show one Mission's record under another's heading. Refused at
  // the same validation boundary as every other shape fault, not repaired.
  if (value.missionId !== missionId || (value.latest !== null && value.latest.missionId !== missionId)) {
    throw new Error("Beacon received an invalid Mission classification.");
  }
  return value;
}

// The OWNER's bounds, not this client's. `mission_completion.rs:128-133` fixes short text at 500
// Unicode scalar values and applies it to a condition note; `isTrimmedText` counts with
// `Array.from`, so astral text is one scalar per character rather than two UTF-16 units. A larger
// cap here would accept a record the owner would refuse and call it recorded content.
const COMPLETION_SHORT_TEXT_SCALARS = 500;
const COMPLETION_MAX_CONDITIONS = 20;
const COMPLETION_MAX_SUPPORT_PER_CONDITION = 8;
const COMPLETION_MAX_DELIVERED_ARTIFACTS = 32;
const COMPLETION_MAX_NARRATIVE_ITEMS = 32;

const COMPLETION_OUTCOMES = ["met", "not_met", "not_assessed", "assessment_inconclusive"] as const;

function isCompletionConditionResult(value: unknown): value is CompletionConditionResult {
  return (
    hasExactKeys(value, ["note", "ordinal", "outcome", "supportCount"]) &&
    isCompletionU32(value.ordinal) &&
    typeof value.outcome === "string" &&
    (COMPLETION_OUTCOMES as readonly string[]).includes(value.outcome) &&
    isCompletionU32(value.supportCount) &&
    value.supportCount <= COMPLETION_MAX_SUPPORT_PER_CONDITION &&
    (value.note === null || isCompletionText(value.note, COMPLETION_SHORT_TEXT_SCALARS))
  );
}

function isCompletionViewOrAbsent(value: unknown): value is CompletionView | null {
  if (value === null) return true;
  if (
    !hasExactKeys(value, [
      "accountableResidentConfigurationId",
      "accountableResidentId",
      "actor",
      "completionId",
      "conditionResults",
      "deliveredArtifactCount",
      "disposition",
      "missionId",
      "unfinishedWorkCount",
    ]) ||
    !isUuid(value.missionId) ||
    !isUuid(value.completionId) ||
    value.actor !== "operator" ||
    !isUuid(value.accountableResidentId) ||
    !isUuid(value.accountableResidentConfigurationId) ||
    (value.disposition !== "concluded" && value.disposition !== "abandoned") ||
    !isCompletionU32(value.deliveredArtifactCount) ||
    value.deliveredArtifactCount > COMPLETION_MAX_DELIVERED_ARTIFACTS ||
    !isCompletionU32(value.unfinishedWorkCount) ||
    value.unfinishedWorkCount > COMPLETION_MAX_NARRATIVE_ITEMS ||
    !Array.isArray(value.conditionResults) ||
    value.conditionResults.length > COMPLETION_MAX_CONDITIONS ||
    !value.conditionResults.every(isCompletionConditionResult)
  ) {
    return false;
  }
  // A repeated ordinal would make one condition's result ambiguous. Refused here rather than
  // silently collapsed, because dropping one is exactly the omission the contract forbids.
  const ordinals = new Set(value.conditionResults.map((result) => result.ordinal));
  return ordinals.size === value.conditionResults.length;
}

// The owner's limits, cited to `mission_completion.rs:124-133`. The frontend refuses what the owner
// would refuse; it is never the authority on canonical size or on what a citation means.
const COMPLETION_MAX_SUMMARY_SCALARS = 2_000;

/** A completion count, version or ordinal is a lossless `u32` in the owner's codec, so `2**32`
 *  is out of domain even though it is a safe JS integer. Deliberately NOT a narrowing of the shared
 *  `isNonnegativeInteger`: that helper also guards millisecond instants (`createdAtMs`,
 *  `admittedAtMs`, ...) for unrelated commands, where a u32 ceiling would refuse correct values. */
function isCompletionU32(value: unknown): value is number {
  return isNonnegativeInteger(value) && value <= MAX_U32_COUNT;
}

/** The owner's completion text grammar, `require_text` at `mission_completion.rs:489-506`: nonempty,
 *  trimmed, at most `maxScalars` Unicode scalars, and no control character other than LF or TAB,
 *  which the owner treats as content. Neither shared helper is this grammar -- `isTrimmedText`
 *  accepts every control character and `isTrimmedTextWithoutControls` refuses the LF and TAB the
 *  owner allows -- and changing either would change unrelated commands. */
function isCompletionText(value: unknown, maxScalars: number): value is string {
  return (
    isTrimmedText(value, maxScalars) &&
    !Array.from(value).some(
      (character) => /\p{Cc}/u.test(character) && character !== "\n" && character !== "\t",
    )
  );
}

function isHex64(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function isSupportRefDto(value: unknown): value is SupportRefDto {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  if (keys.length !== 1) return false;
  const record = value as Record<string, unknown>;
  if (keys[0] === "artifact") return isHex64(record.artifact);
  if (keys[0] === "evaluation") return isHex64(record.evaluation);
  if (keys[0] === "ruling") return isHex64(record.ruling);
  if (keys[0] !== "receipt") return false;
  const receipt = record.receipt;
  return (
    hasExactKeys(receipt, ["receiptBytesSha256", "receiptVersion", "runId"]) &&
    isUuid(receipt.runId) &&
    isCompletionU32(receipt.receiptVersion) &&
    isHex64(receipt.receiptBytesSha256)
  );
}

function isNarrativeInput(value: unknown, kinds: readonly string[]): boolean {
  return (
    hasExactKeys(value, ["conditionOrdinal", "kind", "ordinal", "summary"]) &&
    isCompletionU32(value.ordinal) &&
    typeof value.kind === "string" &&
    kinds.includes(value.kind) &&
    (value.conditionOrdinal === null || isCompletionU32(value.conditionOrdinal)) &&
    isCompletionText(value.summary, COMPLETION_SHORT_TEXT_SCALARS)
  );
}

function isSpendReferenceDto(value: unknown): value is SpendReferenceDto {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value);
  if (keys.length !== 1) return false;
  const record = value as Record<string, unknown>;
  if (keys[0] === "notMeasured") {
    return isCompletionText(record.notMeasured, COMPLETION_SHORT_TEXT_SCALARS);
  }
  if (keys[0] !== "recorded") return false;
  const recorded = record.recorded;
  return (
    hasExactKeys(recorded, ["spendId", "spendSha256", "spendVersion"]) &&
    isUuid(recorded.spendId) &&
    isCompletionU32(recorded.spendVersion) &&
    isHex64(recorded.spendSha256)
  );
}

const UNFINISHED_KINDS = [
  "condition_not_met",
  "deferred_scope",
  "known_defect",
  "blocked_dependency",
] as const;
const UNCERTAINTY_KINDS = [
  "unverified_claim",
  "unmeasured_quantity",
  "conflicting_evidence",
  "outside_scope_of_checks",
] as const;

function isCompletionIntentRequest(value: unknown): value is CompletionIntentRequest {
  return (
    hasExactKeys(value, [
      "accountableResidentId",
      "conditionResults",
      "deliveredArtifacts",
      "disposition",
      "missionId",
      "spend",
      "summary",
      "uncertainty",
      "unfinishedWork",
    ]) &&
    isUuid(value.missionId) &&
    isUuid(value.accountableResidentId) &&
    (value.disposition === "concluded" || value.disposition === "abandoned") &&
    Array.isArray(value.conditionResults) &&
    value.conditionResults.length >= 1 &&
    value.conditionResults.length <= COMPLETION_MAX_CONDITIONS &&
    value.conditionResults.every(
      (result: unknown) =>
        hasExactKeys(result, ["note", "ordinal", "outcome", "support"]) &&
        isCompletionU32(result.ordinal) &&
        typeof result.outcome === "string" &&
        (COMPLETION_OUTCOMES as readonly string[]).includes(result.outcome) &&
        Array.isArray(result.support) &&
        result.support.length <= COMPLETION_MAX_SUPPORT_PER_CONDITION &&
        result.support.every(isSupportRefDto) &&
        (result.note === null || isCompletionText(result.note, COMPLETION_SHORT_TEXT_SCALARS)),
    ) &&
    Array.isArray(value.deliveredArtifacts) &&
    value.deliveredArtifacts.length <= COMPLETION_MAX_DELIVERED_ARTIFACTS &&
    value.deliveredArtifacts.every(isHex64) &&
    Array.isArray(value.unfinishedWork) &&
    value.unfinishedWork.length <= COMPLETION_MAX_NARRATIVE_ITEMS &&
    value.unfinishedWork.every((item: unknown) => isNarrativeInput(item, UNFINISHED_KINDS)) &&
    Array.isArray(value.uncertainty) &&
    value.uncertainty.length <= COMPLETION_MAX_NARRATIVE_ITEMS &&
    value.uncertainty.every((item: unknown) => isNarrativeInput(item, UNCERTAINTY_KINDS)) &&
    isSpendReferenceDto(value.spend) &&
    isCompletionText(value.summary, COMPLETION_MAX_SUMMARY_SCALARS)
  );
}

function isCompletionPreview(value: unknown): value is CompletionPreview {
  return (
    hasExactKeys(value, [
      "accountableResidentConfigurationId",
      "actor",
      "conditions",
      "declaredConditionsSha256",
      "intentSha256",
      "projectId",
      "request",
      "workspaceId",
    ]) &&
    isCompletionIntentRequest(value.request) &&
    isHex64(value.intentSha256) &&
    isUuid(value.workspaceId) &&
    isUuid(value.projectId) &&
    value.actor === "operator" &&
    isUuid(value.accountableResidentConfigurationId) &&
    isHex64(value.declaredConditionsSha256) &&
    Array.isArray(value.conditions) &&
    value.conditions.every((condition: unknown) => typeof condition === "string")
  );
}

function isIntentAcknowledgement(value: unknown): value is { intentSha256: string } {
  return hasExactKeys(value, ["intentSha256"]) && isHex64(value.intentSha256);
}

function isIntentTokenOrAbsent(value: unknown): value is { intentSha256: string } | null {
  return value === null || isIntentAcknowledgement(value);
}

/** Derive the owner's normalized record for review. Writes nothing. */
export async function previewMissionCompletion(
  request: CompletionIntentRequest,
): Promise<CompletionPreview> {
  return invokeAndValidate(
    "preview_mission_completion",
    { request },
    isCompletionPreview,
    "Mission completion preview",
  );
}

/** Record the reviewed intent. The echoed token is a consistency check on authenticated input --
 *  not proof a screen was viewed, and not an idempotency key. */
/** B15 F1: the completion command refuses a Mission that has no classification.
 *
 *  Named here so the surface and its bars share ONE spelling of the wire code. A completion recorded
 *  before a classification would freeze a cut no classification was current at, and a classification
 *  added afterwards cannot repair that cut -- so the order is required, not warned about. */
export const COMPLETION_CLASSIFICATION_REQUIRED = "completion_classification_required";

export async function recordMissionCompletion(
  request: CompletionIntentRequest,
  reviewedIntentSha256: string,
): Promise<{ intentSha256: string }> {
  return invokeAndValidate(
    "record_mission_completion",
    { request, reviewedIntentSha256 },
    isIntentAcknowledgement,
    "Mission completion acknowledgement",
  );
}

/** Recompute the stored completion's consistency token, for recovery. `null` means no completion is
 *  visible in THAT snapshot -- never proof that a pending write cannot still arrive. */
export async function getMissionCompletionIntent(
  missionId: string,
): Promise<{ intentSha256: string } | null> {
  return invokeAndValidate(
    "get_mission_completion_intent",
    { missionId },
    isIntentTokenOrAbsent,
    "Mission completion intent",
  );
}

/** One digest, whole: never a bare hex string. Which canonicalizer produced it is part of the
 *  identity, so this surface holds the exact expected canonicalizer for each of the three. */
export type OutcomeDigestValue = {
  algorithm: string;
  canonicalizer: string;
  value: string;
};

/** What the Operator asked for. Two arms, no `omit`: a caller must not be able to suppress the
 *  verification appendix by asking. */
export type AppendixRequest = "policy_default" | "include";

/** What that request resolved to against the verified Policy classification. Derived by the owner
 *  from the manifest, never a stored column. */
export type AppendixMode = "included" | "policy_state_not_high_impact";

/** The nine fields of C6's preparation receipt. Metadata only: no document, no verdict, no
 *  permission, and deliberately no `isEvaluated`/`isPublished`/`isCurrent` bit -- preparation asks
 *  neither of those owners, so this surface cannot answer for them. */
export type PreparedOutcomeCandidate = {
  missionId: string;
  appendixRequest: AppendixRequest;
  appendixMode: AppendixMode;
  manifestId: string;
  manifestDigest: OutcomeDigestValue;
  candidateArtifactDigest: OutcomeDigestValue;
  candidateContentDigest: OutcomeDigestValue;
  candidateContentBytes: number;
  mediaType: string;
};

/** The owner's own content bound (`artifact.rs`, MAX_CONTENT_BYTES). The byte count is safe as a JS
 *  number because THIS domain is a mebibyte -- not because a `u64` is safe on this boundary. */
const MAX_CANDIDATE_CONTENT_BYTES = 1_048_576;
const CANDIDATE_MEDIA_TYPE = "text/markdown; charset=utf-8";

function isOutcomeDigest(value: unknown, canonicalizer: string): value is OutcomeDigestValue {
  return (
    hasExactKeys(value, ["algorithm", "canonicalizer", "value"]) &&
    value.algorithm === "sha256" &&
    value.canonicalizer === canonicalizer &&
    isHex64(value.value)
  );
}

function isPreparedOutcomeCandidate(value: unknown): value is PreparedOutcomeCandidate {
  if (
    !hasExactKeys(value, [
      "appendixMode",
      "appendixRequest",
      "candidateArtifactDigest",
      "candidateContentBytes",
      "candidateContentDigest",
      "manifestDigest",
      "manifestId",
      "mediaType",
      "missionId",
    ])
  ) {
    return false;
  }
  if (!isUuid(value.missionId) || !isUuid(value.manifestId)) return false;
  if (value.appendixRequest !== "policy_default" && value.appendixRequest !== "include") return false;
  if (value.appendixMode !== "included" && value.appendixMode !== "policy_state_not_high_impact") {
    return false;
  }
  // Each canonicalizer is named here, exactly. A digest whose canonicalizer is another module's is
  // a different identity wearing the right shape, and accepting it would let this screen show one
  // record's hex under another record's label.
  if (!isOutcomeDigest(value.manifestDigest, "beacon.outcome.manifest.v2")) return false;
  if (!isOutcomeDigest(value.candidateArtifactDigest, "beacon.artifact.v1")) return false;
  if (!isOutcomeDigest(value.candidateContentDigest, "beacon.raw.v1")) return false;
  if (!isNonnegativeInteger(value.candidateContentBytes)) return false;
  if (value.candidateContentBytes > MAX_CANDIDATE_CONTENT_BYTES) return false;
  return value.mediaType === CANDIDATE_MEDIA_TYPE;
}

/** Prepare one Outcome candidate from a Mission's recorded completion and evidence.
 *
 *  The Operator's two values go out unchanged and nothing else does: the owner derives the manifest,
 *  the candidate Artifact, every digest and the resolved appendix mode. Preparation writes a
 *  candidate; it does not evaluate or publish one, and this function makes no claim about either.
 *
 *  A reply that does not answer the request it was sent -- another Mission, another appendix request,
 *  or `include` resolved to the excluded mode -- is refused here rather than displayed. That refusal
 *  is an UNCONFIRMED outcome, never proof that nothing was written: the owner commits before it
 *  builds this receipt.
 *
 *  The native `prepare_outcome_candidate` command reaches `OutcomeService::prepare_for_mission`,
 *  then reads the owner-verified records to build this receipt. Frontend tests replace invoke with
 *  a double; their results prove frontend behavior, not the native/store path. */
export async function prepareOutcomeCandidate(request: {
  missionId: string;
  appendixRequest: AppendixRequest;
}): Promise<PreparedOutcomeCandidate> {
  const value = await invokeAndValidate(
    "prepare_outcome_candidate",
    { missionId: request.missionId, appendixRequest: request.appendixRequest },
    isPreparedOutcomeCandidate,
    "Outcome candidate",
  );
  if (
    value.missionId !== request.missionId ||
    value.appendixRequest !== request.appendixRequest ||
    (value.appendixRequest === "include" && value.appendixMode !== "included")
  ) {
    throw new Error("Beacon received an invalid Outcome candidate.");
  }
  return value;
}

/** The eight fields of C7's candidate document read. Metadata plus the exact stored bytes, base64
 *  encoded; no flag, no status, no appendix mode and no inferred "current" candidate. */
export type OutcomeCandidateDocument = {
  missionId: string;
  manifestId: string;
  manifestDigest: OutcomeDigestValue;
  candidateArtifactDigest: OutcomeDigestValue;
  candidateContentDigest: OutcomeDigestValue;
  candidateContentBytes: number;
  mediaType: string;
  candidateContentBase64: string;
};

/** The decoded document, beside the reply it came from. The text is produced ONCE, under fatal
 *  decoding, and never re-derived by a caller. */
export type ReadCandidateDocument = {
  document: OutcomeCandidateDocument;
  text: string;
};

/** `4 * ceil(MAX_CANDIDATE_CONTENT_BYTES / 3)`. Checked BEFORE decoding: a bound that exists to
 *  prevent an allocation has to run before the allocation. */
const MAX_CANDIDATE_BASE64_LENGTH = 1_398_104;

/** Canonical base64, strictly: the standard alphabet, correct padding, and no over-long
 *  representation. The last significant character of a padded group carries unused low bits, and
 *  two encodings that differ only there decode to the same bytes -- so accepting both would let one
 *  document have two encodings, which is exactly what a content-addressed record must not have. */
function isCanonicalBase64(value: unknown): value is string {
  if (typeof value !== "string") return false;
  if (value.length > MAX_CANDIDATE_BASE64_LENGTH) return false;
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) return false;
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  if (value.endsWith("==")) {
    // 12 bits carried, 8 used: the second character's low FOUR bits must be zero.
    return (alphabet.indexOf(value[value.length - 3]) & 0b1111) === 0;
  }
  if (value.endsWith("=")) {
    // 18 bits carried, 16 used: the third character's low TWO bits must be zero.
    return (alphabet.indexOf(value[value.length - 2]) & 0b11) === 0;
  }
  return true;
}

function isOutcomeCandidateDocument(value: unknown): value is OutcomeCandidateDocument {
  if (
    !hasExactKeys(value, [
      "candidateArtifactDigest",
      "candidateContentBase64",
      "candidateContentBytes",
      "candidateContentDigest",
      "manifestDigest",
      "manifestId",
      "mediaType",
      "missionId",
    ])
  ) {
    return false;
  }
  if (!isUuid(value.missionId) || !isUuid(value.manifestId)) return false;
  if (!isOutcomeDigest(value.manifestDigest, "beacon.outcome.manifest.v2")) return false;
  if (!isOutcomeDigest(value.candidateArtifactDigest, "beacon.artifact.v1")) return false;
  if (!isOutcomeDigest(value.candidateContentDigest, "beacon.raw.v1")) return false;
  if (!isNonnegativeInteger(value.candidateContentBytes)) return false;
  if (value.candidateContentBytes > MAX_CANDIDATE_CONTENT_BYTES) return false;
  if (value.mediaType !== CANDIDATE_MEDIA_TYPE) return false;
  return isCanonicalBase64(value.candidateContentBase64);
}

function sameDigest(left: OutcomeDigestValue, right: OutcomeDigestValue): boolean {
  return (
    left.algorithm === right.algorithm &&
    left.canonicalizer === right.canonicalizer &&
    left.value === right.value
  );
}

/** Read the exact stored text of the candidate this receipt names.
 *
 *  The native receives ONE value -- the held manifest digest's hex -- and the whole held receipt is
 *  passed here so every expectation is the RECEIPT's, never the reply's. A reply that answers a
 *  different Mission, manifest, Artifact, content or size is refused before anything is displayed.
 *
 *  Decoding is fatal and happens once: a replacement character would be a byte this client invented,
 *  and a document is its bytes. The BOM, if the stored text has one, is PRESERVED
 *  (`ignoreBOM: true` means "do not strip it"), and nothing here trims, normalises newlines or
 *  otherwise edits what was stored.
 *
 *  This function does not re-hash anything: the owner verifies the digest/content relationship, and
 *  restating that here without implementing it would be a claim this client cannot make.
 *
 *  The native command now routes through `OutcomeService::manifest` and
 *  `OutcomeService::candidate`. Frontend tests still answer invoke with a double;
 *  those tests prove client behavior, while native socket tests cover the owner path. */
export async function readOutcomeCandidateDocument(
  held: PreparedOutcomeCandidate,
): Promise<ReadCandidateDocument> {
  const document = await invokeAndValidate(
    "read_outcome_candidate_document",
    { manifestDigest: held.manifestDigest.value },
    isOutcomeCandidateDocument,
    "Outcome candidate document",
  );
  if (
    document.missionId !== held.missionId ||
    document.manifestId !== held.manifestId ||
    !sameDigest(document.manifestDigest, held.manifestDigest) ||
    !sameDigest(document.candidateArtifactDigest, held.candidateArtifactDigest) ||
    !sameDigest(document.candidateContentDigest, held.candidateContentDigest) ||
    document.candidateContentBytes !== held.candidateContentBytes ||
    document.mediaType !== held.mediaType
  ) {
    throw new Error("Beacon received an invalid Outcome candidate document.");
  }

  const encoded = document.candidateContentBase64;
  const binary = atob(encoded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  if (bytes.length !== held.candidateContentBytes || bytes.length > MAX_CANDIDATE_CONTENT_BYTES) {
    throw new Error("Beacon received an invalid Outcome candidate document.");
  }
  const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes);
  return { document, text };
}

async function invokeAndValidate<T>(
  command: string,
  argumentsValue: Record<string, unknown> | undefined,
  validator: (value: unknown) => value is T,
  label: string,
): Promise<T> {
  const value: unknown = argumentsValue
    ? await invoke(command, argumentsValue)
    : await invoke(command);
  if (!validator(value)) throw new Error(`Beacon received an invalid ${label}.`);
  return value;
}

function isDesktopSessionStatus(value: unknown): value is DesktopSessionStatus {
  if (!hasExactKeys(value, ["operatorCommands", "phase", "workspaceAccess"])) return false;
  const phase = value.phase;
  if (phase === "unlocked") {
    return value.workspaceAccess === true && value.operatorCommands === true;
  }
  return (
    (phase === "operator_setup_required" || phase === "locked") &&
    value.workspaceAccess === false &&
    value.operatorCommands === false
  );
}

function isAuthenticationCapabilities(value: unknown): value is AuthenticationCapabilities {
  if (!hasExactKeys(value, ["passkey", "passwordFallback", "preferredMethod"])) return false;
  if (value.preferredMethod !== "passkey") return false;
  const passkey = value.passkey;
  const fallback = value.passwordFallback;
  return (
    hasExactKeys(passkey, ["available", "reason"]) &&
    passkey.available === false &&
    (passkey.reason === "unsupported_linux_webkitgtk" ||
      passkey.reason === "native_adapter_not_available") &&
    hasExactKeys(fallback, ["available", "scope"]) &&
    fallback.available === true &&
    fallback.scope === "same_machine"
  );
}

function isRecoveryKeyOutput(value: unknown): value is RecoveryKeyOutput {
  return (
    hasExactKeys(value, ["recoveryKey"]) &&
    typeof value.recoveryKey === "string" &&
    value.recoveryKey.length > 0
  );
}

function isSessionPolicy(value: unknown): value is SessionPolicy {
  return (
    hasExactKeys(value, ["inactivityTimeoutMinutes"]) &&
    Number.isInteger(value.inactivityTimeoutMinutes) &&
    typeof value.inactivityTimeoutMinutes === "number" &&
    value.inactivityTimeoutMinutes >= 1 &&
    value.inactivityTimeoutMinutes <= 1_440
  );
}

function isPlatformSessionMonitorStatus(
  value: unknown,
): value is PlatformSessionMonitorStatus {
  if (!hasExactKeys(value, ["screenLock", "suspend"])) return false;
  const states = ["starting", "active", "unavailable"];
  return states.includes(String(value.screenLock)) && states.includes(String(value.suspend));
}

const starterRoles: StarterRole[] = ["lead", "builder", "reviewer", "researcher"];

function isStarterOrganizationView(value: unknown): value is StarterOrganizationView {
  if (
    !hasExactKeys(value, [
      "decision",
      "proposalVersion",
      "residents",
      "templates",
      "workspaceId",
    ]) ||
    !isUuid(value.workspaceId) ||
    value.proposalVersion !== 1 ||
    !["proposed", "accepted", "discarded"].includes(String(value.decision)) ||
    !Array.isArray(value.templates) ||
    !Array.isArray(value.residents)
  ) {
    return false;
  }
  if (
    value.templates.length !== starterRoles.length ||
    !value.templates.every((template, index) => {
      if (!hasExactKeys(template, ["description", "recommended", "role"])) return false;
      return (
        template.role === starterRoles[index] &&
        typeof template.description === "string" &&
        template.description.length > 0 &&
        template.description.length <= 240 &&
        template.recommended === (index < 3)
      );
    })
  ) {
    return false;
  }
  const roles = new Set<string>();
  const residentsValid = value.residents.every((resident) => {
    if (!hasExactKeys(resident, ["displayName", "id", "lifecycle", "role"])) return false;
    if (!isUuid(resident.id) || !starterRoles.includes(resident.role as StarterRole)) return false;
    if (roles.has(String(resident.role))) return false;
    roles.add(String(resident.role));
    return (
      resident.lifecycle === "inactive" &&
      (resident.displayName === null ||
        (typeof resident.displayName === "string" &&
          resident.displayName.trim() === resident.displayName &&
          resident.displayName.length >= 1 &&
          Array.from(resident.displayName).length <= 80 &&
          !/\p{Cc}/u.test(resident.displayName)))
    );
  });
  if (!residentsValid) return false;
  return value.decision === "accepted"
    ? value.residents.length >= 1 && value.residents.length <= 4
    : value.residents.length === 0;
}

function isResidentAgentView(value: unknown): value is ResidentAgentView {
  if (
    !hasExactKeys(value, [
      "createdAtMs",
      "displayName",
      "id",
      "latestDecisionId",
      "lifecycle",
      "role",
      "updatedAtMs",
      "workspaceId",
    ]) ||
    !isUuid(value.id) ||
    !isUuid(value.workspaceId) ||
    !starterRoles.includes(value.role as StarterRole) ||
    !["inactive", "active", "suspended", "retired"].includes(String(value.lifecycle)) ||
    !(value.latestDecisionId === null || isUuid(value.latestDecisionId)) ||
    !isNonnegativeInteger(value.createdAtMs) ||
    !isNonnegativeInteger(value.updatedAtMs) ||
    value.updatedAtMs < value.createdAtMs ||
    !(
      value.displayName === null ||
      (typeof value.displayName === "string" &&
        value.displayName.trim() === value.displayName &&
        value.displayName.length >= 1 &&
        Array.from(value.displayName).length <= 80 &&
        !/\p{Cc}/u.test(value.displayName))
    )
  ) {
    return false;
  }
  return value.lifecycle === "inactive"
    ? value.latestDecisionId === null && value.updatedAtMs === value.createdAtMs
    : value.latestDecisionId !== null;
}

function isResidentList(value: unknown): value is ResidentAgentView[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isResidentAgentView)) {
    return false;
  }
  const ids = new Set<string>();
  return value.every((resident) => {
    if (ids.has(resident.id)) return false;
    ids.add(resident.id);
    return true;
  });
}

const beaconCapabilities: readonly BeaconCapability[] = [
  "work.create_mission",
  "work.form_team",
  "work.coordinate_mission",
  "work.assign_work",
  "artifact.produce",
  "artifact.evaluate",
  "evidence.develop",
];

const trackRecordEvidenceKinds: readonly TrackRecordEvidenceKind[] = [
  "mission_outcome",
  "evaluation",
  "policy_compliance",
  "spend",
  "receipt",
];

const repositorySourceKinds: readonly RepositorySourceKind[] = ["local_path", "git_url"];

const teamIneligibilityReasons: readonly TeamIneligibilityReason[] = [
  "configuration_cleared",
  "resident_inactive",
  "resident_suspended",
  "resident_retired",
  "model_activation_unavailable",
];

function isResidentCapabilityCatalog(value: unknown): value is BeaconCapabilityDescriptor[] {
  return (
    Array.isArray(value) &&
    value.length === beaconCapabilities.length &&
    value.every(
      (descriptor, index) =>
        hasExactKeys(descriptor, ["capability", "description", "label"]) &&
        descriptor.capability === beaconCapabilities[index] &&
        isTrimmedTextWithoutControls(descriptor.label, 80) &&
        isTrimmedTextWithoutControls(descriptor.description, 300),
    )
  );
}

function isResidentConfigurationBinding(value: unknown): value is ResidentConfigurationBinding {
  return (
    hasExactKeys(value, ["activationId", "budgetBoundary", "capabilities"]) &&
    isUuid(value.activationId) &&
    value.budgetBoundary === "no_spend" &&
    isOrderedUniqueClosedValues(value.capabilities, beaconCapabilities, false)
  );
}

function isResidentConfiguration(value: unknown): value is ResidentConfiguration {
  if (
    !hasExactKeys(value, [
      "binding",
      "decidedAtMs",
      "eligibleForTeam",
      "id",
      "ineligibilityReasons",
      "predecessorId",
      "residentId",
      "state",
      "workspaceId",
    ]) ||
    !isUuid(value.id) ||
    !isUuid(value.workspaceId) ||
    !isUuid(value.residentId) ||
    !(value.predecessorId === null || isUuid(value.predecessorId)) ||
    !isNonnegativeInteger(value.decidedAtMs) ||
    typeof value.eligibleForTeam !== "boolean" ||
    !isOrderedUniqueClosedValues(
      value.ineligibilityReasons,
      teamIneligibilityReasons,
      true,
    )
  ) {
    return false;
  }
  if (value.state === "configured") {
    if (!isResidentConfigurationBinding(value.binding)) return false;
  } else if (value.state === "cleared") {
    if (value.binding !== null || !value.ineligibilityReasons.includes("configuration_cleared")) {
      return false;
    }
  } else {
    return false;
  }
  return value.eligibleForTeam
    ? value.state === "configured" && value.ineligibilityReasons.length === 0
    : value.ineligibilityReasons.length > 0;
}

function isResidentConfigurationList(value: unknown): value is ResidentConfiguration[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isResidentConfiguration)) {
    return false;
  }
  const residents = new Set<string>();
  return value.every((configuration, index) => {
    if (residents.has(configuration.residentId)) return false;
    residents.add(configuration.residentId);
    return index === 0 || value[index - 1].residentId < configuration.residentId;
  });
}

function isOrderedUniqueClosedValues<T extends string>(
  value: unknown,
  order: readonly T[],
  allowEmpty: boolean,
): value is T[] {
  if (!Array.isArray(value) || (!allowEmpty && value.length === 0) || value.length > order.length) {
    return false;
  }
  let prior = -1;
  return value.every((item) => {
    const index = order.indexOf(item as T);
    if (index <= prior) return false;
    prior = index;
    return true;
  });
}

function isProjectSummaries(value: unknown): value is ProjectSummary[] {
  return Array.isArray(value) && value.every(isProjectSummary);
}

function isSampleProjectDigest(value: unknown): value is SampleProjectDigest {
  return (
    hasExactKeys(value, ["algorithm", "canonicalizer", "value"]) &&
    value.algorithm === "sha256" &&
    value.canonicalizer === "beacon.sample-project.bundle.v1" &&
    typeof value.value === "string" &&
    /^[0-9a-f]{64}$/.test(value.value)
  );
}

function isSampleProjectInstallation(value: unknown): value is SampleProjectInstallation {
  return (
    hasExactKeys(value, [
      "baseRevision",
      "charterRevision",
      "installedAtMs",
      "projectId",
      "provenance",
      "repositoryPath",
      "repositoryRevision",
      "workspaceId",
    ]) &&
    isUuid(value.workspaceId) &&
    isUuid(value.projectId) &&
    value.charterRevision === 1 &&
    value.repositoryRevision === 1 &&
    isRepositorySource("local_path", value.repositoryPath) &&
    isBaseRevision(value.baseRevision) &&
    hasExactKeys(value.provenance, ["bundleDigest", "bundleVersion", "source"]) &&
    value.provenance.source === "beacon_bundled_sample_project" &&
    value.provenance.bundleVersion === 1 &&
    isSampleProjectDigest(value.provenance.bundleDigest) &&
    isNonnegativeInteger(value.installedAtMs)
  );
}

function isSampleProjectStatus(value: unknown): value is SampleProjectStatus {
  if (hasExactKeys(value, ["bundleDigest", "bundleVersion", "state"])) {
    return (
      value.state === "available" &&
      value.bundleVersion === 1 &&
      isSampleProjectDigest(value.bundleDigest)
    );
  }
  return (
    hasExactKeys(value, ["installation", "state"]) &&
    value.state === "installed" &&
    isSampleProjectInstallation(value.installation)
  );
}

function isSampleProjectInstallResult(value: unknown): value is SampleProjectInstallResult {
  return (
    hasExactKeys(value, ["installation", "outcome"]) &&
    (value.outcome === "installed" || value.outcome === "already_installed") &&
    isSampleProjectInstallation(value.installation)
  );
}

function isProjectSummary(value: unknown): value is ProjectSummary {
  return (
    hasExactKeys(value, [
      "autonomyProfile",
      "charterRevision",
      "createdAtMs",
      "id",
      "name",
    ]) &&
    isUuid(value.id) &&
    isProjectName(value.name) &&
    isPositiveInteger(value.charterRevision) &&
    isAutonomyProfile(value.autonomyProfile) &&
    isNonnegativeInteger(value.createdAtMs)
  );
}

function isProjectView(value: unknown): value is ProjectView {
  return (
    hasExactKeys(value, ["charter", "createdAtMs", "id", "name", "workspaceId"]) &&
    isUuid(value.id) &&
    isUuid(value.workspaceId) &&
    isProjectName(value.name) &&
    isProjectCharter(value.charter) &&
    isNonnegativeInteger(value.createdAtMs)
  );
}

function isProjectCharter(value: unknown): value is ProjectCharter {
  return (
    hasExactKeys(value, [
      "authoredAtMs",
      "autonomyProfile",
      "budgetBoundary",
      "constraints",
      "desiredOutcomes",
      "externalEffects",
      "prohibitions",
      "purpose",
      "revision",
    ]) &&
    isPositiveInteger(value.revision) &&
    isPurpose(value.purpose) &&
    isCharterItems(value.desiredOutcomes, true) &&
    isCharterItems(value.constraints, false) &&
    isCharterItems(value.prohibitions, false) &&
    isAutonomyProfile(value.autonomyProfile) &&
    value.budgetBoundary === "no_spend" &&
    value.externalEffects === "prohibited" &&
    isNonnegativeInteger(value.authoredAtMs)
  );
}

function isProjectCharterHistory(value: unknown): value is ProjectCharter[] {
  if (!Array.isArray(value) || value.length === 0 || !value.every(isProjectCharter)) return false;
  return value.every((charter, index) => {
    if (charter.revision !== index + 1) return false;
    return index === 0 || value[index - 1].authoredAtMs <= charter.authoredAtMs;
  });
}

function isTrackRecordRequirement(value: unknown): value is TrackRecordRequirement {
  return (
    hasExactKeys(value, ["evidenceKind", "minimumCount"]) &&
    trackRecordEvidenceKinds.includes(value.evidenceKind as TrackRecordEvidenceKind) &&
    isPositiveInteger(value.minimumCount)
  );
}

function isTeamLeadSelectionPolicy(value: unknown): value is TeamLeadSelectionPolicy {
  if (
    !hasExactKeys(value, [
      "ambiguityRule",
      "availabilityRule",
      "conflictRule",
      "requiredCapabilities",
      "trackRecordRequirements",
    ]) ||
    !isOrderedUniqueClosedValues(value.requiredCapabilities, beaconCapabilities, false) ||
    !value.requiredCapabilities.includes("work.coordinate_mission") ||
    value.availabilityRule !== "current_team_eligibility" ||
    value.conflictRule !== "require_no_declared_conflict" ||
    value.ambiguityRule !== "require_operator_decision" ||
    !Array.isArray(value.trackRecordRequirements) ||
    value.trackRecordRequirements.length === 0 ||
    value.trackRecordRequirements.length > trackRecordEvidenceKinds.length ||
    !value.trackRecordRequirements.every(isTrackRecordRequirement)
  ) {
    return false;
  }
  let prior = -1;
  return value.trackRecordRequirements.every((requirement) => {
    const index = trackRecordEvidenceKinds.indexOf(requirement.evidenceKind);
    if (index <= prior) return false;
    prior = index;
    return true;
  });
}

function isProjectPolicy(value: unknown): value is ProjectPolicy {
  return (
    hasExactKeys(value, [
      "appliesToCurrentCharter",
      "authoredAtMs",
      "authoredBy",
      "charterRevision",
      "projectId",
      "revision",
      "teamLeadSelection",
      "workspaceId",
    ]) &&
    isUuid(value.workspaceId) &&
    isUuid(value.projectId) &&
    isPositiveInteger(value.revision) &&
    isPositiveInteger(value.charterRevision) &&
    isTeamLeadSelectionPolicy(value.teamLeadSelection) &&
    value.authoredBy === "operator" &&
    isNonnegativeInteger(value.authoredAtMs) &&
    typeof value.appliesToCurrentCharter === "boolean"
  );
}

function isProjectPolicyOrNull(value: unknown): value is ProjectPolicy | null {
  return value === null || isProjectPolicy(value);
}

function isPublicationSummary(value: unknown): value is PublicationSummary {
  return (
    hasExactKeys(value, [
      "candidateArtifactDigest",
      "evaluationDigest",
      "isCurrent",
      "publicationId",
      "publicationVersion",
    ]) &&
    isUuid(value.publicationId) &&
    isPositiveInteger(value.publicationVersion) &&
    isDigestValue(value.candidateArtifactDigest) &&
    isDigestValue(value.evaluationDigest) &&
    typeof value.isCurrent === "boolean"
  );
}

function isPublicationHistory(value: unknown): value is PublicationSummary[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isPublicationSummary)) {
    return false;
  }
  // Newest first, contiguous down to 1, and exactly one current version. The service already
  // guarantees this; the guard states it so a wire that ever disagreed is refused rather than drawn.
  return value.every((summary, index) => {
    if (summary.publicationVersion !== value.length - index) return false;
    return summary.isCurrent === (index === 0);
  });
}

function isExportedOutcome(value: unknown): value is ExportedOutcome {
  return (
    hasExactKeys(value, [
      "bytes",
      "digest",
      "mediaType",
      "publicationVersion",
      "publicationVersionCount",
    ]) &&
    Array.isArray(value.bytes) &&
    value.bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255) &&
    hasExactKeys(value.digest, ["algorithm", "canonicalizer", "value"]) &&
    typeof value.digest.algorithm === "string" &&
    typeof value.digest.canonicalizer === "string" &&
    isDigestValue(value.digest.value) &&
    isPositiveInteger(value.publicationVersion) &&
    isPositiveInteger(value.publicationVersionCount) &&
    value.publicationVersion <= value.publicationVersionCount &&
    typeof value.mediaType === "string"
  );
}

function isDigestValue(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{64}$/.test(value);
}

function isProjectPolicyHistory(value: unknown): value is ProjectPolicy[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isProjectPolicy)) return false;
  return value.every((policy, index) => {
    if (policy.revision !== index + 1) return false;
    if (index === 0) return true;
    const prior = value[index - 1];
    return (
      prior.workspaceId === policy.workspaceId &&
      prior.projectId === policy.projectId &&
      prior.authoredAtMs <= policy.authoredAtMs
    );
  });
}

function isCheckArgument(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !value.includes("\u0000");
}

function isRepositoryRelativeDirectory(value: unknown): value is string {
  if (typeof value !== "string" || value.length === 0) return false;
  if (value === ".") return true;
  if (value.startsWith("/") || value.includes("\\") || value.includes("\u0000")) return false;
  return value
    .split("/")
    .every((component) => component.length > 0 && component !== "." && component !== "..");
}

function isDeclaredCheck(value: unknown): value is DeclaredCheck {
  return (
    hasExactKeys(value, ["argv", "name", "workingDirectory"]) &&
    typeof value.name === "string" &&
    value.name.length <= 64 &&
    /^[a-z0-9][a-z0-9._-]*$/.test(value.name) &&
    Array.isArray(value.argv) &&
    value.argv.length > 0 &&
    value.argv.every(isCheckArgument) &&
    isRepositoryRelativeDirectory(value.workingDirectory)
  );
}

function isDeclaredCheckList(value: unknown): value is DeclaredCheck[] {
  if (!Array.isArray(value) || value.length === 0 || !value.every(isDeclaredCheck)) return false;
  return value.every((check, index) => index === 0 || value[index - 1].name < check.name);
}

function isRepositorySource(kind: RepositorySourceKind, value: unknown): value is string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > 2000 ||
    value.trim() !== value
  ) {
    return false;
  }
  if (kind === "local_path") return value.startsWith("/") && !value.endsWith("/");
  return /^(?:https|ssh):\/\/\S+$/.test(value) || /^\S+@[^\s/:]+:\S+$/.test(value);
}

function isBaseRevision(value: unknown): value is string {
  return typeof value === "string" && /^[0-9a-f]{40}$/.test(value);
}

function isProjectRepository(value: unknown): value is ProjectRepository {
  return (
    hasExactKeys(value, [
      "appliesToCurrentCharter",
      "authorKind",
      "authoredAtMs",
      "baseRevision",
      "charterRevision",
      "declaredChecks",
      "projectId",
      "revision",
      "source",
      "sourceKind",
      "workspaceId",
    ]) &&
    isUuid(value.workspaceId) &&
    isUuid(value.projectId) &&
    isPositiveInteger(value.revision) &&
    isPositiveInteger(value.charterRevision) &&
    repositorySourceKinds.includes(value.sourceKind as RepositorySourceKind) &&
    isRepositorySource(value.sourceKind as RepositorySourceKind, value.source) &&
    isBaseRevision(value.baseRevision) &&
    isDeclaredCheckList(value.declaredChecks) &&
    value.authorKind === "operator" &&
    isNonnegativeInteger(value.authoredAtMs) &&
    typeof value.appliesToCurrentCharter === "boolean"
  );
}

function isRecentReceiptOutcome(value: unknown): value is RecentReceiptOutcome {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const kind = (value as Record<string, unknown>).kind;
  if (kind === "check_preflight") {
    return (
      hasExactKeys(value, ["kind", "outcome"]) &&
      PREFLIGHT_OUTCOMES.includes(value.outcome as PreflightOutcome)
    );
  }
  if (kind === "declared_checks") {
    return (
      hasExactKeys(value, ["kind", "summary"]) &&
      EXECUTION_SUMMARIES.includes(value.summary as ExecutionSummary)
    );
  }
  return false;
}

function isRecentReceipt(value: unknown): value is RecentReceipt {
  return (
    hasExactKeys(value, [
      "endedAtMs",
      "outcome",
      "projectId",
      "projectName",
      "recordedAtMs",
      "runId",
      "startedAtMs",
      "version",
    ]) &&
    isUuid(value.runId) &&
    isUuid(value.projectId) &&
    isTrimmedTextWithoutControls(value.projectName, 120) &&
    isPositiveInteger(value.version) &&
    isRecentReceiptOutcome(value.outcome) &&
    // `isRenderableInstant`, not `isNonnegativeInteger`. The merged fix
    // `4e1ff8b` bought this guard for the path this Query replaced, and the
    // replacement dropped it: every value in
    // `8_640_000_000_000_001 ..= 9_007_199_254_740_991` is a safe non-negative
    // integer and none is a representable instant.
    isRenderableInstant(value.startedAtMs) &&
    isRenderableInstant(value.endedAtMs) &&
    isRenderableInstant(value.recordedAtMs) &&
    (value.endedAtMs as number) >= (value.startedAtMs as number)
  );
}

function isRecentReceiptPage(value: unknown): value is RecentReceiptPage {
  if (
    !hasExactKeys(value, [
      "moreRunsExist",
      "receipts",
      "runWindowApplied",
      "runsInFlightInWindow",
      "runsUnconfirmedInWindow",
      "runsVerifiedExcludedInWindow",
    ])
  ) {
    return false;
  }
  const receipts = value.receipts;
  if (
    typeof value.moreRunsExist !== "boolean" ||
    !isBoundedCount(value.runWindowApplied) ||
    !isBoundedCount(value.runsInFlightInWindow) ||
    !isBoundedCount(value.runsUnconfirmedInWindow) ||
    !isBoundedCount(value.runsVerifiedExcludedInWindow) ||
    !Array.isArray(receipts) ||
    !receipts.every(isRecentReceipt) ||
    receipts.length +
      value.runsInFlightInWindow +
      value.runsUnconfirmedInWindow +
      value.runsVerifiedExcludedInWindow >
      value.runWindowApplied
  ) {
    return false;
  }
  // The daemon defines the order; re-sorting here would put the ordering
  // policy back in the interface. Code points, not `localeCompare`: SQLite
  // orders by BINARY, and a locale-sensitive comparison would refuse a
  // correctly ordered page on some locales.
  return (receipts as RecentReceipt[]).every((receipt, index) => {
    if (index === 0) return true;
    const prior = (receipts as RecentReceipt[])[index - 1];
    if (prior.startedAtMs !== receipt.startedAtMs) return prior.startedAtMs > receipt.startedAtMs;
    return prior.runId > receipt.runId;
  });
}

function isBoundedCount(value: unknown): value is number {
  return isNonnegativeInteger(value) && value <= MAX_U32_COUNT;
}

const PREFLIGHT_OUTCOMES: readonly PreflightOutcome[] = [
  "verified",
  "source_kind_unsupported",
  "binding_stale_for_charter",
  "repository_unreadable",
  "base_revision_unreachable",
  "declared_checks_changed",
  "working_directory_escapes",
  "sandbox_unavailable",
  "preflight_failed",
  "preflight_timed_out",
];

function isRecorded<T>(value: unknown, isValue: (candidate: unknown) => candidate is T): boolean {
  if (hasExactKeys(value, ["kind", "reason"])) {
    return value.kind === "absent" && typeof value.reason === "string";
  }
  if (hasExactKeys(value, ["kind", "value"])) {
    return value.kind === "present" && isValue(value.value);
  }
  return false;
}

const isString = (value: unknown): value is string => typeof value === "string";
const isNumber = (value: unknown): value is number => typeof value === "number";

function isResolvedCheck(value: unknown): value is ResolvedCheck {
  return (
    hasExactKeys(value, ["argv", "declaredWorkingDirectory", "name", "resolvedWorkingDirectory"]) &&
    typeof value.name === "string" &&
    Array.isArray(value.argv) &&
    value.argv.every(isString) &&
    typeof value.declaredWorkingDirectory === "string" &&
    isRecorded(value.resolvedWorkingDirectory, isString)
  );
}

function isUnavailableEvidence(value: unknown): value is UnavailableEvidence {
  return (
    hasExactKeys(value, [
      "costUsdMicros",
      "diffDigest",
      "filesystemEffects",
      "modelInvocations",
      "proposalDigest",
      "resultingRevision",
      "rulingDigest",
    ]) &&
    isRecorded(value.proposalDigest, isString) &&
    isRecorded(value.rulingDigest, isString) &&
    isRecorded(value.resultingRevision, isString) &&
    isRecorded(value.filesystemEffects, isString) &&
    isRecorded(value.diffDigest, isString) &&
    isRecorded(value.modelInvocations, isNumber) &&
    isRecorded(value.costUsdMicros, isNumber)
  );
}

export function isPreflightReceipt(value: unknown): value is PreflightReceipt {
  return (
    hasExactKeys(value, [
      "baseRevision",
      "charterRevision",
      "endedAtMs",
      "notVerified",
      "outcome",
      "policyCharterRevision",
      "policyRepositoryRevision",
      "projectId",
      "repositoryRevision",
      "resolvedChecks",
      "runId",
      "sandboxVersion",
      "source",
      "sourceKind",
      "startedAtMs",
      "unavailable",
      "version",
      "workspaceId",
    ]) &&
    value.version === 1 &&
    isUuid(value.runId) &&
    isUuid(value.workspaceId) &&
    isUuid(value.projectId) &&
    typeof value.repositoryRevision === "number" &&
    typeof value.charterRevision === "number" &&
    PREFLIGHT_OUTCOMES.includes(value.outcome as PreflightOutcome) &&
    (value.sourceKind === "local_path" || value.sourceKind === "git_url") &&
    typeof value.source === "string" &&
    typeof value.baseRevision === "string" &&
    Array.isArray(value.resolvedChecks) &&
    value.resolvedChecks.every(isResolvedCheck) &&
    typeof value.sandboxVersion === "string" &&
    typeof value.policyCharterRevision === "number" &&
    typeof value.policyRepositoryRevision === "number" &&
    isRenderableInstant(value.startedAtMs) &&
    isRenderableInstant(value.endedAtMs) &&
    value.startedAtMs <= value.endedAtMs &&
    Array.isArray(value.notVerified) &&
    value.notVerified.every(isString) &&
    isUnavailableEvidence(value.unavailable)
  );
}

/** The daemon applies no LIMIT to this Query -- `check_preflight.rs` returns
 *  every Receipt a Project has -- so the only bound is here. Matches the cap on
 *  repository history: an unbounded array is a Workspace-sized allocation
 *  crossing the boundary before anything decides how many rows to draw. */
/** A millisecond instant the interface can actually render.
 *
 *  `typeof x === "number"` admits NaN, Infinity, fractions, and values past the
 *  ECMAScript time range. The renderer calls `new Date(x).toISOString()`, which
 *  throws RangeError on those, so an unrenderable instant bypasses the designed
 *  failed-read state and takes the panel down instead. Refusing at the boundary
 *  is what the failed-read state is for. */
const MAX_TIME_VALUE = 8.64e15;
function isRenderableInstant(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= -MAX_TIME_VALUE &&
    value <= MAX_TIME_VALUE
  );
}

function isProjectRepositoryOrNull(value: unknown): value is ProjectRepository | null {
  return value === null || isProjectRepository(value);
}

function isProjectRepositoryHistory(value: unknown): value is ProjectRepository[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isProjectRepository)) {
    return false;
  }
  return value.every((repository, index) => {
    if (repository.revision !== index + 1) return false;
    if (index === 0) return true;
    const prior = value[index - 1];
    return (
      prior.workspaceId === repository.workspaceId &&
      prior.projectId === repository.projectId &&
      prior.authoredAtMs <= repository.authoredAtMs
    );
  });
}

function isTeamMemberView(value: unknown): value is TeamMemberView {
  if (
    !hasExactKeys(value, [
      "currentConfigurationId",
      "eligibleNow",
      "formationConfigurationId",
      "ineligibilityReasons",
      "residentId",
    ]) ||
    !isUuid(value.residentId) ||
    !isUuid(value.formationConfigurationId) ||
    !isUuid(value.currentConfigurationId) ||
    typeof value.eligibleNow !== "boolean" ||
    !isOrderedUniqueClosedValues(value.ineligibilityReasons, teamIneligibilityReasons, true)
  ) {
    return false;
  }
  return value.eligibleNow
    ? value.ineligibilityReasons.length === 0
    : value.ineligibilityReasons.length > 0;
}

function isTeamView(value: unknown): value is TeamView {
  if (
    !hasExactKeys(value, [
      "formedAtMs",
      "formedBy",
      "id",
      "members",
      "name",
      "projectId",
      "workspaceId",
    ]) ||
    !isUuid(value.id) ||
    !isUuid(value.workspaceId) ||
    !isUuid(value.projectId) ||
    !isProjectName(value.name) ||
    value.formedBy !== "operator" ||
    !isNonnegativeInteger(value.formedAtMs) ||
    !Array.isArray(value.members) ||
    value.members.length === 0 ||
    value.members.length > 512 ||
    !value.members.every(isTeamMemberView)
  ) {
    return false;
  }
  const residentIds = new Set<string>();
  return value.members.every((member) => {
    if (residentIds.has(member.residentId)) return false;
    residentIds.add(member.residentId);
    return true;
  });
}

function isTeamList(value: unknown): value is TeamView[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isTeamView)) return false;
  const ids = new Set<string>();
  return value.every((team, index) => {
    if (ids.has(team.id)) return false;
    ids.add(team.id);
    if (index === 0) return true;
    const prior = value[index - 1];
    return prior.formedAtMs < team.formedAtMs ||
      (prior.formedAtMs === team.formedAtMs && prior.id < team.id);
  });
}

function isMissionMemberView(value: unknown): value is MissionMemberView {
  return (
    hasExactKeys(value, ["admissionConfigurationId", "residentId"]) &&
    isUuid(value.residentId) &&
    isUuid(value.admissionConfigurationId)
  );
}

function isMissionView(value: unknown): value is MissionView {
  if (
    !hasExactKeys(value, [
      "admittedAtMs",
      "admittedBy",
      "charterRevision",
      "constraints",
      "desiredOutcome",
      "id",
      "lifecycle",
      "members",
      "prohibitions",
      "projectId",
      "successConditions",
      "teamId",
      "workspaceId",
    ]) ||
    !isUuid(value.id) ||
    !isUuid(value.workspaceId) ||
    !isUuid(value.projectId) ||
    !isPositiveInteger(value.charterRevision) ||
    !isUuid(value.teamId) ||
    !isPurpose(value.desiredOutcome) ||
    !isCharterItems(value.successConditions, true) ||
    !isCharterItems(value.constraints, false) ||
    !isCharterItems(value.prohibitions, false) ||
    value.lifecycle !== "admitted" ||
    value.admittedBy !== "operator" ||
    !isNonnegativeInteger(value.admittedAtMs) ||
    !Array.isArray(value.members) ||
    value.members.length === 0 ||
    value.members.length > 512 ||
    !value.members.every(isMissionMemberView)
  ) {
    return false;
  }
  const residentIds = new Set<string>();
  return value.members.every((member) => {
    if (residentIds.has(member.residentId)) return false;
    residentIds.add(member.residentId);
    return true;
  });
}

function isMissionList(value: unknown): value is MissionView[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isMissionView)) return false;
  const ids = new Set<string>();
  return value.every((mission, index) => {
    if (ids.has(mission.id)) return false;
    ids.add(mission.id);
    if (index === 0) return true;
    const prior = value[index - 1];
    return prior.admittedAtMs < mission.admittedAtMs ||
      (prior.admittedAtMs === mission.admittedAtMs && prior.id < mission.id);
  });
}

function isSubject(value: unknown): value is Subject {
  if (
    !hasExactKeys(value, ["role", "shortened", "value"]) ||
    !SUBJECT_ROLES.includes(value.role as SubjectRole) ||
    typeof value.shortened !== "boolean" ||
    !isTrimmedTextWithoutControls(value.value, MAX_SUBJECT_CHARS)
  ) {
    return false;
  }
  // A device number is a SHAPE, not bounded text, and it is short enough that
  // no constructor ever cuts one: `4294967295:4294967295` is 21 characters
  // against a bound of 24. Accepting `not-a-device`, or a three-character value
  // that claims it was shortened, accepts a report the reader cannot produce.
  if (value.role === "device") {
    return !value.shortened && isDeviceNumber(value.value);
  }
  return true;
}

function isDeviceNumber(value: string): boolean {
  const parts = value.split(":");
  if (parts.length !== 2) return false;
  return parts.every(
    (part) =>
      /^[0-9]{1,10}$/.test(part) &&
      Number(part) <= MAX_U32_COUNT &&
      String(Number(part)) === part,
  );
}

function isLiveDataEncryptionConclusion(
  value: unknown,
): value is LiveDataEncryptionConclusion {
  if (!hasExactKeys(value, ["omittedSubjects", "reason", "state", "subjects"])) return false;
  const { subjects } = value;
  if (
    !REASON_CODES.includes(value.reason as ReasonCode) ||
    !isBoundedCount(value.omittedSubjects) ||
    !Array.isArray(subjects) ||
    subjects.length > MAX_SUBJECTS ||
    (value.omittedSubjects > 0 && subjects.length !== MAX_SUBJECTS) ||
    !subjects.every(isSubject)
  ) {
    return false;
  }
  const reason = value.reason as ReasonCode;
  // A state is READ OFF its reason rather than agreed with it, so the two
  // cannot drift and no third combination has to be enumerated.
  if (value.state !== stateForReason(reason)) return false;
  const roles = new Set((subjects as Subject[]).map((subject) => subject.role));
  return requiredRoles(reason).every((role) => roles.has(role));
}

function isLiveDataEncryptionReport(value: unknown): value is LiveDataEncryptionReport {
  if (
    !hasExactKeys(value, [
      "conclusion",
      "evidence",
      "mountPoint",
      "notVerified",
      "observedAtMs",
      "omittedEvidence",
    ])
  ) {
    return false;
  }
  const { evidence, notVerified } = value;
  return (
    isLiveDataEncryptionConclusion(value.conclusion) &&
    isNonnegativeInteger(value.observedAtMs) &&
    isBoundedCount(value.omittedEvidence) &&
    // A volume is named by where it is mounted. Any other role here is a
    // report the reader could not have produced.
    (value.mountPoint === null ||
      (isSubject(value.mountPoint) && value.mountPoint.role === "location")) &&
    Array.isArray(evidence) &&
    evidence.length <= MAX_EVIDENCE_ENTRIES &&
    (value.omittedEvidence === 0 || evidence.length === MAX_EVIDENCE_ENTRIES) &&
    evidence.every(isEncryptionEvidence) &&
    Array.isArray(notVerified) &&
    notVerified.length <= MAX_NOT_VERIFIED_ENTRIES &&
    notVerified.every((entry) => isTrimmedTextWithoutControls(entry, MAX_OBSERVATION_CHARS))
  );
}

function isEncryptionEvidence(value: unknown): value is EncryptionEvidence {
  return (
    hasExactKeys(value, ["observation", "source", "truncated"]) &&
    typeof value.truncated === "boolean" &&
    isTrimmedTextWithoutControls(value.source, MAX_OBSERVATION_CHARS) &&
    isTrimmedTextWithoutControls(value.observation, MAX_OBSERVATION_CHARS)
  );
}

function isLocalModelDiscoveryReport(value: unknown): value is LocalModelDiscoveryReport {
  if (!hasExactKeys(value, ["endpoint", "models", "observedAtMs", "status"])) return false;
  const models = value.models;
  if (
    !isLoopbackHttpEndpoint(value.endpoint) ||
    !isNonnegativeInteger(value.observedAtMs) ||
    !["available", "unavailable", "invalid_response"].includes(String(value.status)) ||
    !Array.isArray(models) ||
    models.length > 512 ||
    !models.every(isDiscoveredLocalModel)
  ) {
    return false;
  }
  if (value.status !== "available" && models.length !== 0) return false;
  return models.every(
    (model, index) => index === 0 || models[index - 1].name < model.name,
  );
}

function isDiscoveredLocalModel(value: unknown): value is DiscoveredLocalModel {
  return (
    hasExactKeys(value, [
      "activation",
      "details",
      "digest",
      "modifiedAt",
      "name",
      "provider",
      "sizeBytes",
    ]) &&
    value.provider === "ollama" &&
    value.activation === "inactive" &&
    isTrimmedTextWithoutControls(value.name, 256) &&
    typeof value.digest === "string" &&
    /^[0-9a-f]{64}$/.test(value.digest) &&
    isPositiveInteger(value.sizeBytes) &&
    isTrimmedTextWithoutControls(value.modifiedAt, 80) &&
    isLocalModelDetails(value.details)
  );
}

function isLocalModelDetails(value: unknown): value is LocalModelDetails {
  return (
    hasExactKeys(value, ["family", "format", "parameterSize", "quantizationLevel"]) &&
    isTrimmedTextWithoutControls(value.format, 80) &&
    isTrimmedTextWithoutControls(value.family, 80) &&
    isTrimmedTextWithoutControls(value.parameterSize, 80) &&
    isTrimmedTextWithoutControls(value.quantizationLevel, 80)
  );
}

const providerFeatures: readonly ProviderFeature[] = [
  "completion",
  "embedding",
  "thinking",
  "tools",
  "vision",
];

function isLocalModelBinding(value: unknown): value is LocalModelBinding {
  return (
    hasExactKeys(value, [
      "endpoint",
      "inspectionDigest",
      "modelDigest",
      "modelName",
      "provider",
    ]) &&
    value.provider === "ollama" &&
    isLoopbackHttpEndpoint(value.endpoint) &&
    isTrimmedTextWithoutControls(value.modelName, 256) &&
    typeof value.modelDigest === "string" &&
    /^[0-9a-f]{64}$/.test(value.modelDigest) &&
    typeof value.inspectionDigest === "string" &&
    /^[0-9a-f]{64}$/.test(value.inspectionDigest)
  );
}

function isLocalModelActivationReview(value: unknown): value is LocalModelActivationReview {
  return (
    hasExactKeys(value, ["binding", "providerFeatures", "unrecognizedFeatures"]) &&
    isLocalModelBinding(value.binding) &&
    isSortedUniqueProviderFeatures(value.providerFeatures, true) &&
    isSortedUniqueUnknownFeatures(value.unrecognizedFeatures)
  );
}

function isLocalModelActivation(value: unknown): value is LocalModelActivation {
  if (!hasExactKeys(value, [
    "approvedFeatures",
    "binding",
    "decidedAtMs",
    "id",
    "predecessorId",
    "providerFeatures",
    "state",
    "unrecognizedFeatures",
    "workspaceId",
  ])) {
    return false;
  }
  const providerFeaturesValue = value.providerFeatures;
  const approvedFeaturesValue = value.approvedFeatures;
  if (
    !isUuid(value.id) ||
    !isUuid(value.workspaceId) ||
    !isLocalModelBinding(value.binding) ||
    !isSortedUniqueProviderFeatures(providerFeaturesValue, false) ||
    !isSortedUniqueUnknownFeatures(value.unrecognizedFeatures) ||
    !isSortedUniqueProviderFeatures(approvedFeaturesValue, false) ||
    !approvedFeaturesValue.every((feature) => providerFeaturesValue.includes(feature)) ||
    !["active", "inactive"].includes(String(value.state)) ||
    !(value.predecessorId === null || isUuid(value.predecessorId)) ||
    !isNonnegativeInteger(value.decidedAtMs)
  ) return false;
  return value.state === "active" || value.predecessorId !== null;
}

function isLocalModelActivationList(value: unknown): value is LocalModelActivation[] {
  if (!Array.isArray(value) || value.length > 512 || !value.every(isLocalModelActivation)) {
    return false;
  }
  const resources = new Set<string>();
  return value.every((activation, index) => {
    const key = `${activation.binding.modelName}\n${activation.binding.endpoint}`;
    if (resources.has(key)) return false;
    resources.add(key);
    if (index === 0) return true;
    const prior = value[index - 1];
    return (
      prior.binding.modelName < activation.binding.modelName ||
      (prior.binding.modelName === activation.binding.modelName &&
        prior.binding.endpoint < activation.binding.endpoint)
    );
  });
}

function isSortedUniqueProviderFeatures(
  value: unknown,
  allowEmpty: boolean,
): value is ProviderFeature[] {
  if (!Array.isArray(value) || value.length > providerFeatures.length) return false;
  if (!allowEmpty && value.length === 0) return false;
  return value.every(
    (feature, index) =>
      providerFeatures.includes(feature) &&
      (index === 0 || String(value[index - 1]) < String(feature)),
  );
}

function isSortedUniqueUnknownFeatures(value: unknown): value is string[] {
  if (!Array.isArray(value) || value.length > 64) return false;
  return value.every(
    (feature, index) =>
      isTrimmedTextWithoutControls(feature, 64) &&
      !providerFeatures.includes(feature as ProviderFeature) &&
      (index === 0 || String(value[index - 1]) < feature),
  );
}

function isLoopbackHttpEndpoint(value: unknown): value is string {
  if (typeof value !== "string") return false;
  const ipv6 = /^http:\/\/\[::1\]:([1-9][0-9]{0,4})$/.exec(value);
  if (ipv6) return Number(ipv6[1]) <= 65_535;
  const ipv4 = /^http:\/\/127\.(\d{1,3})\.(\d{1,3})\.(\d{1,3}):([1-9][0-9]{0,4})$/.exec(value);
  return (
    ipv4 !== null &&
    ipv4.slice(1, 4).every((octet) => Number(octet) <= 255) &&
    Number(ipv4[4]) <= 65_535
  );
}

function isProjectName(value: unknown): value is string {
  return isTrimmedText(value, 120) && !/\p{Cc}/u.test(value);
}

function isPurpose(value: unknown): value is string {
  return (
    isTrimmedText(value, 2_000) &&
    !/\p{Cc}/u.test(value.replaceAll("\n", ""))
  );
}

function isCharterItems(value: unknown, required: boolean): value is string[] {
  if (!Array.isArray(value) || value.length > 20 || (required && value.length === 0)) return false;
  const distinct = new Set<string>();
  for (const item of value) {
    if (!isTrimmedText(item, 300) || /\p{Cc}/u.test(item) || distinct.has(item)) return false;
    distinct.add(item);
  }
  return true;
}

function isTrimmedText(value: unknown, maxCharacters: number): value is string {
  return (
    typeof value === "string" &&
    value.trim() === value &&
    value.length > 0 &&
    Array.from(value).length <= maxCharacters
  );
}

function isTrimmedTextWithoutControls(value: unknown, maxCharacters: number): value is string {
  return isTrimmedText(value, maxCharacters) && !/\p{Cc}/u.test(value);
}

function isAutonomyProfile(value: unknown): value is AutonomyProfile {
  return value === "observe" || value === "supervised";
}

function isPositiveInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 1;
}

function isNonnegativeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isUuid(value: unknown): value is string {
  return (
    typeof value === "string" &&
    /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(value)
  );
}

function hasExactKeys<K extends string>(
  value: unknown,
  expectedKeys: readonly K[],
): value is Record<K, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value).sort();
  return keys.length === expectedKeys.length && keys.every((key, index) => key === expectedKeys[index]);
}
