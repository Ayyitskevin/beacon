import { invoke } from "@tauri-apps/api/core";
import axe from "axe-core";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";

import { SampleProject } from "./SampleProject";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const invokeMock = vi.mocked(invoke);
const digest = {
  algorithm: "sha256",
  canonicalizer: "beacon.sample-project.bundle.v1",
  value: "7bdcf0a732856553c529dee2ec7dfcf9bbf691a5fb9b2558e74f05c1559dde8f",
} as const;
const installation = {
  workspaceId: "11111111-1111-4111-8111-111111111111",
  projectId: "33333333-3333-4333-8333-333333333333",
  charterRevision: 1,
  repositoryRevision: 1,
  repositoryPath: "/private/beacon/sample-projects/task-tracker-v1",
  baseRevision: "0123456789012345678901234567890123456789",
  provenance: {
    source: "beacon_bundled_sample_project",
    bundleVersion: 1,
    bundleDigest: digest,
  },
  installedAtMs: 123,
} as const;

beforeEach(() => {
  invokeMock.mockReset();
});

it("offers an explicit inert install and reports the created Project accessibly", async () => {
  const user = userEvent.setup();
  invokeMock.mockImplementation((command) => {
    if (command === "sample_project_status") {
      return Promise.resolve({
        state: "available",
        bundleVersion: 1,
        bundleDigest: digest,
      });
    }
    if (command === "install_sample_project") {
      return Promise.resolve({ outcome: "installed", installation });
    }
    return Promise.reject(new Error(`unexpected command: ${command}`));
  });

  const onInstalled = vi.fn();
  const { container } = render(<SampleProject onInstalled={onInstalled} />);

  expect(await screen.findByText("Optional local proving project")).toBeVisible();
  expect(screen.getByText(/starts no Run, model, network request, or execution/i)).toBeVisible();
  const results = await axe.run(container, {
    rules: { "color-contrast": { enabled: false } },
  });
  expect(results.violations).toEqual([]);
  await user.click(screen.getByRole("button", { name: "Install Sample Project" }));

  expect(invokeMock).toHaveBeenCalledWith("install_sample_project");
  expect(await screen.findByRole("status")).toHaveTextContent("Sample Project installed");
  expect(screen.getByText(/Supervised Project and two declared checks/i)).toBeVisible();
  expect(onInstalled).toHaveBeenCalledWith(installation.projectId);
});

it("refuses the backend's wrong-case available payload instead of rendering unverified state", async () => {
  invokeMock.mockResolvedValue({
    state: "available",
    bundle_version: 1,
    bundle_digest: digest,
  });

  render(<SampleProject />);

  expect(await screen.findByRole("alert")).toHaveTextContent("Beacon could not verify Sample Project status.");
  expect(screen.queryByRole("button", { name: "Install Sample Project" })).not.toBeInTheDocument();
});

it("reports refusal when installation returns an unverified payload", async () => {
  const user = userEvent.setup();
  const onInstalled = vi.fn();
  invokeMock.mockImplementation((command) => {
    if (command === "sample_project_status") {
      return Promise.resolve({
        state: "available",
        bundleVersion: 1,
        bundleDigest: digest,
      });
    }
    if (command === "install_sample_project") {
      return Promise.resolve({
        outcome: "installed",
        installation: { ...installation, unexpectedAuthority: true },
      });
    }
    return Promise.reject(new Error(`unexpected command: ${command}`));
  });

  render(<SampleProject onInstalled={onInstalled} />);
  await user.click(await screen.findByRole("button", { name: "Install Sample Project" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Beacon refused the Sample Project installation. No Project was published.",
  );
  expect(onInstalled).not.toHaveBeenCalled();
});

it("keeps the opt-in available and reports a Tauri installation refusal", async () => {
  const user = userEvent.setup();
  const onInstalled = vi.fn();
  invokeMock.mockImplementation((command) => {
    if (command === "sample_project_status") {
      return Promise.resolve({
        state: "available",
        bundleVersion: 1,
        bundleDigest: digest,
      });
    }
    if (command === "install_sample_project") {
      return Promise.reject({ code: "sample_project_unavailable" });
    }
    return Promise.reject(new Error(`unexpected command: ${command}`));
  });

  render(<SampleProject onInstalled={onInstalled} />);
  await user.click(await screen.findByRole("button", { name: "Install Sample Project" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Beacon refused the Sample Project installation. No Project was published.",
  );
  expect(screen.getByRole("button", { name: "Install Sample Project" })).toBeEnabled();
  expect(onInstalled).not.toHaveBeenCalled();
});

it("reports an explicit already-installed result and refreshes the Project portfolio", async () => {
  const user = userEvent.setup();
  const onInstalled = vi.fn();
  invokeMock.mockImplementation((command) => {
    if (command === "sample_project_status") {
      return Promise.resolve({
        state: "available",
        bundleVersion: 1,
        bundleDigest: digest,
      });
    }
    if (command === "install_sample_project") {
      return Promise.resolve({ outcome: "already_installed", installation });
    }
    return Promise.reject(new Error(`unexpected command: ${command}`));
  });

  render(<SampleProject onInstalled={onInstalled} />);
  await user.click(await screen.findByRole("button", { name: "Install Sample Project" }));

  expect(await screen.findByRole("status")).toHaveTextContent("Sample Project was already installed");
  expect(onInstalled).toHaveBeenCalledWith(installation.projectId);
});

it("renders a verified installed status on mount without offering installation again", async () => {
  invokeMock.mockResolvedValue({ state: "installed", installation });

  render(<SampleProject />);

  expect(await screen.findByText(/Supervised Project and two declared checks/i)).toBeVisible();
  expect(screen.getByText(`Revision ${installation.baseRevision}`)).toBeVisible();
  expect(screen.queryByRole("button", { name: "Install Sample Project" })).not.toBeInTheDocument();
  expect(invokeMock).toHaveBeenCalledTimes(1);
  expect(invokeMock).toHaveBeenCalledWith("sample_project_status");
});
