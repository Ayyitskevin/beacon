import { invoke } from "@tauri-apps/api/core";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { expectNoWcagViolations } from "./test/axe";

import { App } from "./App";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const originalMatchMedia = window.matchMedia;
const originalClipboard = Object.getOwnPropertyDescriptor(window.navigator, "clipboard");
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

/** One row of a recent-Receipt page, in the daemon's wire shape. */
function recentRow(
  runId: string,
  startedAtMs: number,
  outcome:
    | { kind: "check_preflight"; outcome: string }
    | { kind: "declared_checks"; summary: string },
  projectName = "Task tracker",
) {
  return {
    runId,
    projectId: briefingProjects[0].id,
    projectName,
    version: outcome.kind === "check_preflight" ? 1 : 2,
    outcome,
    startedAtMs,
    endedAtMs: startedAtMs + 1,
    recordedAtMs: startedAtMs + 1,
  };
}

/** A page as the daemon answers it: newest first, both kinds, counts scoped to
 *  the Run window. */
function recentPage(overrides: Record<string, unknown> = {}) {
  return {
    receipts: [
      recentRow("cccccccc-3333-4333-8333-333333333333", 300, {
        kind: "declared_checks",
        summary: "completed",
      }),
      recentRow(
        "bbbbbbbb-2222-4222-8222-222222222222",
        200,
        { kind: "check_preflight", outcome: "preflight_timed_out" },
        "Ledger",
      ),
      recentRow("aaaaaaaa-1111-4111-8111-111111111111", 100, {
        kind: "check_preflight",
        outcome: "verified",
      }),
    ],
    runsInFlightInWindow: 0,
    runsUnconfirmedInWindow: 0,
    runWindowApplied: 50,
    moreRunsExist: false,
    ...overrides,
  };
}

const encryptionEvidence = [
  { source: "stat(live store)", observation: "st_dev = 259:2", truncated: false },
  {
    source: "/proc/self/mountinfo",
    observation: "259:2 ext4 /dev/nvme0n1p2 at /home/operator",
    truncated: false,
  },
] as const;

