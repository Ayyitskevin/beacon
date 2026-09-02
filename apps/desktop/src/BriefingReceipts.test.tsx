import axe from "axe-core";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import {
  BriefingReceipts,
  RECEIPT_AGGREGATE_LIMIT,
  RECEIPT_HOLD_LIMIT,
  collectRecentReceipts,
  type ReceiptRow,
} from "./BriefingReceipts";
import { MAX_PROJECT_RECEIPT_PAGE } from "./session";
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

it("lists recent preflight Receipts newest first, not in the order they arrived", async () => {
  const rows = [
    row("aaaaaaaa-1111-4111-8111-111111111111", "verified", 100),
    row("cccccccc-3333-4333-8333-333333333333", "preflight_failed", 300),
    row("bbbbbbbb-2222-4222-8222-222222222222", "sandbox_unavailable", 200),
  ];

  const { container } = render(
    <BriefingReceipts rows={rows} total={3} loading={false} failure="" scope="" disabled={false} onRefresh={vi.fn()} />,
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
      scope=""
      disabled={false}
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
      scope=""
      disabled={false}
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
    <BriefingReceipts rows={rows} total={9} loading={false} failure="" scope="" disabled={false} onRefresh={vi.fn()} />,
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
    <BriefingReceipts rows={null} total={0} loading failure="" scope="" disabled={false} onRefresh={onRefresh} />,
  );
  expect(screen.getByText("Loading preflight Receipts")).toBeVisible();
  expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();

  rerender(
    <BriefingReceipts rows={[]} total={0} loading={false} failure="" scope="" disabled={false} onRefresh={onRefresh} />,
  );
  // An empty scope means the reading was complete, so the absolute sentence is
  // earned.
  expect(screen.getByText("No preflight Receipts yet.")).toBeVisible();

  rerender(
    <BriefingReceipts
      rows={null}
      total={0}
      loading={false}
      failure="The local account service is unavailable."
      scope=""
      disabled={false}
      onRefresh={onRefresh}
    />,
  );
  // A read that failed is not a Workspace with no Receipts.
  expect(screen.getByText(/could not be read/)).toBeVisible();
  expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();

  await user.click(screen.getByRole("button", { name: "Refresh preflight Receipts" }));
  expect(onRefresh).toHaveBeenCalledTimes(1);
});

