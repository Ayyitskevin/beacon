import axe from "axe-core";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import { BriefingReceipts } from "./BriefingReceipts";
import type { RecentReceipt, RecentReceiptPage } from "./session";

function receipt(
  runId: string,
  startedAtMs: number,
  outcome: RecentReceipt["outcome"],
  projectName = "Task tracker",
): RecentReceipt {
  return {
    runId,
    projectId: "2b3c4d5e-6f7a-4b8c-9d0e-2f3a4b5c6d7e",
    projectName,
    version: outcome.kind === "check_preflight" ? 1 : 2,
    outcome,
    startedAtMs,
    endedAtMs: startedAtMs + 1,
    recordedAtMs: startedAtMs + 1,
  };
}

function page(overrides: Partial<RecentReceiptPage> = {}): RecentReceiptPage {
  return {
    receipts: [],
    runsInFlightInWindow: 0,
    runsUnconfirmedInWindow: 0,
    runWindowApplied: 50,
    moreRunsExist: false,
    ...overrides,
  };
}

const preflight = (runId: string, at: number, outcome: "verified" | "preflight_failed") =>
  receipt(runId, at, { kind: "check_preflight", outcome });
const execution = (runId: string, at: number, summary: "completed" | "not_admissible") =>
  receipt(runId, at, { kind: "declared_checks", summary });

it("lists Receipts of every kind in the order the daemon sent them", async () => {
  // The panel adds no ordering. The daemon defines it and the adapter refuses a
  // page that is not in it; re-sorting here would put the ordering policy back
  // in the interface, which is what this Query exists to remove.
  const rows = [
    execution("cccccccc-3333-4333-8333-333333333333", 300, "completed"),
    preflight("bbbbbbbb-2222-4222-8222-222222222222", 200, "preflight_failed"),
    preflight("aaaaaaaa-1111-4111-8111-111111111111", 100, "verified"),
  ];
  render(
    <BriefingReceipts
      page={page({ receipts: rows })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  const list = screen.getByRole("list", { name: "Recent Receipts" });
  const items = within(list).getAllByRole("listitem");
  expect(items).toHaveLength(3);
  expect(items[0].textContent).toMatch(/Declared checks/);
  expect(items[1].textContent).toMatch(/Check preflight/);
  expect(items[2].textContent).toMatch(/Check preflight/);
});

it("never says a Declared Check passed", async () => {
  // ADR 0031: `completed` records that every check RAN. A Briefing row that
  // upgraded that to a pass would claim evidence the Receipt does not carry.
  render(
    <BriefingReceipts
      page={page({ receipts: [execution("cccccccc-3333-4333-8333-333333333333", 300, "completed")] })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  const list = screen.getByRole("list", { name: "Recent Receipts" });
  const row = within(list).getAllByRole("listitem")[0];
  expect(row.textContent).toMatch(/every Declared Check ran/);
  // A word match cannot tell a claim from its denial, and both this row and the
  // standing copy above the list deny one. So: the only occurrence in the row is
  // the denial, and there is exactly one.
  const upgrades = (row.textContent ?? "").match(/passed|succeeded|success/gi) ?? [];
  expect(upgrades).toHaveLength(1);
  expect(row.textContent).toMatch(/this does not say they passed/);
  expect(screen.getByText(/records that the Declared Checks ran, not that they passed/)).toBeVisible();
});

it("states every count the reading carries, not a bare 'and more'", async () => {
  // The page knows how many Runs it walked, how many recorded no Receipt, how
  // many are still running, and whether older Runs exist. A bool where the page
  // has a count throws away what the daemon measured.
  render(
    <BriefingReceipts
      page={page({
        receipts: [preflight("aaaaaaaa-1111-4111-8111-111111111111", 100, "verified")],
        runWindowApplied: 50,
        runsUnconfirmedInWindow: 3,
        runsInFlightInWindow: 2,
        moreRunsExist: true,
      })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  const reading = screen.getByText(/this reading covered the 50 most recent Runs/);
  expect(reading).toBeVisible();
  expect(reading.textContent).toMatch(/found 1 Receipt/);
  expect(reading.textContent).toMatch(/3 ended without one/);
  expect(reading.textContent).toMatch(/2 are still running/);
  // Runs, not Receipts: whether another RECEIPT exists is not something this
  // reading established, and an older receiptless Run sets the same flag.
  expect(reading.textContent).toMatch(/older Runs exist/);
  expect(reading.textContent).not.toMatch(/more Receipts/);
});

it("omits the counts it does not have rather than printing zeros", async () => {
  render(
    <BriefingReceipts
      page={page({ receipts: [preflight("aaaaaaaa-1111-4111-8111-111111111111", 100, "verified")] })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  const reading = screen.getByText(/this reading covered the 50 most recent Runs/);
  expect(reading.textContent).toMatch(/found 1 Receipt\./);
  expect(reading.textContent).not.toMatch(/ended without one/);
  expect(reading.textContent).not.toMatch(/still running/);
  expect(reading.textContent).not.toMatch(/older Runs exist/);
});

it("does not call an empty window an empty Workspace", async () => {
  // The honest-absence case: the window found no Receipts, and it saw Runs that
  // recorded none plus older Runs beyond it. "No Receipts yet" would be an
  // absolute claim this reading did not earn.
  render(
    <BriefingReceipts
      page={page({ receipts: [], runsUnconfirmedInWindow: 4, moreRunsExist: true })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  expect(screen.getByText("No Receipts among the Runs in this reading.")).toBeVisible();
  expect(screen.queryByText("No Receipts yet.")).toBeNull();
  const reading = screen.getByText(/this reading covered the 50 most recent Runs/);
  expect(reading.textContent).toMatch(/found 0 Receipts/);
  expect(reading.textContent).toMatch(/4 ended without one/);
  expect(reading.textContent).toMatch(/older Runs exist/);
});

it("earns the absolute empty sentence only when the window saw every Run", async () => {
  render(
    <BriefingReceipts
      page={page()}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={vi.fn()}
    />,
  );

  expect(screen.getByText("No Receipts yet.")).toBeVisible();
  expect(screen.queryByText("No Receipts among the Runs in this reading.")).toBeNull();
});

it("keeps loading, empty and failed apart, and stays accessible", async () => {
  const user = userEvent.setup();
  const onRefresh = vi.fn();
  const { container, rerender } = render(
    <BriefingReceipts page={null} loading failure="" disabled={false} onRefresh={onRefresh} />,
  );
  expect(screen.getByText("Loading Receipts")).toBeVisible();
  expect(screen.queryByText("No Receipts yet.")).toBeNull();

  rerender(
    <BriefingReceipts
      page={null}
      loading={false}
      failure="Beacon could not read recent Run and Receipt state."
      disabled={false}
      onRefresh={onRefresh}
    />,
  );
  // A read that failed is not a Workspace with no Receipts.
  expect(screen.getByText(/could not be read/)).toBeVisible();
  expect(screen.queryByText("No Receipts yet.")).toBeNull();

  rerender(
    <BriefingReceipts
      page={page({ receipts: [preflight("aaaaaaaa-1111-4111-8111-111111111111", 100, "verified")] })}
      loading={false}
      failure=""
      disabled={false}
      onRefresh={onRefresh}
    />,
  );
  await user.click(screen.getByRole("button", { name: "Refresh Receipts" }));
  expect(onRefresh).toHaveBeenCalledTimes(1);

  const results = await axe.run(container, { rules: { "color-contrast": { enabled: false } } });
  expect(results.violations).toEqual([]);
});
