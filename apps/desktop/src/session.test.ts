import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  beginLocalPasswordSetup,
  completeLocalOperatorSetup,
  describeLocalAccountError,
  loadAuthenticationCapabilities,
  loadSessionStatus,
  lockLocalSession,
  recoverLocalOperatorAccount,
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

  it("maps only known local error codes to Operator-facing guidance", () => {
    expect(describeLocalAccountError({ code: "password_confirmation_mismatch" })).toBe(
      "The password confirmation does not match.",
    );
    expect(
      describeLocalAccountError({ code: "authentication_throttled", retryAfterMs: 1_001 }),
    ).toBe("Wait 2 seconds before trying again.");
    expect(describeLocalAccountError({ code: "invented", message: "render me" })).not.toContain(
      "render me",
    );
  });
});