/** The wire shape is a closed conclusion now: no free text to compose here. */
function encryptionReport(
  state: "enabled" | "disabled" | "unknown",
  reason?: string,
  subjects?: { role: string; value: string; shortened: boolean }[],
) {
  const chosen =
    reason ??
    (state === "enabled"
      ? "crypt_covers_every_path"
      : state === "disabled"
        ? "no_crypt_on_any_path"
        : "mount_table_unreadable");
  const defaults =
    chosen === "crypt_covers_every_path"
      ? [{ role: "mechanism", value: "dm-crypt (LUKS2)", shortened: false }]
      : chosen === "no_crypt_on_any_path"
        ? [{ role: "device", value: "259:2", shortened: false }]
        : [];
  return {
    conclusion: {
      state,
      reason: chosen,
      subjects: subjects ?? defaults,
      omittedSubjects: 0,
    },
    mountPoint: { role: "location", value: "/home/operator", shortened: false },
    evidence: encryptionEvidence,
    omittedEvidence: 0,
    notVerified: ["filesystem-level encryption such as fscrypt or ext4 encryption"],
    observedAtMs: 1_756_000_000_000,
  };
}

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
    if (command === "recent_receipts") return Promise.resolve(recentPage({ receipts: [] }));
    if (command === "live_data_encryption_status") {
      return Promise.resolve(encryptionReport("enabled"));
    }
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
    if (originalClipboard) {
      Object.defineProperty(window.navigator, "clipboard", originalClipboard);
    } else {
      Reflect.deleteProperty(window.navigator, "clipboard");
    }
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
    await expectNoWcagViolations(document.body);
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
      await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
  });

  it.each([
    ["operator_setup_required", "Finish setup to open the saved Beacon link."],
    ["locked", "Unlock Beacon to open the saved Beacon link."],
  ] as const)("retains a pasted link while the session is %s", async (phase, message) => {
    const user = userEvent.setup();
    mockInitialState(phase);

    render(<App />);
    await screen.findByRole("heading", {
      name: phase === "locked" ? "Welcome back" : "Set up your operator account",
    });
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/44444444-4444-4444-8444-444444444444`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));

    expect(screen.getByRole("status")).toHaveTextContent(message);
    await expectNoWcagViolations(document.body);
  });

  it("opens a retained link after the operator unlocks", async () => {
    const user = userEvent.setup();
    mockInitialState("locked");
    const base = invokeMock.getMockImplementation();
    if (!base) throw new Error("initial invoke mock is unavailable");
    invokeMock.mockImplementation((command, args) => {
      if (command === "authenticate_password") return Promise.resolve(status("unlocked"));
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome back" });
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/briefing`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));
    expect(screen.getByText("Unlock Beacon to open the saved Beacon link.")).toBeVisible();

    await user.type(screen.getByLabelText("Password"), "a long local passphrase");
    await user.click(screen.getByRole("button", { name: "Unlock Beacon" }));

    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    expect(await screen.findByText("Opened Briefing from the Beacon link.")).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("shows a saved Project link as not evaluated while Projects load, then opens it", async () => {
    const user = userEvent.setup();
    const project = {
      id: "44444444-4444-4444-8444-444444444444",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Linked Project",
      charter: {
        revision: 1,
        purpose: "Open only after membership is evaluated.",
        desiredOutcomes: ["Reach the named Project"],
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
      charterRevision: 1,
      autonomyProfile: "supervised",
      createdAtMs: 120,
    } as const;
    let resolveProjects!: (projects: (typeof summary)[]) => void;
    const loadingProjects = new Promise<(typeof summary)[]>((resolve) => {
      resolveProjects = resolve;
    });
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation();
    if (!base) throw new Error("initial invoke mock is unavailable");
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return loadingProjects;
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([project.charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${project.id}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));

    expect(screen.getByText(/Beacon has not evaluated projects yet/)).toBeVisible();
    expect(screen.queryByText(/No Project with that identity/i)).not.toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("projects");

    await act(async () => resolveProjects([summary]));
    expect(await screen.findByRole("heading", { name: "Linked Project" })).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("visibly refuses a valid link from another Workspace", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.type(
      screen.getByLabelText("Beacon link"),
      "beacon:/99999999-9999-4999-8999-999999999999/project/44444444-4444-4444-8444-444444444444",
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));

    expect(
      screen.getByText("Refused: this Beacon link belongs to another Workspace."),
    ).toBeVisible();
    expect(screen.getByRole("heading", { name: "Briefing" })).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("keeps an absent Project distinct from an unevaluated Project", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/44444444-4444-4444-8444-444444444444`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));

    expect(
      await screen.findByText("No Project with that identity exists in this Workspace."),
    ).toBeVisible();
    expect(screen.queryByText(/has not evaluated projects yet/i)).not.toBeInTheDocument();
    await expectNoWcagViolations(document.body);
  });

  it("copies canonical links from the Project and Mission that are shown", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(window.navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const project = {
      id: "44444444-4444-4444-8444-444444444444",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Linked Project",
      charter: {
        revision: 1,
        purpose: "Mint links from the object being shown.",
        desiredOutcomes: ["One canonical link"],
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
      charterRevision: 1,
      autonomyProfile: "supervised",
      createdAtMs: 120,
    } as const;
    const mission = {
      id: "88888888-8888-4888-8888-888888888888",
      workspaceId: project.workspaceId,
      projectId: project.id,
      charterRevision: 1,
      teamId: "55555555-5555-4555-8555-555555555555",
      desiredOutcome: "Open the copied Mission",
      successConditions: ["The emitted link parses"],
      constraints: [],
      prohibitions: [],
      lifecycle: "admitted",
      admittedBy: "operator",
      admittedAtMs: 126,
      members: [
        {
          residentId: "66666666-6666-4666-8666-666666666666",
          admissionConfigurationId: "77777777-7777-4777-8777-777777777777",
        },
      ],
    } as const;
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation();
    if (!base) throw new Error("initial invoke mock is unavailable");
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([project.charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([mission]);
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Linked Project/i }));
    await screen.findByText("Open the copied Mission");

    await user.click(screen.getByRole("button", { name: "Copy Project link" }));
    const projectLink = `beacon:/${project.workspaceId}/project/${project.id}`;
    expect(writeText).toHaveBeenLastCalledWith(projectLink);
    expect(screen.getByLabelText("Beacon link")).toHaveValue(projectLink);

    await user.click(screen.getByRole("button", { name: "Copy Mission link" }));
    const missionLink = `${projectLink}/mission/${mission.id}`;
    expect(writeText).toHaveBeenLastCalledWith(missionLink);
    expect(screen.getByLabelText("Beacon link")).toHaveValue(missionLink);
    expect(screen.getByText("Copied Mission link.")).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("opens a pasted Mission link after loading its Project and Mission roster", async () => {
    const user = userEvent.setup();
    const project = {
      id: "44444444-4444-4444-8444-444444444444",
      workspaceId: acceptedOrganization.workspaceId,
      name: "Mission link Project",
      charter: {
        revision: 1,
        purpose: "Resolve a Mission from the authenticated shell.",
        desiredOutcomes: ["Reach the exact Mission"],
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
      charterRevision: 1,
      autonomyProfile: "supervised",
      createdAtMs: 120,
    } as const;
    const mission = {
      id: "88888888-8888-4888-8888-888888888888",
      workspaceId: project.workspaceId,
      projectId: project.id,
      charterRevision: 1,
      teamId: "55555555-5555-4555-8555-555555555555",
      desiredOutcome: "Reach this exact linked Mission",
      successConditions: ["The Mission is visibly selected"],
      constraints: [],
      prohibitions: [],
      lifecycle: "admitted",
      admittedBy: "operator",
      admittedAtMs: 126,
      members: [
        {
          residentId: "66666666-6666-4666-8666-666666666666",
          admissionConfigurationId: "77777777-7777-4777-8777-777777777777",
        },
      ],
    } as const;
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation();
    if (!base) throw new Error("initial invoke mock is unavailable");
    invokeMock.mockImplementation((command, args) => {
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") return Promise.resolve([project.charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams") return Promise.resolve([]);
      if (command === "missions") return Promise.resolve([mission]);
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${project.workspaceId}/project/${project.id}/mission/${mission.id}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));

    await screen.findByText("Opened the named Mission from the Beacon link.");
    const outcome = screen.getByText(mission.desiredOutcome);
    expect(outcome.closest("article")).toHaveAttribute("aria-current", "true");
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
  });

  it("shows Briefing's recent Receipts of every kind, in the order the daemon sent", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") return Promise.resolve(recentPage());
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    const list = await screen.findByRole("list", { name: "Recent Receipts" });
    const rows = within(list).getAllByRole("listitem");
    expect(rows.map((row) => row.textContent)).toEqual([
      expect.stringContaining("Run cccccccc"),
      expect.stringContaining("Run bbbbbbbb"),
      expect.stringContaining("Run aaaaaaaa"),
    ]);
    // Both kinds, each in its own vocabulary. The Project-scoped Query this
    // replaced could not carry an execution Receipt at all.
    expect(rows[0].textContent).toContain("Declared checks");
    expect(rows[1].textContent).toContain("Check preflight");
    expect(rows[1].textContent).toContain("Ledger");

    // One Query, and no fan-out: Briefing no longer opens Projects to read
    // Receipts.
    expect(invokeMock).toHaveBeenCalledWith("recent_receipts", { window: 50 });

    await expectNoWcagViolations(container);
  });

  it("warns persistently while live-data encryption is disabled and rechecks on demand", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    let state: "enabled" | "disabled" = "disabled";
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "live_data_encryption_status") {
        return Promise.resolve(encryptionReport(state));
      }
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    expect(
      await screen.findByText(/Live data is not fully protected by disk encryption\./),
    ).toBeVisible();
    // Unobtrusive, but not dismissible: the condition, not the operator, ends it.
    expect(screen.queryByRole("button", { name: /dismiss|ignore|close/i })).toBeNull();

    await user.click(screen.getByRole("button", { name: "How to turn this on" }));
    expect(screen.getByText(/Turning on disk encryption on Linux/)).toBeVisible();
    // The destructive path is named, never handed over as a command to run.
    expect(container.textContent).not.toMatch(/reencrypt|cryptsetup-reencrypt/);
    expect(screen.getByText(/can destroy all data on the volume/)).toBeVisible();

    state = "enabled";
    await user.click(screen.getByRole("button", { name: "Recheck" }));
    expect(
      await screen.findByText(/Operating-system disk encryption detected/),
    ).toBeVisible();
    expect(
      screen.queryByText(/Live data is not fully protected by disk encryption\./),
    ).toBeNull();
    expect(invokeMock).toHaveBeenCalledWith("live_data_encryption_status");

    await expectNoWcagViolations(container);
  });

  it("presents the ruled v1 reading as not evaluated rather than guessing", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "live_data_encryption_status") {
        return Promise.resolve(
          encryptionReport("unknown", "detection_not_evaluated", []),
        );
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    expect(await screen.findByText(/Disk-encryption status is unknown\./)).toBeVisible();
    expect(screen.getByText(/detection was not evaluated in this version/)).toBeVisible();
    expect(screen.getByRole("button", { name: "Recheck" })).toBeVisible();
    await user.click(screen.getByRole("button", { name: "How to turn this on" }));
    expect(
      screen.getByText(/records bounded Linux observations but does not evaluate LUKS coverage/),
    ).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("states the window, the receiptless Runs and the overflow the daemon reported", async () => {
    // Every number the panel shows is a number the page carries. The bound is
    // the daemon's; the interface reports it rather than choosing it.
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") {
        return Promise.resolve(
          recentPage({
            runWindowApplied: 50,
            runsUnconfirmedInWindow: 3,
            runsInFlightInWindow: 1,
            moreRunsExist: true,
          }),
        );
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    const reading = await screen.findByText(/this reading covered the 50 most recent Runs/);
    expect(reading.textContent).toMatch(/found 3 Receipts/);
    expect(reading.textContent).toMatch(/3 ended without one/);
    expect(reading.textContent).toMatch(/1 is still running/);
    expect(reading.textContent).toMatch(/older Runs exist/);
    // Runs, not Receipts: an older Run that recorded none sets the same flag.
    expect(reading.textContent).not.toMatch(/more Receipts/);
    await expectNoWcagViolations(document.body);
  });

  it("does not call a window with no Receipts an empty Workspace", async () => {
    // The honest-absence case: the daemon walked 50 Runs, four of them recorded
    // no Receipt, and older Runs exist. "No Receipts yet" would be an absolute
    // claim about the Workspace that this reading did not earn.
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") {
        return Promise.resolve(
          recentPage({ receipts: [], runsUnconfirmedInWindow: 4, moreRunsExist: true }),
        );
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    expect(
      await screen.findByText("No Receipts among the Runs in this reading."),
    ).toBeVisible();
    expect(screen.queryByText("No Receipts yet.")).toBeNull();
    const reading = screen.getByText(/this reading covered the 50 most recent Runs/);
    expect(reading.textContent).toMatch(/found 0 Receipts/);
    expect(reading.textContent).toMatch(/4 ended without one/);
    await expectNoWcagViolations(document.body);
  });

  it("says the Workspace has no Receipts only when the reading saw every Run", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") return Promise.resolve(recentPage({ receipts: [] }));
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    expect(await screen.findByText("No Receipts yet.")).toBeVisible();
    expect(screen.queryByText("No Receipts among the Runs in this reading.")).toBeNull();
    await expectNoWcagViolations(document.body);
  });

  it("never presents a Receipt as work that succeeded", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") return Promise.resolve(recentPage());
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    const list = await screen.findByRole("list", { name: "Recent Receipts" });
    // A Receipt records what a Run did. Preflight runs no check, and a
    // `completed` execution records that the checks RAN.
    expect(within(list).getByText(/Execution would be admissible — not a passing check/)).toBeVisible();
    expect(within(list).getByText(/every Declared Check ran — this does not say they passed/)).toBeVisible();
    // A word match cannot tell a claim from its denial, and both kinds deny
    // one. Every occurrence of an upgrade word in a row must be inside a
    // denial, so count them: one per row, and only in the two rows that deny.
    const rows = within(list).getAllByRole("listitem");
    for (const row of rows) {
      expect((row.textContent ?? "").match(/succeeded|success/gi) ?? []).toHaveLength(0);
      const passes = (row.textContent ?? "").match(/passing|passed/gi) ?? [];
      expect(passes.length).toBeLessThanOrEqual(1);
      if (passes.length === 1) {
        expect(row.textContent).toMatch(/not a passing check|does not say they passed/);
      }
    }
    await expectNoWcagViolations(document.body);
  });

  it("keeps a failed Receipt read apart from a Workspace with no Receipts", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    let attempts = 0;
    invokeMock.mockImplementation((command, args) => {
      if (command === "recent_receipts") {
        attempts += 1;
        if (attempts === 1) {
          return Promise.reject({ code: "preflight_unavailable", message: "unused" });
        }
        return Promise.resolve(recentPage({ receipts: [] }));
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    // The shared code says "could not record the preflight", which is false for
    // a Query that records nothing. Translated at this seam.
    expect(
      await screen.findByText(/Beacon could not read recent Run and Receipt state\./),
    ).toBeVisible();
    expect(screen.queryByText("No Receipts yet.")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Refresh Receipts" }));
    expect(await screen.findByText("No Receipts yet.")).toBeVisible();
    expect(screen.queryByText(/could not be read/)).toBeNull();
    await expectNoWcagViolations(document.body);
  });

  it("keeps an initial live-data diagnostic failure visible and retryable", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    let attempts = 0;
    invokeMock.mockImplementation((command, args) => {
      if (command === "live_data_encryption_status") {
        attempts += 1;
        // The first read fails. Swallowing it left no status, no warning, no
        // error and no Recheck -- the operator could not even retry.
        if (attempts === 1) {
          return Promise.reject(new Error("diagnostic transport failed"));
        }
        return Promise.resolve(encryptionReport("disabled"));
      }
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });

    await screen.findByRole("heading", { name: "Live data" });
    // The failure is a REPORT now: a closed reason, and the ORIGINAL text as
    // its bounded detail. Passing it through the generic account describer
    // replaced the only text that says what actually went wrong, and carrying
    // it as a free-text prop put an unbounded string beside a bounded one.
    expect(screen.getByText(/Disk-encryption status could not be read\./)).toBeVisible();
    expect(
      screen.getByText(/the disk-encryption reading did not complete: diagnostic tra/),
    ).toBeVisible();
    const recheck = screen.getByRole("button", { name: "Recheck" });

    await user.click(recheck);
    expect(
      await screen.findByText(/Live data is not fully protected by disk encryption\./),
    ).toBeVisible();

    await expectNoWcagViolations(container);
  });

  it("does not present a stale reading when a recheck fails", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    let attempts = 0;
    invokeMock.mockImplementation((command, args) => {
      if (command === "live_data_encryption_status") {
        attempts += 1;
        if (attempts === 1) return Promise.resolve(encryptionReport("disabled"));
        return Promise.reject(new Error("sysfs read denied"));
      }
      return base(command, args);
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    expect(
      await screen.findByText(/Live data is not fully protected by disk encryption\./),
    ).toBeVisible();

    await user.click(screen.getByRole("button", { name: "Recheck" }));

    // The previous reading described the host at ITS observed_at_ms and nothing
    // else. Leaving it on screen after a failed recheck presents a stale
    // reading as the current one; the failure replaces it and says why.
    expect(await screen.findByText(/Disk-encryption status could not be read\./)).toBeVisible();
    expect(
      screen.getByText(/the disk-encryption reading did not complete: sysfs read denied/),
    ).toBeVisible();
    expect(
      screen.queryByText(/Live data is not fully protected by disk encryption\./),
    ).toBeNull();
    // And it is still retryable.
    expect(screen.getByRole("button", { name: "Recheck" })).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("does not deny encryption the reading itself observed", async () => {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    invokeMock.mockImplementation((command, args) => {
      if (command === "live_data_encryption_status") {
        // A mixed stack: one member encrypted, one not. Telling the operator
        // Beacon "found no encryption" here is false, and an operator who can
        // see the crypt layer in the evidence learns to disbelieve the warning.
        return Promise.resolve(
          encryptionReport("disabled", "some_paths_uncovered", [
            { role: "mapping", value: "pv0", shortened: false },
            { role: "device", value: "8:17", shortened: false },
          ]),
        );
      }
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await screen.findByRole("heading", { name: "Live data" });

    expect(container.textContent).not.toContain("found no operating-system disk encryption");
    expect(screen.getByText(/pv0 is encrypted; 8:17 is not/)).toBeVisible();
    expect(screen.getByRole("button", { name: "How to turn this on" })).toBeVisible();
    await expectNoWcagViolations(document.body);
  });

  it("never describes the open live store as Beacon-encrypted", async () => {
    for (const state of ["enabled", "disabled", "unknown"] as const) {
      mockInitialState("unlocked");
      const base = invokeMock.getMockImplementation()!;
      invokeMock.mockImplementation((command, args) => {
        if (command === "live_data_encryption_status") {
          return Promise.resolve(encryptionReport(state));
        }
        return base(command, args);
      });

      const { container, unmount } = render(<App />);
      await screen.findByRole("heading", { name: "Briefing" });
      await screen.findByRole("heading", { name: "Live data" });
      if (state === "unknown") {
        expect(screen.getByText(/Disk-encryption status is unknown\./)).toBeVisible();
        // An unknown reading states why, rather than implying a negative.
        expect(screen.getByText(/the mount table could not be read/)).toBeVisible();
      }
      expect(container.textContent).not.toContain("Beacon-encrypted");
      expect(container.textContent).not.toMatch(/Beacon (encrypts|has encrypted) (your|the) live/);
      await expectNoWcagViolations(document.body);
      unmount();
    }
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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

    // A12 (grok): the Projects view that mounts CheckPreflightSection was mounted here and never measured.
    // The surface is pinned before it is measured: a bar that runs against
    // whatever the test happened to leave on screen reports a pass for a view
    // nobody chose.
    expect(screen.getByRole("heading", { name: "Check preflight" })).toBeVisible();
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(container);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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

    await expectNoWcagViolations(container);
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
      // This test is about the Starter Organization, not about a diagnostic
      // that could not run; a rejected read would now correctly add a second
      // status region and make the assertion below ambiguous.
      if (command === "live_data_encryption_status") {
        return Promise.resolve(encryptionReport("enabled"));
      }
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Build your starter organization" });
    await user.click(screen.getByRole("button", { name: "Start without Residents" }));

    expect(invokeMock).not.toHaveBeenCalledWith("discard_starter_organization", expect.anything());
    // By text, not by "the one status region on the page": the Installation
    // inspector legitimately carries its own now.
    expect(screen.getByText(/closes the one-time proposal/)).toBeVisible();
    fireEvent.submit(screen.getByRole("form", { name: "Starter Organization proposal" }));
    expect(invokeMock).not.toHaveBeenCalledWith("accept_starter_organization", expect.anything());
    await user.click(screen.getByRole("button", { name: "Keep proposal" }));
    expect(screen.queryByText(/closes the one-time proposal/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Start without Residents" }));
    await user.click(screen.getByRole("button", { name: "Confirm start without Residents" }));
    expect(invokeMock).toHaveBeenCalledWith("discard_starter_organization", {
      proposalVersion: 1,
    });
    expect(await screen.findByRole("heading", { name: "Briefing" })).toBeVisible();
    expect(screen.getByText("Starter organization skipped")).toBeVisible();
    await expectNoWcagViolations(document.body);
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

    // A12 (grok): the LOCK view was mounted here and never measured.
    // The surface is pinned before it is measured: a bar that runs against
    // whatever the test happened to leave on screen reports a pass for a view
    // nobody chose.
    expect(screen.getByRole("heading", { name: "Welcome back" })).toBeVisible();
    await expectNoWcagViolations(document.body);
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
    vi.useRealTimers();
    await expectNoWcagViolations(document.body);
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
    // A12 (grok): the CONCEALED view, measured WHILE concealed. Placed here rather than at the end
    // of the test on purpose -- by the last line the window has been refocused and the surface on
    // screen is the reverify prompt, so a bar there would measure a different view and report a
    // pass for one nobody checked.
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    // A12 (grok, round 2): the session-ERROR view, measured WHILE the error is on screen. It sat
    // below the click before, where "Check again" has already navigated to setup -- so it measured
    // the setup surface and the comment claimed otherwise. The same mistake as the concealed view,
    // in the one place I did not check for it.
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
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
    await expectNoWcagViolations(document.body);
  });

  it("has no automated accessibility violations in the setup view", async () => {
    mockInitialState("operator_setup_required");
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Set up your operator account" });

    await expectNoWcagViolations(container);
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

    await expectNoWcagViolations(container);
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

    await expectNoWcagViolations(container);
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

    await expectNoWcagViolations(container);
  });

  // -------------------------------------------------------------------------------------------
  // N11: open the Project behind a Receipt
  // -------------------------------------------------------------------------------------------

  const receiptProject = {
    id: briefingProjects[0].id,
    workspaceId: acceptedOrganization.workspaceId,
    name: "Task tracker",
    charter: {
      revision: 1,
      purpose: "Track the work.",
      desiredOutcomes: ["Reach the named Project"],
      constraints: [],
      prohibitions: [],
      autonomyProfile: "supervised",
      budgetBoundary: "no_spend",
      externalEffects: "prohibited",
      authoredAtMs: 123,
    },
    createdAtMs: 120,
  } as const;

  // Held, and held SEPARATELY from anything a callback produces: the ids and purposes below are
  // the fixture's own, not read back out of `briefingProjects` or derived from the assertion under
  // test. Root's P1 and opus2's Finding 2 were both about expectations that could not tell the
  // clicked row's id from a constant.
  const N11_A_ID = "aaaa1111-1111-4111-8111-aaaaaaaaaaaa";
  const N11_B_ID = "bbbb2222-2222-4222-8222-bbbbbbbbbbbb";
  const N11_A_PURPOSE = "n11-alpha-held-purpose";
  const N11_B_PURPOSE = "n11-bravo-held-purpose";

  function n11Project(id: string, name: string, purpose: string) {
    return {
      id,
      workspaceId: acceptedOrganization.workspaceId,
      name,
      charter: { ...receiptProject.charter, purpose },
      createdAtMs: 120,
    } as const;
  }

  /** Two Receipts naming DIFFERENT Projects, newest first. The one to click is the second. */
  function n11ReceiptPage() {
    return {
      receipts: [
        { ...recentRow("cccccccc-3333-4333-8333-333333333333", 300, {
            kind: "declared_checks", summary: "completed",
          }), projectId: N11_A_ID, projectName: "Alpha Project" },
        { ...recentRow("bbbbbbbb-2222-4222-8222-222222222222", 200, {
            kind: "check_preflight", outcome: "verified",
          }), projectId: N11_B_ID, projectName: "Bravo Project" },
      ],
      runsInFlightInWindow: 0,
      runsUnconfirmedInWindow: 0,
      runWindowApplied: 50,
      moreRunsExist: false,
    };
  }

  it("opens the exact Project a Receipt names, from the keyboard", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const alpha = n11Project(N11_A_ID, "Alpha Project", N11_A_PURPOSE);
    const bravo = n11Project(N11_B_ID, "Bravo Project", N11_B_PURPOSE);
    const asked: unknown[] = [];
    let receiptReads = 0;
    invokeMock.mockImplementation((command, args) => {
      const projectId = (args as { projectId?: string } | undefined)?.projectId;
      if (command === "recent_receipts") {
        receiptReads += 1;
        return Promise.resolve(n11ReceiptPage());
      }
      if (command === "project") {
        asked.push(args);
        return Promise.resolve(projectId === N11_B_ID ? bravo : alpha);
      }
      if (command === "project_charter_history") {
        return Promise.resolve([projectId === N11_B_ID ? bravo.charter : alpha.charter]);
      }
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    const list = await screen.findByRole("list", { name: "Recent Receipts" });
    await expectNoWcagViolations(container);
    const readsBefore = receiptReads;

    // The SECOND row, so a control that hard-coded the first would be caught here.
    const rows = within(list).getAllByRole("listitem");
    const open = within(rows[1]).getByRole("button", { name: "Open Bravo Project Project" });
    open.focus();
    await user.keyboard("{Enter}");

    // Settle on something true under BOTH the clean code and root's dropped-adoption mutant --
    // the read is issued either way -- so every assertion below is reached and can name itself
    // rather than dying inside a `findBy*` that throws before `expect` runs.
    await vi.waitFor(() => expect(asked).toHaveLength(1));

    // The id actually asked for is the clicked row's held id, and no other row's.
    expect(asked, "N11.OPEN.EXACT_PROJECT").toEqual([{ projectId: N11_B_ID }]);
    // And a Project record was really adopted: root's P1 mutant drops exactly this.
    await vi.waitFor(() =>
      expect(
        screen.queryByRole("heading", { name: "Bravo Project" }),
        "N11.OPEN.PROJECT_RECORD_VISIBLE",
      ).not.toBeNull(),
    );
    expect(
      screen.queryAllByText(N11_B_PURPOSE).length,
      "N11.OPEN.PROJECT_RECORD_VISIBLE",
    ).toBeGreaterThan(0);
    expect(screen.queryAllByText(N11_A_PURPOSE), "N11.OPEN.PROJECT_RECORD_VISIBLE").toHaveLength(0);
    expect(
      await screen.findByRole("button", { name: "Projects", current: "page" }),
      "N11.OPEN.EXACT_PROJECT",
    ).toBeVisible();
    // Measured through App, not through a component callback: opening read no Receipts again.
    expect(receiptReads, "N11.OPEN.NO_RECEIPT_RELOAD").toBe(readsBefore);
  });

  it("stays on Briefing and shows the error when the Project behind a Receipt cannot be read", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const alpha = n11Project(N11_A_ID, "Alpha Project", N11_A_PURPOSE);
    invokeMock.mockImplementation((command, args) => {
      const projectId = (args as { projectId?: string } | undefined)?.projectId;
      if (command === "recent_receipts") return Promise.resolve(n11ReceiptPage());
      if (command === "project") {
        // A reads; B refuses. The prior successful selection is what makes the refusal meaningful.
        return projectId === N11_B_ID
          ? Promise.reject(new Error("the Project read failed"))
          : Promise.resolve(alpha);
      }
      if (command === "project_charter_history") return Promise.resolve([alpha.charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    const firstList = await screen.findByRole("list", { name: "Recent Receipts" });
    const firstRows = within(firstList).getAllByRole("listitem");

    // 1. A is really selected first.
    await user.click(within(firstRows[0]).getByRole("button", { name: "Open Alpha Project Project" }));
    expect(
      await screen.findByRole("heading", { name: "Alpha Project" }),
      "N11.OPEN.FAILURE_STAYS_BRIEFING",
    ).toBeVisible();

    // 2. Back to Briefing through the rail, with A still the selected Project underneath.
    await user.click(screen.getByRole("button", { name: "Briefing" }));
    const list = await screen.findByRole("list", { name: "Recent Receipts" });
    const rows = within(list).getAllByRole("listitem");

    // 3. B refuses.
    await user.click(within(rows[1]).getByRole("button", { name: "Open Bravo Project Project" }));

    // Settle on the refusal being rendered -- true under BOTH the clean code and the
    // navigate-after-failed-read mutant -- so the named rail assertions below are always reached.
    // opus2's Finding 3 was that this test died inside an ambiguous query before `expect` ran.
    await vi.waitFor(() =>
      expect(
        screen.queryAllByText("Beacon could not complete that local request.").length,
      ).toBeGreaterThan(0),
    );
    expect(
      screen.queryByRole("button", { name: "Briefing", current: "page" }),
      "N11.OPEN.FAILURE_STAYS_BRIEFING",
    ).not.toBeNull();
    expect(
      screen.queryByRole("button", { name: "Projects", current: "page" }),
      "N11.OPEN.FAILURE_STAYS_BRIEFING",
    ).toBeNull();
    // No mislabelling: B's record never appears, and A's is not drawn on Briefing either.
    expect(
      screen.queryByRole("heading", { name: "Bravo Project" }),
      "N11.OPEN.NO_MISLABELLED_PROJECT",
    ).toBeNull();
    expect(
      screen.queryByRole("heading", { name: "Alpha Project" }),
      "N11.OPEN.NO_MISLABELLED_PROJECT",
    ).toBeNull();
    // The refusal is visible, matched exactly and scoped, not by a substring that matches twice.
    expect(
      screen.getAllByText("Beacon could not complete that local request.").length,
      "N11.OPEN.FAILURE_IS_VISIBLE",
    ).toBeGreaterThan(0);
    await expectNoWcagViolations(container);
  });

  // -------------------------------------------------------------------------------------------
  // N15: a Project-scoped preflight display must not survive opening another Project
  // -------------------------------------------------------------------------------------------
  //
  // App.tsx:1216-1255 `openProject` replaces the Project, Charter, Policy, Repository, Team and
  // Mission state but never touches `preflightReceipt` (declared at :315). Projects.tsx:723-728
  // hands that state straight to CheckPreflightSection, which speaks about "this Project"
  // (CheckPreflight.tsx:78) and renders the stored commands and repository revision without ever
  // receiving a Project key. Whether an ordinary UI path reaches it is what this measures.

  const n15Charter = {
    revision: 1,
    purpose: "Ship the thing.",
    desiredOutcomes: ["Reach the named Project"],
    constraints: [],
    prohibitions: [],
    autonomyProfile: "supervised",
    budgetBoundary: "no_spend",
    externalEffects: "prohibited",
    authoredAtMs: 123,
  } as const;

  function n15Project(id: string, name: string) {
    return {
      id,
      workspaceId: acceptedOrganization.workspaceId,
      name,
      charter: n15Charter,
      createdAtMs: 123,
    } as const;
  }

  function n15Repository(projectId: string, command: string) {
    return {
      workspaceId: acceptedOrganization.workspaceId,
      projectId,
      revision: 1,
      charterRevision: 1,
      authorKind: "operator",
      authoredAtMs: 200,
      appliesToCurrentCharter: true,
      sourceKind: "local_path",
      source: "/srv/repo",
      baseRevision: "0".repeat(40),
      declaredChecks: [{ name: "gate", argv: [command], workingDirectory: "." }],
    } as const;
  }

  function n15Receipt(projectId: string, runId: string, command: string) {
    return {
      version: 1,
      runId,
      workspaceId: acceptedOrganization.workspaceId,
      projectId,
      repositoryRevision: 1,
      charterRevision: 1,
      outcome: "verified",
      sourceKind: "local_path",
      source: "/srv/repo",
      baseRevision: "0".repeat(40),
      resolvedChecks: [
        {
          name: "gate",
          argv: [command],
          declaredWorkingDirectory: ".",
          resolvedWorkingDirectory: { kind: "present", value: "/srv/repo" },
        },
      ],
      sandboxVersion: "sandbox-1",
      policyCharterRevision: 1,
      policyRepositoryRevision: 1,
      startedAtMs: 1_756_000_000_000,
      endedAtMs: 1_756_000_000_001,
      notVerified: [],
      unavailable: {
        proposalDigest: { kind: "absent", reason: "no Proposal is cited" },
        rulingDigest: { kind: "absent", reason: "no Ruling is cited" },
        resultingRevision: { kind: "absent", reason: "nothing was executed" },
        filesystemEffects: { kind: "absent", reason: "nothing was executed" },
        diffDigest: { kind: "absent", reason: "nothing was executed" },
        modelInvocations: { kind: "absent", reason: "nothing was invoked" },
        costUsdMicros: { kind: "absent", reason: "nothing was spent" },
      },
    } as const;
  }

  const N15_A = "66666666-6666-4666-8666-666666666666";
  const N15_B = "77777777-7777-4777-8777-777777777777";
  const N15_A_COMMAND = "n15-alpha-only-command";
  const N15_B_COMMAND = "n15-bravo-only-command";

  function installN16ProjectAdapter(
    options: {
      alphaPreflight?: Promise<ReturnType<typeof n15Receipt>>;
      bravoProjectFailure?: unknown;
    } = {},
  ) {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const alpha = n15Project(N15_A, "Alpha Project");
    const bravo = n15Project(N15_B, "Bravo Project");
    invokeMock.mockImplementation((command, args) => {
      const projectId = (args as { projectId?: string } | undefined)?.projectId;
      if (command === "projects") {
        return Promise.resolve([alpha, bravo].map((project) => ({
          id: project.id,
          name: project.name,
          charterRevision: 1,
          autonomyProfile: "supervised",
          createdAtMs: 123,
        })));
      }
      if (command === "project") {
        if (projectId === N15_B && "bravoProjectFailure" in options) {
          return Promise.reject(options.bravoProjectFailure);
        }
        return Promise.resolve(projectId === N15_B ? bravo : alpha);
      }
      if (command === "project_charter_history") return Promise.resolve([n15Charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") {
        return Promise.resolve(
          n15Repository(projectId ?? N15_A, projectId === N15_B ? N15_B_COMMAND : N15_A_COMMAND),
        );
      }
      if (command === "project_repository_history") {
        return Promise.resolve([
          n15Repository(projectId ?? N15_A, projectId === N15_B ? N15_B_COMMAND : N15_A_COMMAND),
        ]);
      }
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      if (command === "preflight_project_checks") {
        if (projectId === N15_A && options.alphaPreflight) return options.alphaPreflight;
        return Promise.resolve(
          projectId === N15_B
            ? n15Receipt(N15_B, "88888888-8888-4888-8888-888888888888", N15_B_COMMAND)
            : n15Receipt(N15_A, "99999999-9999-4999-8999-999999999999", N15_A_COMMAND),
        );
      }
      return base(command, args);
    });
  }

  it("does not show one Project's preflight result under another Project", async () => {
    const user = userEvent.setup();
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const alpha = n15Project(N15_A, "Alpha Project");
    const bravo = n15Project(N15_B, "Bravo Project");
    invokeMock.mockImplementation((command, args) => {
      const projectId = (args as { projectId?: string } | undefined)?.projectId;
      if (command === "projects") {
        return Promise.resolve([alpha, bravo].map((p) => ({
          id: p.id,
          name: p.name,
          charterRevision: 1,
          autonomyProfile: "supervised",
          createdAtMs: 123,
        })));
      }
      if (command === "project") return Promise.resolve(projectId === N15_B ? bravo : alpha);
      if (command === "project_charter_history") return Promise.resolve([n15Charter]);
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") {
        return Promise.resolve(
          n15Repository(projectId ?? N15_A, projectId === N15_B ? N15_B_COMMAND : N15_A_COMMAND),
        );
      }
      if (command === "project_repository_history") {
        return Promise.resolve([
          n15Repository(projectId ?? N15_A, projectId === N15_B ? N15_B_COMMAND : N15_A_COMMAND),
        ]);
      }
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      if (command === "preflight_project_checks") {
        return Promise.resolve(
          projectId === N15_B
            ? n15Receipt(N15_B, "88888888-8888-4888-8888-888888888888", N15_B_COMMAND)
            : n15Receipt(N15_A, "99999999-9999-4999-8999-999999999999", N15_A_COMMAND),
        );
      }
      return base(command, args);
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));

    // Project A, through the ordinary Projects list, then its ordinary Preflight control.
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    expect(await screen.findByText(N15_A_COMMAND)).toBeVisible();
    await expectNoWcagViolations(container);

    // Now open B the same ordinary way -- back to the Projects list through the rail, then the
    // Bravo row. Nothing is injected and no component state is reached into.
    // There is no "back to the list" control once a Project is open, so B is reached the other
    // ordinary way an operator opens a Project: a Beacon link, which runs the same `openProject`.
    await user.click(screen.getByRole("button", { name: "Briefing" }));
    await user.type(
      await screen.findByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${N15_B}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));
    expect(
      await screen.findByRole("heading", { name: "Bravo Project" }),
      "N15.PROJECT_SCOPE.OLD_PREFLIGHT_ABSENT",
    ).toBeVisible();
    expect(
      screen.queryByText(N15_A_COMMAND),
      "N15.PROJECT_SCOPE.OLD_PREFLIGHT_ABSENT",
    ).toBeNull();
  });

  it("blue shows Project B's own subsequent preflight result", async () => {
    const user = userEvent.setup();
    installN16ProjectAdapter();
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    expect(await screen.findByText(N15_A_COMMAND)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Briefing" }));
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${N15_B}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));
    await screen.findByRole("heading", { name: "Bravo Project" });
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    expect(
      await screen.findByText(N15_B_COMMAND),
      "N16.PROJECT_SCOPE.B_OWN_RESULT_VISIBLE",
    ).toBeVisible();
    expect(screen.queryByText(N15_A_COMMAND)).toBeNull();
    await expectNoWcagViolations(container);
  });

  it("blue preserves Project A's display when opening B is refused", async () => {
    const user = userEvent.setup();
    installN16ProjectAdapter({
      bravoProjectFailure: { code: "project_absent", message: "the named Project was refused" },
    });
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    expect(await screen.findByText(N15_A_COMMAND)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Briefing" }));
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${N15_B}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));
    await screen.findByText("Beacon could not open the Project named by that link.");
    expect(
      screen.getByRole("heading", { name: "Alpha Project" }),
      "N16.PROJECT_SCOPE.REFUSED_B_PRESERVES_A",
    ).toBeVisible();
    expect(screen.getByText(N15_A_COMMAND)).toBeVisible();
    await expectNoWcagViolations(container);
  });

  it("blue does not let a held Project A preflight result reappear under B", async () => {
    const user = userEvent.setup();
    let releaseAlpha!: (receipt: ReturnType<typeof n15Receipt>) => void;
    const heldAlpha = new Promise<ReturnType<typeof n15Receipt>>((resolve) => {
      releaseAlpha = resolve;
    });
    installN16ProjectAdapter({ alphaPreflight: heldAlpha });
    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    await screen.findByRole("button", { name: "Preflighting…" });

    const briefing = screen.getByRole("button", { name: "Briefing" });
    expect(briefing, "N16.LATE_RESULT.BEACON_LINK_REMAINS_REACHABLE").toBeEnabled();
    await user.click(briefing);
    await user.type(
      screen.getByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${N15_B}`,
    );
    const openLink = screen.getByRole("button", { name: "Open Beacon link" });
    expect(openLink, "N16.LATE_RESULT.BEACON_LINK_REMAINS_REACHABLE").toBeEnabled();
    await user.click(openLink);
    await screen.findByRole("heading", { name: "Bravo Project" });
    expect(screen.queryByText(N15_A_COMMAND)).toBeNull();

    await act(async () => {
      releaseAlpha(n15Receipt(N15_A, "99999999-9999-4999-8999-999999999999", N15_A_COMMAND));
      await heldAlpha;
    });
    expect(
      screen.queryByText(N15_A_COMMAND),
      "N16.LATE_RESULT.HELD_A_MUST_NOT_REAPPEAR_UNDER_B",
    ).toBeNull();
    await expectNoWcagViolations(container);
  });

  it("keeps Project B's own preflight when a held Project A result completes afterwards", async () => {
    // The fifth case of the table: A pending, B opened, B's OWN preflight completes, and only then
    // A's older observation lands. B's valid result must survive it -- a retired observation must
    // not erase a current one any more than it may replace it.
    const user = userEvent.setup();
    let releaseAlpha!: (receipt: unknown) => void;
    const heldAlpha = new Promise<unknown>((resolve) => {
      releaseAlpha = resolve;
    });
    installN16ProjectAdapter({
      alphaPreflight: heldAlpha as Promise<ReturnType<typeof n15Receipt>>,
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));

    // B is opened while A's read is still held, then B runs its own preflight to completion.
    await user.click(screen.getByRole("button", { name: "Briefing" }));
    await user.type(
      await screen.findByLabelText("Beacon link"),
      `beacon:/${acceptedOrganization.workspaceId}/project/${N15_B}`,
    );
    await user.click(screen.getByRole("button", { name: "Open Beacon link" }));
    await screen.findByRole("heading", { name: "Bravo Project" });
    await user.click(await screen.findByRole("button", { name: "Preflight checks" }));
    expect(await screen.findByText(N15_B_COMMAND)).toBeVisible();
    await expectNoWcagViolations(container);

    // Now A's older observation completes.
    await act(async () => {
      releaseAlpha(n15Receipt(N15_A, "99999999-9999-4999-8999-999999999999", N15_A_COMMAND));
      await Promise.resolve();
    });

    expect(
      screen.queryByText(N15_A_COMMAND),
      "N16.LATE_RESULT.HELD_A_MUST_NOT_ERASE_B",
    ).toBeNull();
    expect(
      screen.queryByText(N15_B_COMMAND),
      "N16.LATE_RESULT.HELD_A_MUST_NOT_ERASE_B",
    ).not.toBeNull();
  });

  // -------------------------------------------------------------------------------------------
  // N17: return from an open Project to the portfolio
  // -------------------------------------------------------------------------------------------
  //
  // N15 measured that once a Project is open there is no way back to the list: the rail's Projects
  // button keeps the selection, and the only other route is a Beacon link. These bars are that
  // missing navigation, asserted on what the operator can actually see.

  const n17Charter = {
    revision: 1,
    purpose: "Ship the thing.",
    desiredOutcomes: ["Reach the named Project"],
    constraints: [],
    prohibitions: [],
    autonomyProfile: "supervised",
    budgetBoundary: "no_spend",
    externalEffects: "prohibited",
    authoredAtMs: 123,
  } as const;
  const N17_A = "aaaaaaa1-1111-4111-8111-aaaaaaaaaaaa";
  const N17_B = "bbbbbbb2-2222-4222-8222-bbbbbbbbbbbb";
  // Held fields, not derived from any callback: each Project's purpose is its own witness.
  const N17_A_PURPOSE = "n17-alpha-held-purpose";
  const N17_B_PURPOSE = "n17-bravo-held-purpose";

  function n17Project(id: string, name: string, purpose: string) {
    return {
      id,
      workspaceId: acceptedOrganization.workspaceId,
      name,
      charter: { ...n17Charter, purpose },
      createdAtMs: 123,
    } as const;
  }

  function mockN17(extra?: (command: string, args: unknown) => Promise<unknown> | undefined) {
    mockInitialState("unlocked");
    const base = invokeMock.getMockImplementation()!;
    const alpha = n17Project(N17_A, "Alpha Project", N17_A_PURPOSE);
    const bravo = n17Project(N17_B, "Bravo Project", N17_B_PURPOSE);
    const listReads: unknown[] = [];
    invokeMock.mockImplementation((command, args) => {
      const projectId = (args as { projectId?: string } | undefined)?.projectId;
      const injected = extra?.(command, args);
      if (injected !== undefined) return injected;

      if (command === "projects") {
        listReads.push(args);
        return Promise.resolve([alpha, bravo].map((p) => ({
          id: p.id,
          name: p.name,
          charterRevision: 1,
          autonomyProfile: "supervised",
          createdAtMs: 123,
        })));
      }
      if (command === "project") return Promise.resolve(projectId === N17_B ? bravo : alpha);
      if (command === "project_charter_history") {
        return Promise.resolve([projectId === N17_B ? bravo.charter : alpha.charter]);
      }
      if (command === "project_policy") return Promise.resolve(null);
      if (command === "project_policy_history") return Promise.resolve([]);
      if (command === "current_project_repository") return Promise.resolve(null);
      if (command === "project_repository_history") return Promise.resolve([]);
      if (command === "teams" || command === "missions") return Promise.resolve([]);
      return base(command, args);
    });
    return { listReads };
  }

  it("returns from an open Project to the portfolio and opens another by its own id", async () => {
    const user = userEvent.setup();
    const { listReads } = mockN17();

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    expect((await screen.findAllByText(N17_A_PURPOSE)).length).toBeGreaterThan(0);
    const readsAfterOpen = listReads.length;

    // The new control, reached and activated from the keyboard.
    const back = await screen.findByRole("button", { name: "All Projects" });
    back.focus();
    expect(back, "N17.LIST.RETURN_VISIBLE").toHaveFocus();
    await user.keyboard("{Enter}");

    // Settle on something independent of the property under test -- Alpha's record leaving the
    // screen -- so the named assertions below are always reached. Under the removed-clear mutant
    // this waitFor is what fails, and it fails BY NAME rather than inside a findBy that throws
    // before any assertion runs.
    await vi.waitFor(() =>
      expect(
        screen.queryAllByText(N17_A_PURPOSE),
        "N17.LIST.RETURN_VISIBLE",
      ).toHaveLength(0),
    );
    expect(
      screen.queryByRole("button", { name: /Alpha Project/i }),
      "N17.LIST.RETURN_VISIBLE",
    ).not.toBeNull();
    expect(
      screen.queryByRole("button", { name: /Bravo Project/i }),
      "N17.LIST.RETURN_VISIBLE",
    ).not.toBeNull();
    await expectNoWcagViolations(container);

    // The cached list was not re-read to get back to it.
    expect(listReads.length, "N17.LIST.NO_EXTRA_LIST_READ").toBe(readsAfterOpen);

    // And the next Project opens by its own held id, showing its own held purpose.
    await user.click(screen.getByRole("button", { name: /Bravo Project/i }));
    expect(
      (await screen.findAllByText(N17_B_PURPOSE)).length,
      "N17.LIST.NEXT_PROJECT_ID",
    ).toBeGreaterThan(0);
    expect(screen.queryAllByText(N17_A_PURPOSE), "N17.LIST.NEXT_PROJECT_ID").toHaveLength(0);
    expect(
      screen.getByRole("heading", { name: "Bravo Project" }),
      "N17.LIST.NEXT_PROJECT_ID",
    ).toBeVisible();
  });

  it("disables returning to the portfolio while other work is in flight", async () => {
    const user = userEvent.setup();
    // `Form Team` runs `prepareTeamFormation`, which loads Resident configurations inside
    // `runAction` -- so holding that read holds the shared busy state open.
    let releaseConfigurations!: (configurations: unknown[]) => void;
    mockN17((command) => {
      if (command === "resident_configurations") {
        return new Promise((resolve) => {
          releaseConfigurations = resolve as (configurations: unknown[]) => void;
        });
      }
      return undefined;
    });

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await screen.findAllByText(N17_A_PURPOSE);
    await expectNoWcagViolations(container);

    // A held Team read keeps the shared busy state set; the new control follows it rather than
    // inventing request semantics of its own.
    await user.click(await screen.findByRole("button", { name: "Form Team" }));
    expect(
      await screen.findByRole("button", { name: "All Projects" }),
      "N17.LIST.BUSY_DISABLED",
    ).toBeDisabled();
    await act(async () => {
      releaseConfigurations([]);
      await Promise.resolve();
    });
  });

  it("cannot silently discard an unsaved Repository draft, and Cancel restores the way back", async () => {
    const user = userEvent.setup();
    mockN17();
    const HELD_DRAFT = "n17-held-unsaved-repository-source";

    const { container } = render(<App />);
    await screen.findByRole("heading", { name: "Briefing" });
    await user.click(screen.getByRole("button", { name: "Projects" }));
    await user.click(await screen.findByRole("button", { name: /Alpha Project/i }));
    await screen.findAllByText(N17_A_PURPOSE);

    // The way back exists before any composer is open.
    expect(
      screen.queryByRole("button", { name: "All Projects" }),
      "N18.COMPOSER.DRAFT_CANNOT_BE_SILENTLY_DISCARDED",
    ).not.toBeNull();

    // Open the real Repository form and hold a real value in its own scoped input.
    await user.click(await screen.findByRole("button", { name: "Bind Repository" }));
    const form = await screen.findByRole("form", { name: "Bind Repository" });
    const source = within(form).getByLabelText("Repository source");
    await user.type(source, HELD_DRAFT);
    expect(source, "N18.COMPOSER.DRAFT_CANNOT_BE_SILENTLY_DISCARDED").toHaveValue(HELD_DRAFT);
    await expectNoWcagViolations(container);

    // With a draft held, there is no control that would discard it without Cancel.
    expect(
      screen.queryByRole("button", { name: "All Projects" }),
      "N18.COMPOSER.DRAFT_CANNOT_BE_SILENTLY_DISCARDED",
    ).toBeNull();
    expect(
      within(await screen.findByRole("form", { name: "Bind Repository" })).getByLabelText(
        "Repository source",
      ),
      "N18.COMPOSER.DRAFT_CANNOT_BE_SILENTLY_DISCARDED",
    ).toHaveValue(HELD_DRAFT);

    // The form's own Cancel is the way out, and it brings the way back with it.
    await user.click(within(form).getByRole("button", { name: "Cancel" }));
    await vi.waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "All Projects" }),
        "N17.LIST.RETURN_VISIBLE",
      ).not.toBeNull(),
    );
    await user.click(screen.getByRole("button", { name: "All Projects" }));
    await vi.waitFor(() =>
      expect(screen.queryAllByText(N17_A_PURPOSE), "N17.LIST.RETURN_VISIBLE").toHaveLength(0),
    );
    expect(
      screen.queryByRole("button", { name: /Bravo Project/i }),
      "N17.LIST.RETURN_VISIBLE",
    ).not.toBeNull();
  });
});
