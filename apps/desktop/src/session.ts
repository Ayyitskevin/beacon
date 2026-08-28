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

function hasExactKeys<K extends string>(
  value: unknown,
  expectedKeys: readonly K[],
): value is Record<K, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const keys = Object.keys(value).sort();
  return keys.length === expectedKeys.length && keys.every((key, index) => key === expectedKeys[index]);
}
