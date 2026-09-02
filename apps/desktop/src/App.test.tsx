import { invoke } from "@tauri-apps/api/core";
import axe from "axe-core";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const originalMatchMedia = window.matchMedia;
const capabilities = {
  preferredMethod: "passkey",
  passkey: { available: false, reason: "unsupported_linux_webkitgtk" },
  passwordFallback: { available: true, scope: "same_machine" },
} as const;
const sessionPolicy = { inactivityTimeoutMinutes: 30 } as const;
const monitorStatus = { screenLock: "active", suspend: "active" } as const;
const starterTemplates = [
  { role: "lead", description: "Coordinates Missions.", recommended: true },
  { role: "builder", description: "Produces Artifacts.", recommended: true },
  { role: "reviewer", description: "Evaluates work.", recommended: true },
  { role: "researcher", description: "Develops Evidence.", recommended: false },
] as const;
const proposedOrganization = {
  workspaceId: "11111111-1111-4111-8111-111111111111",
  proposalVersion: 1,
  decision: "proposed",
  templates: starterTemplates,
  residents: [],
} as const;
const acceptedOrganization = {
  ...proposedOrganization,
  decision: "accepted",
  residents: [
    {
      id: "22222222-2222-4222-8222-222222222222",
      role: "lead",
      displayName: null,
      lifecycle: "inactive",
    },
  ],
} as const;
const inactiveResident = {
  id: acceptedOrganization.residents[0].id,
  workspaceId: acceptedOrganization.workspaceId,
  role: acceptedOrganization.residents[0].role,
  displayName: acceptedOrganization.residents[0].displayName,
  lifecycle: "inactive",
  latestDecisionId: null,
  createdAtMs: 100,
  updatedAtMs: 100,
} as const;
const residentRoster = [inactiveResident] as const;
const localModelReport = {
  status: "available",
  endpoint: "http://127.0.0.1:11434",
  observedAtMs: 123,
  models: [
    {
      provider: "ollama",
      activation: "inactive",
      name: "qwen3.8:27b",
      digest: "22130167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
      sizeBytes: 17_741_872_154,
      modifiedAt: "2026-08-14T14:38:20.13373148-04:00",
      details: {
        format: "gguf",
        family: "qwen35",
        parameterSize: "27.3B",
        quantizationLevel: "Q4_K_M",
      },
    },
  ],
} as const;
const localModelReview = {
  binding: {
    provider: "ollama",
    endpoint: localModelReport.endpoint,
    modelName: localModelReport.models[0].name,
    modelDigest: localModelReport.models[0].digest,
    inspectionDigest:
      "12340167c4c20e20c7b71454612966ca8e8171e9b3cc8ab6ce8aa6cbfec79643",
  },
  providerFeatures: ["completion", "tools"],
  unrecognizedFeatures: ["future_audio"],
} as const;
const activeLocalModel = {
  id: "44444444-4444-4444-8444-444444444444",
  workspaceId: proposedOrganization.workspaceId,
  ...localModelReview,
  approvedFeatures: ["completion"],
  state: "active",
  predecessorId: null,
  decidedAtMs: 123,
} as const;
const residentCapabilityCatalog = [
  {
    capability: "work.create_mission",
    label: "Create Missions",
    description: "Request Mission creation through Work Policy.",
  },
  {
    capability: "work.form_team",
    label: "Form Teams",
    description: "Request Team formation through Work Policy.",
  },
  {
    capability: "work.coordinate_mission",
    label: "Coordinate Missions",
    description: "Coordinate accepted Mission work through Work Policy.",
  },
  {
    capability: "work.assign_work",
    label: "Assign Work",
    description: "Assign bounded work through Work Policy.",
  },
  {
    capability: "artifact.produce",
    label: "Produce Artifacts",
    description: "Produce reviewable Mission artifacts.",
  },
  {
    capability: "artifact.evaluate",
    label: "Evaluate Artifacts",
    description: "Evaluate artifacts without approving effects.",
  },
  {
    capability: "evidence.develop",
    label: "Develop Evidence",
    description: "Develop evidence for operator review.",
  },
] as const;

const briefingProjects = [
  {
    id: "44444444-4444-4444-8444-444444444444",
    name: "Task tracker",
    charterRevision: 1,
    autonomyProfile: "observe",
    createdAtMs: 10,
  },
  {
    id: "55555555-5555-4555-8555-555555555555",
    name: "Ledger",
    charterRevision: 1,
    autonomyProfile: "observe",
    createdAtMs: 20,
  },
] as const;

const receiptAbsent = (reason: string) => ({ kind: "absent", reason }) as const;

function briefingReceipt(
  runId: string,
  projectId: string,
  outcome: string,
  startedAtMs: number,
) {
  return {
    version: 1,
    runId,
    workspaceId: acceptedOrganization.workspaceId,
    projectId,
    repositoryRevision: 1,
    charterRevision: 1,
    outcome,
    sourceKind: "local_path",
    source: "/private/beacon/repo",
    baseRevision: "0123456789012345678901234567890123456789",
    resolvedChecks: [],
    sandboxVersion: "landlock-v1",
    policyCharterRevision: 1,
    policyRepositoryRevision: 1,
    startedAtMs,
    endedAtMs: startedAtMs + 1,
    notVerified: ["preflight refuses to execute, so nothing ran"],
    unavailable: {
      proposalDigest: receiptAbsent("no Proposal surface exists"),
      rulingDigest: receiptAbsent("no Proposal surface exists"),
      resultingRevision: receiptAbsent("preflight refuses to execute, so nothing ran"),
      filesystemEffects: receiptAbsent("preflight refuses to execute, so nothing ran"),
      diffDigest: receiptAbsent("preflight refuses to execute, so nothing ran"),
      modelInvocations: receiptAbsent("preflight performs no model inference"),
      costUsdMicros: receiptAbsent("preflight performs no model inference"),
    },
  };
}

const receiptsByProject: Record<string, ReturnType<typeof briefingReceipt>[]> = {
  [briefingProjects[0].id]: [
    briefingReceipt("aaaaaaaa-1111-4111-8111-111111111111", briefingProjects[0].id, "verified", 100),
    briefingReceipt(
      "cccccccc-3333-4333-8333-333333333333",
      briefingProjects[0].id,
      "sandbox_unavailable",
      300,
    ),
  ],
  [briefingProjects[1].id]: [
    briefingReceipt(
      "bbbbbbbb-2222-4222-8222-222222222222",
      briefingProjects[1].id,
      "preflight_timed_out",
      200,
    ),
  ],
};

type Phase = "operator_setup_required" | "locked" | "unlocked";

function status(phase: Phase) {
  const unlocked = phase === "unlocked";
  return { phase, workspaceAccess: unlocked, operatorCommands: unlocked };
}

function mockInitialState(phase: Phase) {
  invokeMock.mockImplementation((command, args) => {
    if (command === "session_status") return Promise.resolve(status(phase));
    if (command === "authentication_capabilities") return Promise.resolve(capabilities);
    if (command === "session_policy") return Promise.resolve(sessionPolicy);
    if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
    if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
    if (command === "residents") return Promise.resolve(residentRoster);
    if (command === "record_operator_activity") return Promise.resolve(null);
    // Briefing reads its recent preflight Receipts by fanning out over the Projects.
    if (command === "projects") return Promise.resolve([]);
    if (command === "preflight_receipts") return Promise.resolve([]);
    if (command === "set_session_timeout") {
      return Promise.resolve({
        inactivityTimeoutMinutes: (args as { inactivityTimeoutMinutes: number })
          .inactivityTimeoutMinutes,
      });
    }
    return Promise.reject(new Error(`unexpected command: ${command}`));
  });
}

