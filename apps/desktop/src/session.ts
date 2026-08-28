import { invoke } from "@tauri-apps/api/core";

export type DesktopSessionStatus = {
  phase: "operator_setup_required";
  workspaceAccess: false;
  operatorCommands: false;
};

// Tauri's current frontend command adapter returns a Promise for the Rust value.
// Source: https://v2.tauri.app/develop/calling-rust/#commands
export async function loadSessionStatus(): Promise<DesktopSessionStatus> {
  const value: unknown = await invoke("session_status");
  if (!isDesktopSessionStatus(value)) {
    throw new Error("Beacon received an invalid session status.");
  }
  return value;
}

function isDesktopSessionStatus(value: unknown): value is DesktopSessionStatus {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record).sort();
  return (
    keys.length === 3 &&
    keys[0] === "operatorCommands" &&
    keys[1] === "phase" &&
    keys[2] === "workspaceAccess" &&
    record.phase === "operator_setup_required" &&
    record.workspaceAccess === false &&
    record.operatorCommands === false
  );
}