it("says the list is preflight Receipts, not every Receipt Beacon writes", () => {
  render(
    <BriefingReceipts
      rows={[row("aaaaaaaa-1111-4111-8111-111111111111", "verified", 100)]}
      total={1}
      loading={false}
      failure=""
      scope=""
      disabled={false}
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

/** Eight Projects, each at the adapter's page cap: 4,096 rows exist. */
function fullPages() {
  const projects = Array.from({ length: 8 }, (_, index) => ({
    id: `${index.toString(16).padStart(8, "0")}-4444-4444-8444-444444444444`,
    name: `Project ${index}`,
  }));
  const page = (index: number) =>
    Array.from({ length: MAX_PROJECT_RECEIPT_PAGE }, (_, row) =>
      receipt(
        `${(index * 1000 + row).toString(16).padStart(8, "0")}-1111-4111-8111-111111111111`,
        "verified",
        index * 1000 + row,
      ),
    );
  return { projects, page };
}

it("never holds more than the cap plus one page, whatever the Workspace contains", async () => {
  // The bound this collection documents, MEASURED. A rendered list of five
  // looks identical whether the reading held 768 rows or 4,096, which is how
  // the previous guard survived a gather-every-page-then-slice implementation:
  // it observed only totals and list items, never the collection.
  const { projects, page } = fullPages();
  const collected = await collectRecentReceipts(projects, async (projectId) =>
    page(projects.findIndex((project) => project.id === projectId)),
  );

  expect(collected.found).toBe(8 * MAX_PROJECT_RECEIPT_PAGE);
  expect(collected.rows).toHaveLength(RECEIPT_AGGREGATE_LIMIT);
  expect(collected.peakHeld).toBeLessThanOrEqual(RECEIPT_HOLD_LIMIT);
  // And the bound is not vacuous: a reading of this size does approach it.
  expect(collected.peakHeld).toBeGreaterThan(RECEIPT_AGGREGATE_LIMIT);
});

it("reads one Project at a time, because a page in flight is a page held", async () => {
  // Four concurrent reads of a 512-row page held 2,048 rows beside the 256
  // kept -- three times the documented bound. Concurrency here is not a
  // scheduling choice, it is retention.
  const { projects, page } = fullPages();
  let inFlight = 0;
  let peakInFlight = 0;
  const collected = await collectRecentReceipts(projects, async (projectId) => {
    inFlight += 1;
    peakInFlight = Math.max(peakInFlight, inFlight);
    await Promise.resolve();
    inFlight -= 1;
    return page(projects.findIndex((project) => project.id === projectId));
  });

  expect(peakInFlight).toBe(1);
  expect(collected.rows).toHaveLength(RECEIPT_AGGREGATE_LIMIT);
});

it("keeps the newest rows across the merge, not the first page it read", async () => {
  // Trimming per page is only correct if the trim is by the merge key. A
  // collection that kept whichever page arrived first would draw the oldest.
  //
  // At the REAL cap, not a small one passed in for the test's convenience. The
  // parameter that allowed that made the function's documented bound a promise
  // about its default: a valid call could hold twice it.
  const projects = [
    { id: "00000001-4444-4444-8444-444444444444", name: "Older" },
    { id: "00000002-4444-4444-8444-444444444444", name: "Newer" },
  ];
  // The first Project alone fills the cap, so the newer row can only appear if
  // the merge re-sorts rather than keeping what it already had.
  const older = Array.from({ length: RECEIPT_AGGREGATE_LIMIT }, (_, index) =>
    receipt(
      `${index.toString(16).padStart(8, "0")}-1111-4111-8111-111111111111`,
      "verified",
      index,
    ),
  );
  const collected = await collectRecentReceipts(projects, async (projectId) =>
    projectId === projects[0].id
      ? older
      : [receipt("bbbbbbbb-2222-4222-8222-222222222222", "verified", 900)],
  );

  expect(collected.found).toBe(RECEIPT_AGGREGATE_LIMIT + 1);
  expect(collected.rows).toHaveLength(RECEIPT_AGGREGATE_LIMIT);
  // The newest row is the one that arrived last, and the oldest was dropped to
  // make room for it.
  expect(collected.rows[0].projectName).toBe("Newer");
  expect(collected.rows.map((row) => row.receipt.startedAtMs)).not.toContain(0);
});

it("requires exactly the projects and the page read", async () => {
  // gpt2's round-four finding: the function took an optional cap defaulting to
  // the aggregate limit, so a valid call with 1024 over eight full pages
  // returned 1,024 rows and held 1,536 against a documented bound of 768. The
  // parameter had exactly one caller -- a test -- and removing it makes that
  // call unwritable rather than merely unwise.
  //
  // What this asserts is only what it can: `length` counts REQUIRED
  // parameters, so it fails a third required one and would NOT fail an
  // optional one. The compiler is what makes the optional shape unwritable
  // (`Expected 2 arguments, but got 3`), and that is stated here rather than
  // left for a reader to assume this test covers it.
  expect(collectRecentReceipts.length).toBe(2);
  // And the bound holds at the far end of the input range too: no Projects is
  // no rows held, not an empty answer arrived at by holding everything.
  const empty = await collectRecentReceipts([], async () => []);
  expect(empty).toEqual({ rows: [], found: 0, peakHeld: 0 });
});

it("does not call a bounded reading an empty Workspace", async () => {
  // The Project budget bit and the Projects it opened had no Receipts. "No
  // preflight Receipts yet" is an absolute claim about the Workspace, and it
  // rendered directly above a sentence admitting only 8 of 513 Projects were
  // read -- so the 505 unread Projects were denied by one sentence and
  // disclosed by the next.
  const onRefresh = vi.fn();
  render(
    <BriefingReceipts
      rows={[]}
      total={0}
      loading={false}
      failure=""
      scope="this reading covered the 8 most recent Projects of 513"
      disabled={false}
      onRefresh={onRefresh}
    />,
  );

  expect(screen.getByText("No preflight Receipts in this reading.")).toBeVisible();
  expect(screen.queryByText("No preflight Receipts yet.")).toBeNull();
  // And the scope that makes the qualification true is on screen beside it.
  expect(
    screen.getByText(/this reading covered the 8 most recent Projects of 513/),
  ).toBeVisible();
});

it("earns the absolute empty sentence only when the reading was complete", async () => {
  // The other half of the same rule: a reading that covered every Project and
  // kept every Receipt it found HAS established that the Workspace has none,
  // and qualifying that would be weaker than the facts support.
  const onRefresh = vi.fn();
  render(
    <BriefingReceipts
      rows={[]}
      total={0}
      loading={false}
      failure=""
      scope=""
      disabled={false}
      onRefresh={onRefresh}
    />,
  );

  expect(screen.getByText("No preflight Receipts yet.")).toBeVisible();
  expect(screen.queryByText("No preflight Receipts in this reading.")).toBeNull();
});