describe("Beacon desktop shell", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: originalMatchMedia,
    });
  });

  it("moves focus to main content from the skip link", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    await user.tab();
    const skipLink = screen.getByRole("link", { name: "Skip to main content" });
    expect(skipLink).toHaveFocus();

    await user.keyboard("{Enter}");
    expect(screen.getByRole("main")).toHaveFocus();
  });

  it.each(["{Enter}", " "])(
    "uses %s to enter Projects and continue past unavailable applications",
    async (activationKey) => {
      const user = userEvent.setup();
      mockInitialState("unlocked");
      const base = invokeMock.getMockImplementation();
      if (!base) throw new Error("initial invoke mock is unavailable");
      invokeMock.mockImplementation((command, args) => {
        if (command === "projects") return Promise.resolve([]);
        return base(command, args);
      });

      render(<App />);
      await screen.findByRole("heading", { name: "Briefing" });
      const suiteNavigation = screen.getByRole("navigation", { name: "Beacon applications" });
      const briefing = within(suiteNavigation).getByRole("button", { name: "Briefing" });
      const projects = within(suiteNavigation).getByRole("button", { name: "Projects" });

      await user.tab();
      await user.tab();
      expect(briefing).toHaveFocus();
      await user.tab();
      expect(projects).toHaveFocus();
      for (const application of ["Conversations", "Knowledge", "Software"]) {
        expect(within(suiteNavigation).getByRole("button", { name: application })).toBeDisabled();
      }

      await user.keyboard(activationKey);
      expect(await screen.findByRole("heading", { name: "Projects" })).toBeVisible();
      expect(projects).toHaveAttribute("aria-current", "page");
      await user.tab();
      expect(screen.getByRole("button", { name: "Create Project" })).toHaveFocus();
    },
  );

  it("exposes the opt-in Sample Project from the authenticated Projects application", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation();
    if (!base) throw new Error("initial invoke mock is unavailable");
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve([]);
      if (command === "sample_project_status") {
        return Promise.resolve({
          state: "available",
          bundleVersion: 1,
          bundleDigest: {
            algorithm: "sha256",
            canonicalizer: "beacon.sample-project.bundle.v1",
            value: "7bdcf0a732856553c529dee2ec7dfcf9bbf691a5fb9b2558e74f05c1559dde8f",
          },
        });
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));

    expect(await screen.findByText("Optional local proving project")).toBeVisible();
    expect(screen.getByRole("button", { name: "Install Sample Project" })).toBeVisible();
  });

  it("keeps Workspace material and Operator Commands closed before setup", async () => {
    mockInitialState("operator_setup_required");

    render(<App />);

    expect(
      await screen.findByRole("heading", { name: "Set up your operator account" }),
    ).toBeVisible();
    expect(invokeMock).toHaveBeenCalledWith("session_status");
    expect(await screen.findByText(/Passkeys are not available on this Linux build/i)).toBeVisible();
    expect(screen.getByText("Workspace access locked")).toBeVisible();
    const suiteNavigation = screen.getByRole("navigation", { name: "Beacon applications" });
    for (const button of within(suiteNavigation).getAllByRole("button")) {
      expect(button).toBeDisabled();
    }
    expect(screen.queryByText("Personal")).not.toBeInTheDocument();
    expect(document.body).not.toHaveTextContent(/workspace[_ -]id/i);
  });

  it("enrolls the Owner only after the one-time Recovery Key is confirmed saved", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("operator_setup_required"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "begin_password_setup") {
        return Promise.resolve({ recoveryKey: "ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567" });
      }
      if (command === "complete_operator_setup") return Promise.resolve(status("unlocked"));
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Set up your operator account" });
    await user.type(screen.getByLabelText("Password"), "a long local passphrase");
    await user.type(screen.getByLabelText("Confirm password"), "a long local passphrase");
    await user.click(screen.getByRole("button", { name: "Create recovery key" }));

    expect(screen.getByText("ABCD-EFGH-IJKL-MNOP-QRST-UVWX-YZ23-4567")).toBeVisible();
    expect(screen.getByRole("button", { name: "Finish setup" })).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /saved this Recovery Key offline/i }));
    await user.click(screen.getByRole("button", { name: "Finish setup" }));

    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    expect(invokeMock).toHaveBeenCalledWith("complete_operator_setup", { recoverySaved: true });
    const suiteNavigation = screen.getByRole("navigation", { name: "Beacon applications" });
    expect(within(suiteNavigation).getByRole("button", { name: "Briefing" })).toBeEnabled();
    expect(within(suiteNavigation).getByRole("button", { name: "Projects" })).toBeEnabled();
    expect(within(suiteNavigation).getByRole("button", { name: "Conversations" })).toBeDisabled();
    expect(within(suiteNavigation).getByRole("button", { name: "Knowledge" })).toBeDisabled();
    expect(within(suiteNavigation).getByRole("button", { name: "Software" })).toBeDisabled();
  });

  it("shows Briefing's recent preflight Receipts merged across Projects, newest first", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve(briefingProjects);
      if (command === "preflight_receipts") {
        const { projectId } = args as { projectId: string };
        return Promise.resolve(receiptsByProject[projectId] ?? []);
      }
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    const list = await screen.findByRole("list", { name: "Recent preflight Receipts" });
    const rows = within(list).getAllByRole("listitem");
    // Two Projects, merged. Per-Project order would put the tracker's older
    // Receipt above the ledger's newer one.
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("Run cccccccc"),
      expect.stringContaining("Run bbbbbbbb"),
      expect.stringContaining("Run aaaaaaaa"),
    ]);
    expect(rows[1].textContent).toContain("Ledger");

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("bounds the Receipt fan-out and says the reading is partial", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const many = Array.from({ length: 513 }, (_, index) => ({
      id: `${index.toString(16).padStart(8, "0")}-4444-4444-8444-444444444444`,
      name: `Project ${index}`,
      charterRevision: 1,
      autonomyProfile: "observe" as const,
      createdAtMs: index,
    }));
    let reads = 0;
    let concurrent = 0;
    let peak = 0;
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve(many);
      if (command === "preflight_receipts") {
        reads += 1;
        concurrent += 1;
        peak = Math.max(peak, concurrent);
        return new Promise((resolve) => {
          setTimeout(() => {
            concurrent -= 1;
            resolve([]);
          }, 0);
        });
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    // A Workspace with 513 Projects used to start 513 concurrent IPC calls and
    // hold up to N x 512 Receipts before sorting to five.
    expect(
      await screen.findByText(/this reading covered the 8 most recent Projects of 513/),
    ).toBeVisible();
    // The scope sentence says what the READING covered. "showing ... Receipts
    // for" made it a claim about what is on screen, which is the verb that
    // contradicted the row list in the capped case below.
    expect(screen.queryByText(/^showing /)).toBeNull();
    expect(reads).toBe(8);
    // ONE page in flight, because a page in flight is a page held: a window of
    // four concurrent reads of a 512-row page held 2,048 rows beside the 256
    // kept, three times the documented bound.
    expect(peak).toBe(1);
  });

  it("holds only the rows it will draw and reports the true number found", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const eight = Array.from({ length: 8 }, (_, index) => ({
      id: `${index.toString(16).padStart(8, "0")}-4444-4444-8444-444444444444`,
      name: `Project ${index}`,
      charterRevision: 1,
      autonomyProfile: "observe" as const,
      createdAtMs: index,
    }));
    const page = (projectId: string, base: number) =>
      Array.from({ length: 512 }, (_, index) =>
        briefingReceipt(
          `${(base + index).toString(16).padStart(8, "0")}-1111-4111-8111-111111111111`,
          projectId,
          "verified",
          base + index,
        ),
      );
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve(eight);
      if (command === "preflight_receipts") {
        const { projectId } = args as { projectId: string };
        return Promise.resolve(page(projectId, eight.findIndex((p) => p.id === projectId) * 1000));
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    const list = await screen.findByRole("list", { name: "Recent preflight Receipts" });

    // 8 x 512 = 4,096 Receipts exist. Capping to 256 and then reporting 256 as
    // the total made 3,840 read rows vanish from the disclosure.
    //
    // Two sentences, two subjects, and they must not contradict each other:
    // the panel makes ONE display claim, and the scope line describes the
    // reading that claim was drawn from. "showing 256 most recent preflight
    // Receipts" said 256 rows were on screen while five were, and every number
    // in it was individually true.
    expect(await screen.findByText(/Showing the 5 most recent of 4096/)).toBeVisible();
    expect(
      screen.getByText(
        /this reading covered 8 of 8 Projects and kept the 256 most recent Receipts of the 4096 it found/,
      ),
    ).toBeVisible();
    expect(within(list).getAllByRole("listitem")).toHaveLength(5);
    // No sentence claims the retained rows are the displayed ones.
    expect(screen.queryByText(/showing 256/)).toBeNull();
    expect(within(list).getAllByRole("listitem")).toHaveLength(5);
  });

  it("never presents a Receipt as work that succeeded", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve(briefingProjects);
      if (command === "preflight_receipts") {
        const { projectId } = args as { projectId: string };
        return Promise.resolve(receiptsByProject[projectId] ?? []);
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    const list = await screen.findByRole("list", { name: "Recent preflight Receipts" });

    // A Receipt records what a Run did. Preflight runs no check, so nothing
    // here may read as a completed or passing one.
    expect(list.textContent).not.toMatch(/passed|succeeded|completed|✓/i);
    expect(within(list).getByText(/not a passing check/)).toBeVisible();
  });

  it("keeps a failed Receipt read apart from a Workspace with no Receipts", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    let attempts = 0;
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve(briefingProjects);
      if (command === "preflight_receipts") {
        attempts += 1;
        if (attempts === 1) return Promise.reject({ code: "preflight_unavailable" });
        return Promise.resolve([]);
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    expect(await screen.findByText(/Recent preflight Receipts could not be read/)).toBeVisible();
    expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();

    // The failure is retryable rather than terminal, and the retry can reach
    // the genuinely-empty state.
    await user.click(screen.getByRole("button", { name: "Refresh preflight Receipts" }));
    expect(await screen.findByText("No preflight Receipts yet.")).toBeVisible();
    expect(screen.queryByText(/Recent preflight Receipts could not be read/)).toBeNull();
  });

  it("discovers installed local models as inactive resources from Briefing", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command, args) => {
      if (command === "discover_local_models") return Promise.resolve(localModelReport);
      if (command === "local_model_activations") return Promise.resolve([]);
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "set_session_timeout") {
        return Promise.resolve({
          inactivityTimeoutMinutes: (args as { inactivityTimeoutMinutes: number })
            .inactivityTimeoutMinutes,
        });
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    expect(screen.getByText("Not checked yet")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Check local models" }));

    expect(await screen.findByText("qwen3.8:27b")).toBeVisible();
    expect(screen.getByText("Inactive")).toBeVisible();
    expect(screen.getByText(/does not activate or run them/i)).toBeVisible();
    expect(invokeMock).toHaveBeenCalledWith("discover_local_models");
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("reviews advertised features before activation and deactivates without running a model", async () => {
    const user = userEvent.setup();
    const inactive = {
      ...activeLocalModel,
      id: "55555555-5555-4555-8555-555555555555",
      state: "inactive",
      predecessorId: activeLocalModel.id,
      decidedAtMs: 124,
    } as const;
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "discover_local_models") return Promise.resolve(localModelReport);
      if (command === "local_model_activations") return Promise.resolve([]);
      if (command === "inspect_local_model") return Promise.resolve(localModelReview);
      if (command === "activate_local_model") return Promise.resolve(activeLocalModel);
      if (command === "deactivate_local_model") return Promise.resolve(inactive);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Check local models" }));
    await user.click(await screen.findByRole("button", { name: "Review qwen3.8:27b" }));

    expect(await screen.findByRole("heading", { name: "Review qwen3.8:27b" })).toBeVisible();
    expect(screen.getByText(/advertised by Ollama, not Beacon permissions/i)).toBeVisible();
    expect(screen.getByText(/future_audio.*cannot be approved/i)).toBeVisible();
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
    await user.click(screen.getByRole("checkbox", { name: "Approve tools" }));
    await user.click(screen.getByRole("button", { name: "Activate for this Workspace" }));

    expect(invokeMock).toHaveBeenCalledWith("activate_local_model", {
      review: localModelReview,
      approvedFeatures: ["completion"],
    });
    expect(await screen.findByText("Active")).toBeVisible();
    expect(screen.getByText(/does not create a Route or run the model/i)).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Deactivate qwen3.8:27b" }));
    expect(invokeMock).toHaveBeenCalledWith("deactivate_local_model", {
      expectedActivationId: activeLocalModel.id,
    });
    expect(await screen.findByText("Inactive")).toBeVisible();
  });

  it("keeps an active decision visible and revocable while Ollama is offline", async () => {
    const user = userEvent.setup();
    const unavailable = {
      status: "unavailable",
      endpoint: localModelReport.endpoint,
      observedAtMs: 125,
      models: [],
    } as const;
    const inactive = {
      ...activeLocalModel,
      id: "55555555-5555-4555-8555-555555555555",
      state: "inactive",
      predecessorId: activeLocalModel.id,
      decidedAtMs: 126,
    } as const;
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "discover_local_models") return Promise.resolve(unavailable);
      if (command === "local_model_activations") return Promise.resolve([activeLocalModel]);
      if (command === "deactivate_local_model") return Promise.resolve(inactive);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Check local models" }));

    expect(await screen.findByText("Active")).toBeVisible();
    expect(screen.getByText(/Ollama is not available/i)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Deactivate qwen3.8:27b" }));

    expect(invokeMock).toHaveBeenCalledWith("deactivate_local_model", {
      expectedActivationId: activeLocalModel.id,
    });
    expect(await screen.findByText("Inactive")).toBeVisible();
  });

  it("creates a Project from an explicit Charter without granting effects or spend", async () => {
    const user = userEvent.setup();
    const created = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: proposedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter: {
        revision: 1,
        purpose: "Build a trustworthy local software product.",
        desiredOutcomes: ["Ship a verified vertical slice", "Preserve operator control"],
        constraints: ["Keep Workspace data local"],
        prohibitions: ["Do not deploy publicly"],
        autonomyProfile: "supervised",
        budgetBoundary: "no_spend",
        externalEffects: "prohibited",
        authoredAtMs: 123,
      },
      createdAtMs: 123,
    } as const;
    const revised = {
      ...created,
      charter: {
        ...created.charter,
        revision: 2,
        purpose: "Build a trustworthy local software factory.",
        desiredOutcomes: ["Publish an evaluated change"],
        autonomyProfile: "observe",
        authoredAtMs: 124,
      },
    } as const;
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command, args) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "projects") return Promise.resolve([]);
      if (command === "create_project") return Promise.resolve(created);
      if (command === "revise_project_charter") return Promise.resolve(revised);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command} ${JSON.stringify(args)}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    expect(await screen.findByRole("heading", { name: "Projects" })).toBeVisible();
    expect(screen.getByText("No Projects yet")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Create Project" }));

    await user.type(screen.getByLabelText("Project name"), "Beacon Alpha");
    await user.type(
      screen.getByLabelText("Purpose"),
      "Build a trustworthy local software product.",
    );
    await user.type(
      screen.getByLabelText("Desired outcomes"),
      "Ship a verified vertical slice{enter}Preserve operator control",
    );
    await user.type(screen.getByLabelText("Constraints"), "Keep Workspace data local");
    await user.type(screen.getByLabelText("Prohibitions"), "Do not deploy publicly");
    expect(screen.getByRole("radio", { name: /Supervised/i })).toBeChecked();
    expect(screen.getByText("No spend")).toBeVisible();
    expect(screen.getByText("External effects prohibited")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Create Project" }));

    expect(invokeMock).toHaveBeenCalledWith("create_project", {
      draft: {
        name: "Beacon Alpha",
        charter: {
          purpose: "Build a trustworthy local software product.",
          desiredOutcomes: ["Ship a verified vertical slice", "Preserve operator control"],
          constraints: ["Keep Workspace data local"],
          prohibitions: ["Do not deploy publicly"],
          autonomyProfile: "supervised",
        },
      },
    });
    expect(await screen.findByRole("heading", { name: "Beacon Alpha" })).toBeVisible();
    expect(screen.getByText("Charter revision 1")).toBeVisible();
    expect(screen.getByText("No work has started.")).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Revise Charter" }));
    const purpose = screen.getByLabelText("Purpose");
    await user.clear(purpose);
    await user.type(purpose, "Build a trustworthy local software factory.");
    const outcomes = screen.getByLabelText("Desired outcomes");
    await user.clear(outcomes);
    await user.type(outcomes, "Publish an evaluated change");
    await user.click(screen.getByRole("radio", { name: /Observe/i }));
    await user.click(screen.getByRole("button", { name: "Publish revision 2" }));

    expect(invokeMock).toHaveBeenCalledWith("revise_project_charter", {
      projectId: created.id,
      expectedCurrentRevision: 1,
      draft: {
        purpose: "Build a trustworthy local software factory.",
        desiredOutcomes: ["Publish an evaluated change"],
        constraints: ["Keep Workspace data local"],
        prohibitions: ["Do not deploy publicly"],
        autonomyProfile: "observe",
      },
    });
    expect(await screen.findByText("Charter revision 2")).toBeVisible();
    expect(screen.getByText("2 preserved revisions")).toBeVisible();
    expect(screen.getAllByText("Build a trustworthy local software factory.")).toHaveLength(2);
  });

  it("opens an existing Project with its complete preserved Charter history", async () => {
    const user = userEvent.setup();
    const revisionOne = {
      revision: 1,
      purpose: "Build a trustworthy local software product.",
      desiredOutcomes: ["Ship a verified vertical slice"],
      constraints: ["Keep Workspace data local"],
      prohibitions: ["Do not deploy publicly"],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 9_000_000_000_000_000,
    } as const;
    const revisionTwo = {
      ...revisionOne,
      revision: 2,
      purpose: "Build a trustworthy local software factory.",
      autonomyProfile: "observe",
      authoredAtMs: 9_000_000_000_000_000,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: proposedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter: revisionTwo,
      createdAtMs: 123,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: 2,
      autonomyProfile: "observe",
      createdAtMs: project.createdAtMs,
    } as const;
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") {
        return Promise.resolve([revisionOne, revisionTwo]);
      }
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));

    expect(await screen.findByText("Charter revision 2")).toBeVisible();
    await user.click(screen.getByText("2 preserved revisions"));
    expect(screen.getByText("Revision 2 (current)")).toBeVisible();
    expect(screen.getByText("Revision 1")).toBeVisible();
    expect(screen.getByText("Build a trustworthy local software product.")).toBeVisible();
    expect(screen.getAllByText("Recorded time unavailable")).toHaveLength(2);
    expect(invokeMock).toHaveBeenCalledWith("project_charter_history", {
      projectId: project.id,
    });
  });

  it("publishes Team Lead selection criteria without selecting a Lead or granting authority", async () => {
    const user = userEvent.setup();
    const charter = {
      revision: 2,
      purpose: "Build a trustworthy local software factory.",
      desiredOutcomes: ["Ship an attributable Mission"],
      constraints: ["Keep Workspace data local"],
      prohibitions: ["Do not deploy publicly"],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 123,
    } as const;
    const priorCharter = {
      ...charter,
      revision: 1,
      authoredAtMs: 122,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter,
      createdAtMs: 120,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: charter.revision,
      autonomyProfile: charter.autonomyProfile,
      createdAtMs: project.createdAtMs,
    } as const;
    const published = {
      workspaceId: project.workspaceId,
      projectId: project.id,
      revision: 1,
      charterRevision: charter.revision,
      authoredBy: "operator",
      authoredAtMs: 124,
      appliesToCurrentCharter: true,
      teamLeadSelection: {
        requiredCapabilities: ["work.coordinate_mission"],
        availabilityRule: "current_team_eligibility",
        trackRecordRequirements: [
          { evidenceKind: "mission_outcome", minimumCount: 1 },
          { evidenceKind: "evaluation", minimumCount: 2 },
        ],
        conflictRule: "require_no_declared_conflict",
        ambiguityRule: "require_operator_decision",
      },
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([priorCharter, charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "publish_project_policy") return Promise.resolve(published);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));

    expect(await screen.findByText("No Team Lead Policy has been published.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Publish Team Lead Policy" }));
    expect(await screen.findByRole("form", { name: "Publish Team Lead Policy" })).toBeVisible();
    expect(screen.getByRole("checkbox", { name: /Coordinate Missions.*required/i })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Coordinate Missions.*required/i })).toBeDisabled();
    expect(screen.getByRole("checkbox", { name: "Mission outcomes" })).toBeChecked();
    await user.click(screen.getByRole("checkbox", { name: "Evaluations" }));
    const evaluationCount = screen.getByLabelText("Evaluation minimum count");
    await user.clear(evaluationCount);
    await user.type(evaluationCount, "2");
    expect(screen.getByText("This does not select a Lead or grant authority.")).toBeVisible();
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Publish Policy revision 1" }));

    expect(invokeMock).toHaveBeenCalledWith("publish_project_policy", {
      projectId: project.id,
      expectedCharterRevision: charter.revision,
      expectedPolicyRevision: null,
      draft: {
        teamLeadSelection: {
          requiredCapabilities: ["work.coordinate_mission"],
          availabilityRule: "current_team_eligibility",
          trackRecordRequirements: [
            { evidenceKind: "mission_outcome", minimumCount: 1 },
            { evidenceKind: "evaluation", minimumCount: 2 },
          ],
          conflictRule: "require_no_declared_conflict",
          ambiguityRule: "require_operator_decision",
        },
      },
    });
    expect(await screen.findByText(/^Policy revision 1/)).toBeVisible();
    expect(screen.getByText("No Lead selected")).toBeVisible();
    expect(screen.getByText(/Operator decides whenever eligible candidates remain ambiguous/i)).toBeVisible();
  });

  it("binds a Project repository and its declared checks without opening the repository", async () => {
    const user = userEvent.setup();
    const charter = {
      revision: 2,
      purpose: "Build a trustworthy local software factory.",
      desiredOutcomes: ["Ship an attributable Mission"],
      constraints: ["Keep Workspace data local"],
      prohibitions: ["Do not deploy publicly"],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 123,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter,
      createdAtMs: 120,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: charter.revision,
      autonomyProfile: charter.autonomyProfile,
      createdAtMs: project.createdAtMs,
    } as const;
    const bound = {
      workspaceId: project.workspaceId,
      projectId: project.id,
      revision: 1,
      charterRevision: charter.revision,
      sourceKind: "local_path",
      source: "/home/operator/projects/sample-tracker",
      baseRevision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      declaredChecks: [
        {
          name: "pytest",
          argv: ["pytest", "tests/test_invariants.py", "-q"],
          workingDirectory: ".",
        },
      ],
      authorKind: "operator",
      authoredAtMs: 124,
      appliesToCurrentCharter: true,
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history")
        return Promise.resolve([{ ...charter, revision: 1, authoredAtMs: 122 }, charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "bind_project_repository") return Promise.resolve(bound);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));

    expect(await screen.findByText("No repository has been bound to this Project.")).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Bind Repository" }));
    expect(await screen.findByRole("form", { name: "Bind Repository" })).toBeVisible();
    expect(screen.getByRole("radio", { name: "Local path" })).toBeChecked();
    await user.type(screen.getByLabelText("Repository source"), bound.source);
    await user.type(screen.getByLabelText("Pinned base revision"), bound.baseRevision);
    await user.type(screen.getByLabelText("Check 1 name"), "pytest");
    await user.type(
      screen.getByLabelText("Check 1 arguments"),
      "pytest{enter}tests/test_invariants.py{enter}-q",
    );
    expect(
      screen.getAllByText("Declared, not verified — Beacon has not opened this repository.").length,
    ).toBeGreaterThan(0);
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Bind Repository revision 1" }));

    expect(invokeMock).toHaveBeenCalledWith("bind_project_repository", {
      projectId: project.id,
      expectedCharterRevision: charter.revision,
      expectedPriorRepositoryRevision: null,
      draft: {
        sourceKind: "local_path",
        source: bound.source,
        baseRevision: bound.baseRevision,
        declaredChecks: [
          {
            name: "pytest",
            argv: ["pytest", "tests/test_invariants.py", "-q"],
            workingDirectory: ".",
          },
        ],
      },
    });
    expect(await screen.findByText(/^Repository revision 1/)).toBeVisible();
    expect(screen.getByText(bound.baseRevision)).toBeVisible();
    expect(
      screen.getByText("Declared, not verified — Beacon has not opened this repository."),
    ).toBeVisible();
  });

  it("warns that a bound repository no longer applies to the current Charter", async () => {
    const user = userEvent.setup();
    const charter = {
      revision: 2,
      purpose: "Build a trustworthy local software factory.",
      desiredOutcomes: ["Ship an attributable Mission"],
      constraints: ["Keep Workspace data local"],
      prohibitions: ["Do not deploy publicly"],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 123,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter,
      createdAtMs: 120,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: charter.revision,
      autonomyProfile: charter.autonomyProfile,
      createdAtMs: project.createdAtMs,
    } as const;
    const stale = {
      workspaceId: project.workspaceId,
      projectId: project.id,
      revision: 1,
      charterRevision: 1,
      sourceKind: "local_path",
      source: "/home/operator/projects/sample-tracker",
      baseRevision: "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
      declaredChecks: [
        {
          name: "pytest",
          argv: ["pytest", "tests/test_invariants.py", "-q"],
          workingDirectory: ".",
        },
      ],
      authorKind: "operator",
      authoredAtMs: 124,
      appliesToCurrentCharter: false,
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history")
        return Promise.resolve([{ ...charter, revision: 1, authoredAtMs: 122 }, charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(stale);
      if (command === "project_repository_history") return Promise.resolve([stale]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));

    expect(await screen.findByText(/^Repository revision 1/)).toBeVisible();
    expect(
      screen.getByText(
        "The Charter is now revision 2. Bind a new Repository revision before planning work against it.",
      ),
    ).toBeVisible();
    expect(
      screen.getByText("Declared, not verified — Beacon has not opened this repository."),
    ).toBeVisible();
    expect(screen.getByRole("button", { name: "Revise Repository Binding" })).toBeVisible();
  });

  it("forms a Project Team from exact eligible Resident configurations without implying authority", async () => {
    const user = userEvent.setup();
    const resident = {
      ...inactiveResident,
      displayName: "Northstar",
      lifecycle: "active",
      latestDecisionId: "66666666-6666-4666-8666-666666666666",
      updatedAtMs: 110,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter: {
        revision: 1,
        purpose: "Build a trustworthy local software product.",
        desiredOutcomes: ["Form an attributable Team"],
        constraints: [],
        prohibitions: [],
        autonomyProfile: "supervised",
        budgetBoundary: "no_spend",
        externalEffects: "prohibited",
        authoredAtMs: 123,
      },
      createdAtMs: 123,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: 1,
      autonomyProfile: "supervised",
      createdAtMs: 123,
    } as const;
    const configuration = {
      id: "77777777-7777-4777-8777-777777777777",
      workspaceId: acceptedOrganization.workspaceId,
      residentId: resident.id,
      state: "configured",
      binding: {
        activationId: activeLocalModel.id,
        capabilities: ["work.create_mission"],
        budgetBoundary: "no_spend",
      },
      predecessorId: null,
      decidedAtMs: 124,
      eligibleForTeam: true,
      ineligibilityReasons: [],
    } as const;
    const formed = {
      id: "55555555-5555-4555-8555-555555555555",
      workspaceId: acceptedOrganization.workspaceId,
      projectId: project.id,
      name: "Alpha Team",
      formedBy: "operator",
      formedAtMs: 125,
      members: [
        {
          residentId: resident.id,
          formationConfigurationId: configuration.id,
          currentConfigurationId: configuration.id,
          eligibleNow: true,
          ineligibilityReasons: [],
        },
      ],
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve([resident]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([project.charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      if (command === "resident_configurations") return Promise.resolve([configuration]);
      if (command === "form_team") return Promise.resolve(formed);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));
    expect(await screen.findByText("No Teams have been formed for this Project.")).toBeVisible();
    expect(screen.getByText(/no permanent lead/i)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Form Team" }));

    expect(await screen.findByRole("form", { name: "Form Team" })).toBeVisible();
    expect(screen.getByText(/starts no Mission, Run, spending, model invocation/i)).toBeVisible();
    await user.type(screen.getByLabelText("Team name"), "Alpha Team");
    await user.click(screen.getByRole("checkbox", { name: "Northstar — lead" }));
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Form Team" }));

    expect(invokeMock).toHaveBeenCalledWith("form_team", {
      draft: {
        projectId: project.id,
        name: "Alpha Team",
        members: [
          {
            residentId: resident.id,
            expectedConfigurationId: configuration.id,
          },
        ],
      },
    });
    expect(await screen.findByText("Alpha Team")).toBeVisible();
    expect(screen.getByText(/Operator formed · 1 member/i)).toBeVisible();
  });

  it("admits a finite Mission from the exact current Team without inventing a lead", async () => {
    const user = userEvent.setup();
    const resident = {
      ...inactiveResident,
      displayName: "Northstar",
      lifecycle: "active",
      latestDecisionId: "66666666-6666-4666-8666-666666666666",
      updatedAtMs: 110,
    } as const;
    const previousCharter = {
      revision: 1,
      purpose: "Build a trustworthy local software product.",
      desiredOutcomes: ["Form an attributable Team"],
      constraints: [],
      prohibitions: [],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 122,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter: {
        revision: 2,
        purpose: "Build a trustworthy local software product.",
        desiredOutcomes: ["Ship an attributable Mission"],
        constraints: [],
        prohibitions: [],
        autonomyProfile: "supervised",
        budgetBoundary: "no_spend",
        externalEffects: "prohibited",
        authoredAtMs: 123,
      },
      createdAtMs: 120,
    } as const;
    const summary = {
      id: project.id,
      name: project.name,
      charterRevision: 2,
      autonomyProfile: "supervised",
      createdAtMs: 120,
    } as const;
    const configurationId = "77777777-7777-4777-8777-777777777777";
    const team = {
      id: "55555555-5555-4555-8555-555555555555",
      workspaceId: acceptedOrganization.workspaceId,
      projectId: project.id,
      name: "Alpha Team",
      formedBy: "operator",
      formedAtMs: 125,
      members: [
        {
          residentId: resident.id,
          formationConfigurationId: configurationId,
          currentConfigurationId: configurationId,
          eligibleNow: true,
          ineligibilityReasons: [],
        },
      ],
    } as const;
    const admitted = {
      id: "88888888-8888-4888-8888-888888888888",
      workspaceId: acceptedOrganization.workspaceId,
      projectId: project.id,
      charterRevision: project.charter.revision,
      teamId: team.id,
      desiredOutcome: "Ship one reviewed Beacon feature",
      successConditions: ["Relevant gates pass"],
      constraints: ["Keep the change reversible"],
      prohibitions: ["Do not deploy"],
      lifecycle: "admitted",
      admittedBy: "operator",
      admittedAtMs: 126,
      members: [
        {
          residentId: resident.id,
          admissionConfigurationId: configurationId,
        },
      ],
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve([resident]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") {
        return Promise.resolve([previousCharter, project.charter]);
      }
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([team]);
      if (command === "missions") return Promise.resolve([]);
      if (command === "admit_mission") return Promise.resolve(admitted);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));
    expect(await screen.findByText("No Missions have been admitted for this Project.")).toBeVisible();
    expect(screen.getByText(/Team Lead selection waits for Policy/i)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Admit Mission" }));

    expect(await screen.findByRole("form", { name: "Admit Mission" })).toBeVisible();
    await user.selectOptions(screen.getByLabelText("Mission Team"), team.id);
    await user.type(screen.getByLabelText("Desired outcome"), admitted.desiredOutcome);
    await user.type(screen.getByLabelText("Success conditions"), admitted.successConditions[0]);
    await user.type(screen.getByLabelText("Mission constraints"), admitted.constraints[0]);
    await user.type(screen.getByLabelText("Mission prohibitions"), admitted.prohibitions[0]);
    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Admit Mission" }));

    expect(invokeMock).toHaveBeenCalledWith("admit_mission", {
      draft: {
        projectId: project.id,
        expectedCharterRevision: project.charter.revision,
        teamId: team.id,
        desiredOutcome: admitted.desiredOutcome,
        successConditions: [...admitted.successConditions],
        constraints: [...admitted.constraints],
        prohibitions: [...admitted.prohibitions],
        members: [
          {
            residentId: resident.id,
            expectedConfigurationId: configurationId,
          },
        ],
      },
    });
    expect(await screen.findByText(admitted.desiredOutcome)).toBeVisible();
    expect(screen.getByText(/Operator admitted · Charter revision 2/i)).toBeVisible();
  });

  it("edits and explicitly admits the recommended Starter Organization", async () => {
    const user = userEvent.setup();
    const accepted = {
      ...proposedOrganization,
      decision: "accepted",
      residents: [
        {
          id: "22222222-2222-4222-8222-222222222222",
          role: "lead",
          displayName: "Northstar",
          lifecycle: "inactive",
        },
        {
          id: "33333333-3333-4333-8333-333333333333",
          role: "builder",
          displayName: null,
          lifecycle: "inactive",
        },
      ],
    } as const;
    const admittedResidents = accepted.residents.map((resident, index) => ({
      ...resident,
      workspaceId: accepted.workspaceId,
      latestDecisionId: null,
      createdAtMs: 100 + index,
      updatedAtMs: 100 + index,
    }));
    let admitted = false;
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command, args) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(proposedOrganization);
      if (command === "residents") return Promise.resolve(admitted ? admittedResidents : []);
      if (command === "accept_starter_organization") {
        admitted = true;
        return Promise.resolve(accepted);
      }
      return Promise.reject(new Error(`unexpected command: ${command} ${JSON.stringify(args)}`));
    });

    render(<App />);

    expect(
      await screen.findByRole("heading", { name: "Build your starter organization" }),
    ).toBeVisible();
    expect(screen.getByRole("checkbox", { name: "Include Lead" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Include Builder" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Include Reviewer" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Include Researcher" })).not.toBeChecked();
    await user.type(screen.getByLabelText("Lead Display Name"), "Northstar");
    await user.click(screen.getByRole("checkbox", { name: "Include Reviewer" }));
    await user.click(screen.getByRole("button", { name: "Admit selected Residents" }));

    expect(invokeMock).toHaveBeenCalledWith("accept_starter_organization", {
      proposalVersion: 1,
      residents: [
        { role: "lead", displayName: "Northstar" },
        { role: "builder", displayName: null },
      ],
    });
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    expect(screen.getByText("2 Residents in this Workspace")).toBeVisible();
    expect(screen.getByText(/No Resident has execution authority yet/)).toBeVisible();
  });

  it("controls Resident availability and confirms terminal retirement", async () => {
    const user = userEvent.setup();
    const active = {
      ...inactiveResident,
      lifecycle: "active",
      latestDecisionId: "33333333-3333-4333-8333-333333333333",
      updatedAtMs: 110,
    } as const;
    const suspended = {
      ...active,
      lifecycle: "suspended",
      latestDecisionId: "44444444-4444-4444-8444-444444444444",
      updatedAtMs: 120,
    } as const;
    const reactivated = {
      ...active,
      latestDecisionId: "55555555-5555-4555-8555-555555555555",
      updatedAtMs: 130,
    } as const;
    const retired = {
      ...reactivated,
      lifecycle: "retired",
      latestDecisionId: "66666666-6666-4666-8666-666666666666",
      updatedAtMs: 140,
    } as const;
    let current:
      | typeof inactiveResident
      | typeof active
      | typeof suspended
      | typeof reactivated
      | typeof retired = inactiveResident;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve([current]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "activate_resident") {
        current = active;
        return Promise.resolve(active);
      }
      if (command === "suspend_resident") {
        current = suspended;
        return Promise.resolve(suspended);
      }
      if (command === "reactivate_resident") {
        current = reactivated;
        return Promise.resolve(reactivated);
      }
      if (command === "retire_resident") {
        current = retired;
        return Promise.resolve(retired);
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    const roster = screen.getByRole("list", { name: "Resident roster" });
    await user.click(within(roster).getByRole("button", { name: "Activate" }));
    expect(invokeMock).toHaveBeenCalledWith("activate_resident", {
      residentId: inactiveResident.id,
      expectedDecisionId: null,
    });
    expect(await within(roster).findByText("active")).toBeVisible();

    await user.click(within(roster).getByRole("button", { name: "Suspend" }));
    expect(invokeMock).toHaveBeenCalledWith("suspend_resident", {
      residentId: active.id,
      expectedDecisionId: active.latestDecisionId,
    });
    await user.click(within(roster).getByRole("button", { name: "Reactivate" }));
    expect(invokeMock).toHaveBeenCalledWith("reactivate_resident", {
      residentId: suspended.id,
      expectedDecisionId: suspended.latestDecisionId,
    });

    await user.click(within(roster).getByRole("button", { name: "Retire" }));
    const confirmation = screen.getByRole("alertdialog", {
      name: "Permanently retire this Resident?",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("retire_resident", expect.anything());
    await user.click(within(confirmation).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();

    await user.click(within(roster).getByRole("button", { name: "Retire" }));
    await user.click(
      within(screen.getByRole("alertdialog")).getByRole("button", {
        name: "Permanently retire",
      }),
    );
    expect(invokeMock).toHaveBeenCalledWith("retire_resident", {
      residentId: reactivated.id,
      expectedDecisionId: reactivated.latestDecisionId,
    });
    expect(await within(roster).findByText("retired")).toBeVisible();
    expect(within(roster).queryByRole("button")).not.toBeInTheDocument();
    expect(screen.getByText(/Lifecycle controls availability only/)).toBeVisible();
  });

  it("configures and clears a Resident without granting execution authority", async () => {
    const user = userEvent.setup();
    const activeResident = {
      ...inactiveResident,
      lifecycle: "active",
      latestDecisionId: "33333333-3333-4333-8333-333333333333",
      updatedAtMs: 110,
    } as const;
    const configured = {
      id: "77777777-7777-4777-8777-777777777777",
      workspaceId: activeResident.workspaceId,
      residentId: activeResident.id,
      state: "configured",
      binding: {
        activationId: activeLocalModel.id,
        capabilities: [
          "work.create_mission",
          "work.form_team",
          "work.coordinate_mission",
        ],
        budgetBoundary: "no_spend",
      },
      predecessorId: null,
      decidedAtMs: 200,
      eligibleForTeam: true,
      ineligibilityReasons: [],
    } as const;
    const cleared = {
      ...configured,
      id: "88888888-8888-4888-8888-888888888888",
      state: "cleared",
      binding: null,
      predecessorId: configured.id,
      decidedAtMs: 210,
      eligibleForTeam: false,
      ineligibilityReasons: ["configuration_cleared"],
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve([activeResident]);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "resident_capability_catalog") {
        return Promise.resolve(residentCapabilityCatalog);
      }
      if (command === "resident_configurations") return Promise.resolve([]);
      if (command === "local_model_activations") return Promise.resolve([activeLocalModel]);
      if (command === "configure_resident") return Promise.resolve(configured);
      if (command === "clear_resident_configuration") return Promise.resolve(cleared);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Review Resident configurations" }));
    await user.click(await screen.findByRole("button", { name: "Configure Lead" }));
    expect(screen.getByRole("combobox", { name: "Local model for Lead" })).toHaveValue(
      activeLocalModel.id,
    );
    await user.click(screen.getByRole("checkbox", { name: "Assign Work" }));
    await user.click(screen.getByRole("button", { name: "Save Resident configuration" }));

    expect(invokeMock).toHaveBeenCalledWith("configure_resident", {
      residentId: activeResident.id,
      expectedDecisionId: null,
      activationId: activeLocalModel.id,
      capabilities: [
        "work.create_mission",
        "work.form_team",
        "work.coordinate_mission",
      ],
    });
    expect(await screen.findByText("Eligible for Team formation")).toBeVisible();
    expect(screen.getByText(/does not grant execution authority/i)).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Clear configuration for Lead" }));
    const confirmation = screen.getByRole("alertdialog", {
      name: "Clear this Resident configuration?",
    });
    await user.click(
      within(confirmation).getByRole("button", { name: "Clear Resident configuration" }),
    );
    expect(invokeMock).toHaveBeenCalledWith("clear_resident_configuration", {
      residentId: activeResident.id,
      expectedDecisionId: configured.id,
    });
    expect(await screen.findByText("Cleared; not eligible for Team formation")).toBeVisible();

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("requires confirmation before discarding the one-time Starter Organization", async () => {
    const user = userEvent.setup();
    const discarded = { ...proposedOrganization, decision: "discarded" } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(proposedOrganization);
      if (command === "residents") return Promise.resolve([]);
      if (command === "discard_starter_organization") return Promise.resolve(discarded);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Build your starter organization" });
    await user.click(screen.getByRole("button", { name: "Start without Residents" }));

    expect(invokeMock).not.toHaveBeenCalledWith("discard_starter_organization", expect.anything());
    expect(screen.getByRole("status")).toHaveTextContent("closes the one-time proposal");
    fireEvent.submit(screen.getByRole("form", { name: "Starter Organization proposal" }));
    expect(invokeMock).not.toHaveBeenCalledWith("accept_starter_organization", expect.anything());
    await user.click(screen.getByRole("button", { name: "Keep proposal" }));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start without Residents" }));
    await user.click(screen.getByRole("button", { name: "Confirm start without Residents" }));
    expect(invokeMock).toHaveBeenCalledWith("discard_starter_organization", {
      proposalVersion: 1,
    });
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    expect(screen.getByText("Starter organization skipped")).toBeVisible();
  });

  it("signs into an existing local account and can lock it again", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("locked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "authenticate_password") return Promise.resolve(status("unlocked"));
      if (command === "lock_session") return Promise.resolve(status("locked"));
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome back" });
    await user.type(screen.getByLabelText("Password"), "a long local passphrase");
    await user.click(screen.getByRole("button", { name: "Unlock Beacon" }));
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Lock Beacon" }));
    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeVisible();
  });

  it("closes Workspace access when the supervisor reports session expiry", async () => {
    vi.useFakeTimers();
    let checks = 0;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") {
        checks += 1;
        return Promise.resolve(status(checks === 1 ? "unlocked" : "locked"));
      }
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("heading", { name: "Briefing" })).toBeVisible();

    await act(async () => {
      vi.advanceTimersByTime(1_000);
    });

    expect(screen.getByRole("heading", { name: "Welcome back" })).toBeVisible();
    expect(screen.getByText("Workspace access locked")).toBeVisible();
  });

  it("conceals Workspace material on focus loss until the supervisor is reverified", async () => {
    let sessionChecks = 0;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") {
        sessionChecks += 1;
        return Promise.resolve(status(sessionChecks === 1 ? "unlocked" : "locked"));
      }
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();

    fireEvent.blur(window);
    expect(screen.getByRole("heading", { name: "Beacon is concealed" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Briefing" })).not.toBeInTheDocument();
    fireEvent.focus(window);

    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Briefing" })).not.toBeInTheDocument();
  });

  it("stays concealed when focus is lost during an in-flight session verification", async () => {
    let resolveForegroundCheck: ((value: ReturnType<typeof status>) => void) | undefined;
    let sessionChecks = 0;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") {
        sessionChecks += 1;
        if (sessionChecks === 1) return Promise.resolve(status("unlocked"));
        return new Promise((resolve) => {
          resolveForegroundCheck = resolve;
        });
      }
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();

    fireEvent.focus(window);
    expect(screen.getByRole("heading", { name: "Beacon is concealed" })).toBeVisible();
    fireEvent.blur(window);
    await act(async () => {
      resolveForegroundCheck?.(status("unlocked"));
      await Promise.resolve();
    });

    expect(screen.getByRole("heading", { name: "Beacon is concealed" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Briefing" })).not.toBeInTheDocument();
  });

  it("explains a session check failure and can retry without reloading", async () => {
    let sessionChecks = 0;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") {
        sessionChecks += 1;
        return sessionChecks === 1
          ? Promise.reject(new Error("supervisor unavailable"))
          : Promise.resolve(status("operator_setup_required"));
      }
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const user = userEvent.setup();

    render(<App />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Beacon could not verify the local session.",
    );
    await user.click(screen.getByRole("button", { name: "Check again" }));
    expect(
      await screen.findByRole("heading", { name: "Set up your operator account" }),
    ).toBeVisible();
  });

  it("persists the session timeout and renews only on explicit operator activity", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");

    render(<App />);

    const timeout = await screen.findByRole("spinbutton", {
      name: "Lock after inactivity in minutes",
    });
    expect(timeout).toHaveValue(30);
    expect(await screen.findByText("OS lock monitoring active")).toBeVisible();
    expect(invokeMock).not.toHaveBeenCalledWith("record_operator_activity");

    fireEvent.keyDown(document, { key: "Tab" });
    expect(invokeMock).toHaveBeenCalledWith("record_operator_activity");
    await user.clear(timeout);
    await user.type(timeout, "60");
    await user.click(screen.getByRole("button", { name: "Save timeout" }));

    expect(invokeMock).toHaveBeenCalledWith("set_session_timeout", {
      inactivityTimeoutMinutes: 60,
    });
  });

  it("warns without claiming protection when an OS lock monitor is unavailable", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") {
        return Promise.resolve({ screenLock: "unavailable", suspend: "active" });
      }
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);

    expect(
      await screen.findByText(/Automatic OS lock monitoring is unavailable/i),
    ).toBeVisible();
    expect(screen.queryByText("OS lock monitoring active")).not.toBeInTheDocument();
  });

  it("fails closed when the command returns an undeclared field", async () => {
    invokeMock.mockResolvedValue({
      phase: "operator_setup_required",
      workspaceAccess: false,
      operatorCommands: false,
      workspaceName: "Personal",
    });

    render(<App />);

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Beacon could not verify the local session.",
    );
    expect(screen.queryByText("Personal")).not.toBeInTheDocument();
  });

  it("starts in light mode and lets the Operator choose dark mode", async () => {
    const user = userEvent.setup();
    mockInitialState("operator_setup_required");

    render(<App />);
    await screen.findByRole("heading", { name: "Set up your operator account" });

    expect(screen.getByRole("button", { name: "Use light theme" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await user.click(screen.getByRole("button", { name: "Use dark theme" }));
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("tracks operating-system theme changes while following system", async () => {
    const user = userEvent.setup();
    const listeners = new Set<(event: MediaQueryListEvent) => void>();
    let dark = false;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      writable: true,
      value: vi.fn(() => ({
        get matches() {
          return dark;
        },
        media: "(prefers-color-scheme: dark)",
        addEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) =>
          listeners.add(listener),
        removeEventListener: (_type: string, listener: (event: MediaQueryListEvent) => void) =>
          listeners.delete(listener),
      })),
    });
    mockInitialState("operator_setup_required");

    render(<App />);
    await screen.findByRole("heading", { name: "Set up your operator account" });
    await user.click(screen.getByRole("button", { name: "Follow system theme" }));
    expect(document.documentElement).toHaveAttribute("data-theme", "light");

    dark = true;
    act(() => {
      for (const listener of listeners) listener({ matches: true } as MediaQueryListEvent);
    });
    expect(document.documentElement).toHaveAttribute("data-theme", "dark");
  });

  it("has no automated accessibility violations in the setup view", async () => {
    mockInitialState("operator_setup_required");
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Set up your operator account" });

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("has no automated accessibility violations in the Starter Organization view", async () => {
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(proposedOrganization);
      if (command === "residents") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Build your starter organization" });

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("has no automated accessibility violations in the Project Charter composer", async () => {
    const user = userEvent.setup();
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "projects") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await screen.findByText("No Projects yet");
    await user.click(screen.getByRole("button", { name: "Create Project" }));

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });

  it("has no automated accessibility violations in the Charter revision composer", async () => {
    const user = userEvent.setup();
    const charter = {
      revision: 1,
      purpose: "Build a trustworthy local software product.",
      desiredOutcomes: ["Ship a verified vertical slice"],
      constraints: [],
      prohibitions: [],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 123,
    } as const;
    const project = {
      id: "33333333-3333-4333-8333-333333333333",
      workspaceId: proposedOrganization.workspaceId,
      name: "Beacon Alpha",
      charter,
      createdAtMs: 123,
    } as const;
    invokeMock.mockImplementation((command) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(acceptedOrganization);
      if (command === "residents") return Promise.resolve(residentRoster);
      if (command === "projects") {
        return Promise.resolve([
          {
            id: project.id,
            name: project.name,
            charterRevision: 1,
            autonomyProfile: "supervised",
            createdAtMs: 123,
          },
        ]);
      }
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Beacon Alpha/i }));
    await user.click(await screen.findByRole("button", { name: "Revise Charter" }));

    const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
    expect(results.violations).toEqual([]);
  });
});
