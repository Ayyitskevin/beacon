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

    expect(await screen.findByRole("heading", { name: "Owner workspace" })).toBeVisible();
    expect(invokeMock).toHaveBeenCalledWith("complete_operator_setup", { recoverySaved: true });
    const suiteNavigation = screen.getByRole("navigation", { name: "Beacon applications" });
    for (const button of within(suiteNavigation).getAllByRole("button")) {
      expect(button).toBeEnabled();
    }
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
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Welcome back" });
    await user.type(screen.getByLabelText("Password"), "a long local passphrase");
    await user.click(screen.getByRole("button", { name: "Unlock Beacon" }));
    expect(await screen.findByRole("heading", { name: "Owner workspace" })).toBeVisible();
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
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByRole("heading", { name: "Owner workspace" })).toBeVisible();

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
      if (command === "record_operator_activity") return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected command: ${command}`));
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "Owner workspace" })).toBeVisible();

    fireEvent.blur(window);
    expect(screen.getByRole("heading", { name: "Beacon is concealed" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Owner workspace" })).not.toBeInTheDocument();
    fireEvent.focus(window);

    expect(await screen.findByRole("heading", { name: "Welcome back" })).toBeVisible();
    expect(screen.queryByRole("heading", { name: "Owner workspace" })).not.toBeInTheDocument();
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
    expect(screen.getByText("OS lock monitoring active")).toBeVisible();
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
});
