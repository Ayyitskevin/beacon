import axe from "axe-core";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import { BriefingReceipts, type ReceiptRow } from "./BriefingReceipts";
import type { PreflightOutcome, PreflightReceipt } from "./session";

const absent = (reason: string) => ({ kind: "absent" as const, reason });

function receipt(
  runId: string,
  outcome: PreflightOutcome,
  startedAtMs: number,
): PreflightReceipt {
  return {
    version: 1,
    runId,
    workspaceId: "1a2b3c4d-5e6f-4a8b-9c0d-1e2f3a4b5c6d",
    projectId: "2b3c4d5e-6f7a-4b8c-9d0e-2f3a4b5c6d7e",
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
      proposalDigest: absent("no Proposal surface exists"),
      rulingDigest: absent("no Proposal surface exists"),
      resultingRevision: absent("preflight refuses to execute, so nothing ran"),
      filesystemEffects: absent("preflight refuses to execute, so nothing ran"),
      diffDigest: absent("preflight refuses to execute, so nothing ran"),
      modelInvocations: absent("preflight performs no model inference"),
      costUsdMicros: absent("preflight performs no model inference"),
    },
  };
}

function row(
  runId: string,
  outcome: PreflightOutcome,
  startedAtMs: number,
  projectName = "Task tracker",
): ReceiptRow {
  return { receipt: receipt(runId, outcome, startedAtMs), projectName };
}

it("lists recent Receipts newest first, not in the order they arrived", async () => {
  const rows = [
    row("aaaaaaaa-1111-4111-8111-111111111111", "verified", 100),
    row("cccccccc-3333-4333-8333-333333333333", "preflight_failed", 300),
    row("bbbbbbbb-2222-4222-8222-222222222222", "sandbox_unavailable", 200),
  ];

  const { container } = render(
    <BriefingReceipts rows={rows} total={3} loading={false} failure="" busy={false} onRefresh={vi.fn()} />,
  );

  const listed = within(screen.getByRole("list", { name: "Recent preflight Receipts" })).getAllByRole(
    "listitem",
  );
  expect(listed.map((item) => item.textContent)).toEqual([
    expect.stringContaining("Run cccccccc"),
    expect.stringContaining("Run bbbbbbbb"),
    expect.stringContaining("Run aaaaaaaa"),
  ]);

  const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
  expect(results.violations).toEqual([]);
});

it("does not render a verified Receipt as a passing check", () => {
  render(
    <BriefingReceipts
      rows={[row("aaaaaaaa-1111-4111-8111-111111111111", "verified", 100)]}
      total={1}
      loading={false}
      failure=""
      busy={false}
      onRefresh={vi.fn()}
    />,
  );

  // Preflight runs no check. A row that says "passed" claims something no
  // Receipt in this set records.
  expect(screen.getByText(/not a passing check/)).toBeVisible();
  const text = screen.getByRole("list", { name: "Recent preflight Receipts" }).textContent ?? "";
  expect(text).not.toMatch(/passed|succeeded|completed/i);
});

it("states each refusal as its own outcome rather than one shared failure", () => {
  render(
    <BriefingReceipts
      rows={[
        row("aaaaaaaa-1111-4111-8111-111111111111", "sandbox_unavailable", 300),
        row("bbbbbbbb-2222-4222-8222-222222222222", "base_revision_unreachable", 200),
        row("cccccccc-3333-4333-8333-333333333333", "preflight_timed_out", 100),
      ]}
      total={3}
      loading={false}
      failure=""
      busy={false}
      onRefresh={vi.fn()}
    />,
  );

  expect(screen.getByText(/Refused — no sandbox/)).toBeVisible();
  expect(screen.getByText(/Refused — base revision absent/)).toBeVisible();
  // "did not finish" is not "errored": an unfinished step's answer is unknown.
  expect(screen.getByText(/Refused — a step did not finish/)).toBeVisible();
});

it("says a truncated list is truncated instead of implying completeness", () => {
  const rows = Array.from({ length: 9 }, (_, index) =>
    row(`0000000${index}-1111-4111-8111-11111111111${index}`, "verified", 100 + index),
  );

  render(
    <BriefingReceipts rows={rows} total={9} loading={false} failure="" busy={false} onRefresh={vi.fn()} />,
  );

  expect(
    within(screen.getByRole("list", { name: "Recent preflight Receipts" })).getAllByRole("listitem"),
  ).toHaveLength(5);
  expect(screen.getByText(/Showing the 5 most recent of 9/)).toBeVisible();
  // No Project-open path reads this Query, so pointing there is a promise the
  // interface cannot keep. State the scope instead of inventing a destination.
  expect(screen.queryByText(/Open a Project/)).toBeNull();
  expect(screen.getByText(/Beacon has no full preflight history view yet/)).toBeVisible();
});

it("keeps empty, loading and failed apart", async () => {
  const user = userEvent.setup();
  const onRefresh = vi.fn();
  const { rerender } = render(
    <BriefingReceipts rows={null} total={0} loading failure="" busy={false} onRefresh={onRefresh} />,
  );
  expect(screen.getByText("Loading Receipts")).toBeVisible();
  expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();

  rerender(
    <BriefingReceipts rows={[]} total={0} loading={false} failure="" busy={false} onRefresh={onRefresh} />,
  );
  expect(screen.getByText("No preflight Receipts yet.")).toBeVisible();

  rerender(
    <BriefingReceipts
      rows={null}
      total={0}
      loading={false}
      failure="The local account service is unavailable."
      busy={false}
      onRefresh={onRefresh}
    />,
  );
  // A read that failed is not a Workspace with no Receipts.
  expect(screen.getByText(/could not be read/)).toBeVisible();
  expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();

  await user.click(screen.getByRole("button", { name: "Refresh Receipts" }));
  expect(onRefresh).toHaveBeenCalledTimes(1);
});

it("says the list is preflight Receipts, not every Receipt Beacon writes", () => {
  render(
    <BriefingReceipts
      rows={[row("aaaaaaaa-1111-4111-8111-111111111111", "verified", 100)]}
      total={1}
      loading={false}
      failure=""
      busy={false}
      onRefresh={vi.fn()}
    />,
  );

  // Beacon already writes execution Receipts for declared-check Runs; this
  // Query filters runs.kind = 'check_preflight'. Calling the result "Recent
  // Receipts" presents a kind-filtered subset as the whole.
  expect(screen.getByRole("heading", { name: "Recent preflight Receipts" })).toBeVisible();
  expect(screen.getByRole("list", { name: "Recent preflight Receipts" })).toBeVisible();
  const section = screen.getByRole("heading", { name: "Recent preflight Receipts" }).parentElement;
  expect(section?.textContent).not.toMatch(/what each Run did/);
  expect(screen.getByText(/does not include execution Receipts/)).toBeVisible();
});
