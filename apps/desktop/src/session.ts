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
