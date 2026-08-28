import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  beginLocalPasswordSetup,
  acceptStarterOrganization,
  completeLocalOperatorSetup,
  describeLocalAccountError,
  discardStarterOrganization,
  loadAuthenticationCapabilities,
  loadPlatformSessionMonitorStatus,
  loadSessionPolicy,
  loadSessionStatus,
  loadStarterOrganization,
  lockLocalSession,
  recordLocalOperatorActivity,
  recoverLocalOperatorAccount,
  setLocalSessionTimeout,
  signInWithLocalPassword,
} from "./session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("desktop session transport", () => {
  beforeEach(() => invokeMock.mockReset());

  it.each([
    ["operator_setup_required", false, false],
    ["locked", false, false],
    ["unlocked", true, true],
  ] as const)("accepts the exact %s status shape", async (phase, workspaceAccess, operatorCommands) => {
    const exactStatus = { phase, workspaceAccess, operatorCommands };
    invokeMock.mockResolvedValue(exactStatus);

    await expect(loadSessionStatus()).resolves.toEqual(exactStatus);
    expect(invokeMock).toHaveBeenCalledWith("session_status");
  });

  it.each([
    ["mismatched setup access", { phase: "operator_setup_required", workspaceAccess: true, operatorCommands: false }],
    ["mismatched unlocked access", { phase: "unlocked", workspaceAccess: false, operatorCommands: true }],
    ["undeclared fields", { phase: "locked", workspaceAccess: false, operatorCommands: false, workspaceName: "Personal" }],
    ["a non-object response", null],
  ])("rejects %s rather than widening the session boundary", async (_case, response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadSessionStatus()).rejects.toThrow("invalid session status");
  });

  it("accepts only the closed authentication-capability shape", async () => {
    const capabilities = {
      preferredMethod: "passkey",
      passkey: { available: false, reason: "unsupported_linux_webkitgtk" },
      passwordFallback: { available: true, scope: "same_machine" },
    } as const;
    invokeMock.mockResolvedValue(capabilities);

    await expect(loadAuthenticationCapabilities()).resolves.toEqual(capabilities);
    expect(invokeMock).toHaveBeenCalledWith("authentication_capabilities");
  });

  it("sends secrets only in the command that consumes them and validates one-time output", async () => {
    invokeMock.mockResolvedValueOnce({ recoveryKey: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567" });

    await expect(beginLocalPasswordSetup("one secret", "one secret")).resolves.toEqual({
      recoveryKey: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567",
    });
    expect(invokeMock).toHaveBeenCalledWith("begin_password_setup", {
      password: "one secret",
      confirmation: "one secret",
    });
  });

  it("maps setup, sign-in, recovery, and lock to narrow local commands", async () => {
    const unlocked = { phase: "unlocked", workspaceAccess: true, operatorCommands: true };
    const locked = { phase: "locked", workspaceAccess: false, operatorCommands: false };
    invokeMock
      .mockResolvedValueOnce(unlocked)
      .mockResolvedValueOnce(unlocked)
      .mockResolvedValueOnce({ recoveryKey: "NEW-RECOVERY-KEY" })
      .mockResolvedValueOnce(locked);

    await expect(completeLocalOperatorSetup(true)).resolves.toEqual(unlocked);
    await expect(signInWithLocalPassword("local password")).resolves.toEqual(unlocked);
    await expect(
      recoverLocalOperatorAccount("old key", "new password", "new password"),
    ).resolves.toEqual({ recoveryKey: "NEW-RECOVERY-KEY" });
    await expect(lockLocalSession()).resolves.toEqual(locked);

    expect(invokeMock.mock.calls).toEqual([
      ["complete_operator_setup", { recoverySaved: true }],
      ["authenticate_password", { password: "local password" }],
      [
        "recover_operator_account",
        { recoveryKey: "old key", newPassword: "new password", confirmation: "new password" },
      ],
      ["lock_session"],
    ]);
  });

  it("validates session policy and platform monitor responses exactly", async () => {
    invokeMock
      .mockResolvedValueOnce({ inactivityTimeoutMinutes: 30 })
      .mockResolvedValueOnce({ inactivityTimeoutMinutes: 90 })
      .mockResolvedValueOnce({ screenLock: "active", suspend: "unavailable" })
      .mockResolvedValueOnce(null);

    await expect(loadSessionPolicy()).resolves.toEqual({ inactivityTimeoutMinutes: 30 });
    await expect(setLocalSessionTimeout(90)).resolves.toEqual({ inactivityTimeoutMinutes: 90 });
    await expect(loadPlatformSessionMonitorStatus()).resolves.toEqual({
      screenLock: "active",
      suspend: "unavailable",
    });
    await expect(recordLocalOperatorActivity()).resolves.toBeUndefined();
    expect(invokeMock.mock.calls).toEqual([
      ["session_policy"],
      ["set_session_timeout", { inactivityTimeoutMinutes: 90 }],
      ["platform_session_monitor_status"],
      ["record_operator_activity"],
    ]);
  });

  it("maps the Starter Organization commands and rejects widened roster results", async () => {
    const proposed = {
      workspaceId: "11111111-1111-4111-8111-111111111111",
      proposalVersion: 1,
      decision: "proposed",
      templates: [
        { role: "lead", description: "Coordinates Missions.", recommended: true },
        { role: "builder", description: "Produces Artifacts.", recommended: true },
        { role: "reviewer", description: "Evaluates work.", recommended: true },
        { role: "researcher", description: "Develops Evidence.", recommended: false },
      ],
      residents: [],
    } as const;
    const accepted = {
      ...proposed,
      decision: "accepted",
      residents: [
        {
          id: "22222222-2222-4222-8222-222222222222",
          role: "lead",
          displayName: "Northstar",
          lifecycle: "inactive",
        },
      ],
    } as const;
    const discarded = { ...proposed, decision: "discarded" } as const;
    invokeMock
      .mockResolvedValueOnce(proposed)
      .mockResolvedValueOnce(accepted)
      .mockResolvedValueOnce(discarded)
      .mockResolvedValueOnce({ ...proposed, authority: "ambient" });

    await expect(loadStarterOrganization()).resolves.toEqual(proposed);
    await expect(
      acceptStarterOrganization(1, [{ role: "lead", displayName: "Northstar" }]),
    ).resolves.toEqual(accepted);
    await expect(discardStarterOrganization(1)).resolves.toEqual(discarded);
    await expect(loadStarterOrganization()).rejects.toThrow("invalid Starter Organization");
    expect(invokeMock.mock.calls).toEqual([
      ["starter_organization"],
      [
        "accept_starter_organization",
        { proposalVersion: 1, residents: [{ role: "lead", displayName: "Northstar" }] },
      ],
      ["discard_starter_organization", { proposalVersion: 1 }],
      ["starter_organization"],
    ]);
  });

  it.each([
    [
      "changed recommendation defaults",
      {
        workspaceId: "11111111-1111-4111-8111-111111111111",
        proposalVersion: 1,
        decision: "proposed",
        templates: [
          { role: "lead", description: "Coordinates Missions.", recommended: false },
          { role: "builder", description: "Produces Artifacts.", recommended: true },
          { role: "reviewer", description: "Evaluates work.", recommended: true },
          { role: "researcher", description: "Develops Evidence.", recommended: false },
        ],
        residents: [],
      },
    ],
    [
      "a control character in a Display Name",
      {
        workspaceId: "11111111-1111-4111-8111-111111111111",
        proposalVersion: 1,
        decision: "accepted",
        templates: [
          { role: "lead", description: "Coordinates Missions.", recommended: true },
          { role: "builder", description: "Produces Artifacts.", recommended: true },
          { role: "reviewer", description: "Evaluates work.", recommended: true },
          { role: "researcher", description: "Develops Evidence.", recommended: false },
        ],
        residents: [
          {
            id: "22222222-2222-4222-8222-222222222222",
            role: "lead",
            displayName: "North\nstar",
            lifecycle: "inactive",
          },
        ],
      },
    ],
  ])("rejects %s in a Starter Organization result", async (_case, response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadStarterOrganization()).rejects.toThrow("invalid Starter Organization");
  });

  it.each([
    { inactivityTimeoutMinutes: 0 },
    { inactivityTimeoutMinutes: 1_441 },
    { inactivityTimeoutMinutes: 30, extra: true },
  ])("rejects an invalid session policy response", async (response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadSessionPolicy()).rejects.toThrow("invalid session policy");
  });

  it("maps only known local error codes to Operator-facing guidance", () => {
    expect(describeLocalAccountError({ code: "password_confirmation_mismatch" })).toBe(
      "The password confirmation does not match.",
    );
    expect(
      describeLocalAccountError({ code: "authentication_throttled", retryAfterMs: 1_001 }),
    ).toBe("Wait 2 seconds before trying again.");
    expect(describeLocalAccountError({ code: "starter_organization_unavailable" })).toBe(
      "Beacon could not verify the Starter Organization state.",
    );
    expect(describeLocalAccountError({ code: "invented", message: "render me" })).not.toContain(
      "render me",
    );
  });
});
