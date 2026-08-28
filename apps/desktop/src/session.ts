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

export async function loadProjects(): Promise<ProjectSummary[]> {
  return invokeAndValidate("projects", undefined, isProjectSummaries, "Project list");
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

function isProjectSummaries(value: unknown): value is ProjectSummary[] {
  return Array.isArray(value) && value.every(isProjectSummary);
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
