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
    if (command === "record_operator_activity") return Promise.resolve(null);
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
      if (command === "projects") return Promise.resolve([]);
      if (command === "create_project") return Promise.resolve(created);
      if (command === "revise_project_charter") return Promise.resolve(revised);
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
      if (command === "projects") return Promise.resolve([summary]);
      if (command === "project") return Promise.resolve(project);
      if (command === "project_charter_history") {
        return Promise.resolve([revisionOne, revisionTwo]);
      }
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
    mockInitialState("unlocked");
    invokeMock.mockImplementation((command, args) => {
      if (command === "session_status") return Promise.resolve(status("unlocked"));
      if (command === "authentication_capabilities") return Promise.resolve(capabilities);
      if (command === "session_policy") return Promise.resolve(sessionPolicy);
      if (command === "platform_session_monitor_status") return Promise.resolve(monitorStatus);
      if (command === "record_operator_activity") return Promise.resolve(null);
      if (command === "starter_organization") return Promise.resolve(proposedOrganization);
      if (command === "accept_starter_organization") return Promise.resolve(accepted);
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
    expect(screen.getByText("2 inactive Residents")).toBeVisible();
    expect(screen.getByText("No Resident has execution authority yet.")).toBeVisible();
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
