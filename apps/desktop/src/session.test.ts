import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { loadSessionStatus } from "./session";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);

describe("desktop session transport", () => {
  beforeEach(() => invokeMock.mockReset());

  it("accepts only the exact locked signed-out shape", async () => {
    const exactStatus = {
      phase: "operator_setup_required",
      workspaceAccess: false,
      operatorCommands: false,
    } as const;
    invokeMock.mockResolvedValue(exactStatus);

    await expect(loadSessionStatus()).resolves.toEqual(exactStatus);
    expect(invokeMock).toHaveBeenCalledWith("session_status");
  });

  it.each([
    [
      "workspace access",
      { phase: "operator_setup_required", workspaceAccess: true, operatorCommands: false },
    ],
    [
      "operator commands",
      { phase: "operator_setup_required", workspaceAccess: false, operatorCommands: true },
    ],
    [
      "undeclared fields",
      {
        phase: "operator_setup_required",
        workspaceAccess: false,
        operatorCommands: false,
        workspaceName: "Personal",
      },
    ],
    ["a non-object response", null],
  ])("rejects %s rather than widening the signed-out boundary", async (_case, response) => {
    invokeMock.mockResolvedValue(response);

    await expect(loadSessionStatus()).rejects.toThrow("invalid session status");
  });
});
