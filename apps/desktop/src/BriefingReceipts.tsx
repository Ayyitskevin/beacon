import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import { executionSummaryCopy, preflightOutcomeCopy } from "./preflightOutcomes";
import type { RecentReceipt, RecentReceiptPage } from "./session";

/** How many rows Briefing draws. It "optimizes for exceptions, not activity
 *  volume" (BLUEPRINT §3). This is a presentation choice on top of a reading
 *  the daemon already bounded, and the line below says how many the reading
 *  actually held. */
export const RECENT_RECEIPT_LIMIT = 5;

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalXL },
  heading: {
    fontSize: tokens.fontSizeBase400,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  body: { color: tokens.colorNeutralForeground2, maxWidth: "62ch", margin: 0 },
  list: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    marginTop: tokens.spacingVerticalM,
    marginBottom: 0,
    paddingLeft: 0,
    listStyleType: "none",
  },
  row: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr) auto",
    columnGap: tokens.spacingHorizontalM,
    ...shorthands.padding(tokens.spacingVerticalS, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  outcome: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  detail: {
    margin: 0,
    marginTop: tokens.spacingVerticalXXS,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
  },
  identity: {
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    color: tokens.colorNeutralForeground2,
    whiteSpace: "nowrap",
  },
  truncation: {
    margin: 0,
    marginTop: tokens.spacingVerticalS,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
  },
  actions: { marginTop: tokens.spacingVerticalM },
});

function shortRunId(runId: string): string {
  return runId.slice(0, 8);
}

/** One vocabulary per kind, chosen by the tag the daemon sent. A single line
 *  for both kinds would have to invent words neither map owns. */
function outcomeLine(receipt: RecentReceipt): string {
  return receipt.outcome.kind === "check_preflight"
    ? `Check preflight — ${preflightOutcomeCopy[receipt.outcome.outcome].summary}`
    : `Declared checks — ${executionSummaryCopy[receipt.outcome.summary].summary}`;
}

/** The second line of defence on a time the interface cannot render.
 *
 *  The adapter is the primary guard and refuses a page carrying one, so this
 *  fallback is not reachable through the Query. It exists because the failure
 *  mode is not a wrong string: `toISOString` RAISES on a value outside the
 *  ECMAScript Date range, and an uncaught throw inside render removes the panel
 *  rather than degrading it -- App's try/catch is around the fetch. A surface
 *  whose whole job is to stay quietly visible must not be one bad number away
 *  from disappearing, and it must say which fact it lost rather than printing a
 *  plausible wrong one. */
function renderInstant(value: number): string {
  return Number.isFinite(value) && Math.abs(value) <= 8.64e15
    ? new Date(value).toISOString()
    : "time not renderable";
}

/** Runs in the window that do not produce a visible Receipt row. */
function windowRuns(page: RecentReceiptPage): number {
  return (
    page.runsInFlightInWindow +
    page.runsUnconfirmedInWindow +
    page.runsVerifiedExcludedInWindow
  );
}

/** What the reading covered, in the daemon's own terms.
 *
 *  Every number here is a number the page carries. The window is over RUNS, so
 *  "more" is about Runs and never claims another Receipt exists -- an older Run
 *  that recorded none sets the same flag. The two receiptless counts are stated
 *  separately because recorded-open and unconfirmed are two conditions, and
 *  because a bool where the page has a count throws away what it knows. */
function describeReading(page: RecentReceiptPage): string {
  const receipts = `${page.receipts.length} ${page.receipts.length === 1 ? "Receipt" : "Receipts"}`;
  const clauses = [
    `this reading covered the ${page.runWindowApplied} most recent Runs and found ${receipts}`,
  ];
  if (page.runsUnconfirmedInWindow > 0) {
    clauses.push(
      `${page.runsUnconfirmedInWindow} ended without one`,
    );
  }
  if (page.runsInFlightInWindow > 0) {
    clauses.push(`${page.runsInFlightInWindow} recorded as open`);
  }
  if (page.runsVerifiedExcludedInWindow > 0) {
    clauses.push(
      page.runsVerifiedExcludedInWindow === 1
        ? "1 recent Run is verified but not shown in this view"
        : `${page.runsVerifiedExcludedInWindow} recent Runs are verified but not shown in this view`,
    );
  }
  // Runs, not Receipts. Whether an older Receipt exists is not something this
  // reading established.
  if (page.moreRunsExist) {
    clauses.push("older Runs exist");
  }
  return `${clauses.join("; ")}.`;
}

