import { invoke } from "@tauri-apps/api/core";

type LockedPhase = "operator_setup_required" | "locked";

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

export async function loadPreflightReceipts(projectId: string): Promise<PreflightReceipt[]> {
  return invokeAndValidate(
    "preflight_receipts",
    { projectId },
    isPreflightReceiptList,
    "preflight Receipt list",
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

export function describeLocalAccountError(error: unknown): string {
  if (typeof error !== "object" || error === null || Array.isArray(error)) {
    return "Beacon could not complete that local account action.";
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
      return "Beacon could not complete that local account action.";
  }
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
    typeof value.startedAtMs === "number" &&
    typeof value.endedAtMs === "number" &&
    Array.isArray(value.notVerified) &&
    value.notVerified.every(isString) &&
    isUnavailableEvidence(value.unavailable)
  );
}

function isPreflightReceiptList(value: unknown): value is PreflightReceipt[] {
  return Array.isArray(value) && value.every(isPreflightReceipt);
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