/** Briefing's recent Receipts — BLUEPRINT:171's item, across every Run kind.
 *
 *  One Workspace-scoped Query answers this now. The Project-scoped fan-out this
 *  replaced made Briefing choose which Projects to open, in what order, how
 *  many rows to hold and how to merge them; this component renders what one
 *  reading answered and adds nothing to it -- no sort, no cap, no count of its
 *  own. See `docs/implementation/phase-1-recent-receipts-query.md`.
 *
 *  Every row states the kind of Run, the outcome exactly as recorded, the Run
 *  it belongs to, and when it started. It never upgrades an outcome: nothing
 *  here says passed, completed, or succeeded, because a Receipt that does not
 *  say so is not evidence that it happened -- and ADR 0031 is explicit that a
 *  `completed` execution records that the checks RAN.
 */
export function BriefingReceipts({
  page,
  loading,
  failure,
  disabled,
  onRefresh,
  onOpenProject,
}: {
  /** One bounded reading, as the daemon answered it. Null before the first
   *  read and after one that failed. */
  page: RecentReceiptPage | null;
  loading: boolean;
  failure: string;
  /** Unrelated work in flight. Kept apart from `loading` so retiring a Resident
   *  does not label this button "Refreshing…" while no Receipt read is running. */
  disabled: boolean;
  onRefresh: () => void;
  /** Open the Project a Receipt names. The caller decides what "open" means; this panel only
   *  hands over the id the Receipt already holds and never synthesises a link or a name lookup. */
  onOpenProject: (projectId: string) => void;
}) {
  const styles = useStyles();
  // No sort here. The daemon defines the order and the adapter refuses a page
  // that is not in it; re-establishing it would put the ordering policy back
  // in the interface, which is what this Query exists to remove.
  const shown = page ? page.receipts.slice(0, RECENT_RECEIPT_LIMIT) : [];
  const hidden = page ? Math.max(0, page.receipts.length - shown.length) : 0;

  return (
    <section className={styles.section} aria-labelledby="briefing-receipts-heading">
      <h2 id="briefing-receipts-heading" className={styles.heading}>
        Recent Receipts
      </h2>
      <p className={styles.body}>
        An immutable account of what each Run did. Preflight establishes whether execution would be
        admissible and runs no check; an execution Receipt records that the Declared Checks ran, not
        that they passed.
      </p>

      {failure ? (
        <p className={styles.body} role="status">
          Recent Receipts could not be read. {failure}
        </p>
      ) : loading ? (
        <p className={styles.body} role="status">
          Loading Receipts
        </p>
      ) : !page ? null : shown.length === 0 ? (
        <p className={styles.body} role="status">
          {/* The window is over RUNS, so an empty list is not an empty
              Workspace: Runs in this window may have recorded no Receipt, and
              older Runs may exist beyond it. Only a reading that saw every Run
              and found no Receipt has earned the absolute sentence. */}
          {page.moreRunsExist || windowRuns(page) > 0
            ? "No Receipts among the Runs in this reading."
            : "No Receipts yet."}
        </p>
      ) : (
        <>
          <ul className={styles.list} aria-label="Recent Receipts">
            {shown.map((receipt) => (
              <li key={receipt.runId} className={styles.row}>
                <span>
                  <p className={styles.outcome}>{outcomeLine(receipt)}</p>
                  <p className={styles.detail}>
                    {receipt.projectName} · {renderInstant(receipt.startedAtMs)}
                  </p>
                </span>
                <span className={styles.identity} title={receipt.runId}>
                  Run {shortRunId(receipt.runId)}
                </span>
                {/* The Project is identified to the caller by the id the Receipt carries, never by
                    the name shown beside it: two Projects may share a display name, and a control
                    that navigated by name would be undetectably wrong for one of them. The
                    accessible name still says which Project, because a row of identical "Open"
                    buttons tells a screen reader nothing. */}
                <Button
                  size="small"
                  disabled={disabled}
                  onClick={() => onOpenProject(receipt.projectId)}
                >
                  Open {receipt.projectName} Project
                </Button>
              </li>
            ))}
          </ul>
          {hidden > 0 ? (
            <p className={styles.truncation}>
              Showing the {shown.length} most recent of {page.receipts.length} in this reading.
              Beacon has no full Receipt history view yet, so the rest are recorded but not readable
              here.
            </p>
          ) : null}
        </>
      )}

      {page && !failure && !loading ? (
        <p className={styles.truncation}>{describeReading(page)}</p>
      ) : null}

      <div className={styles.actions}>
        <Button size="small" disabled={disabled || loading} onClick={onRefresh}>
          {loading ? "Refreshing…" : "Refresh Receipts"}
        </Button>
      </div>
    </section>
  );
}
