import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import { preflightOutcomeCopy } from "./preflightOutcomes";
import { MAX_PROJECT_RECEIPT_PAGE, type PreflightReceipt } from "./session";

/** How many rows Briefing draws. It "optimizes for exceptions, not activity
 *  volume" (BLUEPRINT §3), and the Query it reads is unbounded, so the bound
 *  is a decision made here and stated to the operator when it bites. */
export const RECENT_RECEIPT_LIMIT = 5;

/** How many Projects one reading may open, and how many rows it may hold
 *  before sorting.
 *
 *  The Query is Project-scoped, so Briefing must fan out; an unbounded fan-out
 *  is not a design but an assumption about the Workspace. These are small,
 *  named, and disclosed when they bite -- a stopgap until a Workspace-scoped,
 *  server-bounded Query exists behind the interface.
 */
export const RECEIPT_PROJECT_BUDGET = 8;
export const RECEIPT_AGGREGATE_LIMIT = 256;

/** The most rows one reading may hold at once: the cap it keeps, plus the one
 *  page it is merging. Named so a test can assert the bound rather than trust
 *  the prose that states it. */
export const RECEIPT_HOLD_LIMIT = RECEIPT_AGGREGATE_LIMIT + MAX_PROJECT_RECEIPT_PAGE;

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


export type ReceiptRow = { receipt: PreflightReceipt; projectName: string };

/** Newest first, by the same key the daemon orders each Project's Receipts on,
 *  extended across a merge of several Projects so the order is stable. */
export function orderReceipts(rows: readonly ReceiptRow[]): ReceiptRow[] {
  return [...rows].sort((left, right) => {
    if (left.receipt.startedAtMs !== right.receipt.startedAtMs) {
      return right.receipt.startedAtMs - left.receipt.startedAtMs;
    }
    return right.receipt.runId.localeCompare(left.receipt.runId);
  });
}

/** What one reading gathered, and what it cost to gather it. */
export type ReceiptCollection = {
  /** Newest first, never more than the aggregate cap. */
  rows: ReceiptRow[];
  /** Every row the reading saw -- not the number kept. The panel's "of N" is a
   *  count of what exists, and a capped count there is a quiet completeness
   *  claim. */
  found: number;
  /** The most rows this reading ever held at once.
   *
   *  It is returned because the bound cannot be seen any other way: a rendered
   *  list of five looks identical whether the reading held 768 rows or 4,096,
   *  which is how the previous guard survived a gather-every-page-then-slice
   *  implementation. The guard reads this; the interface does not. */
  peakHeld: number;
};

/** Merges each Project's page into a running top-N and throws the rest away.
 *
 *  ONE Project at a time. A concurrent window is not free here: a page in
 *  flight is a page held, so four concurrent reads of a 512-row page held
 *  2,048 rows beside the 256 kept -- three times the bound this function
 *  documents, and the reason round three held. The daemon dispatches on a
 *  single accept loop, so the window bought little to begin with.
 *
 *  What it holds never exceeds `RECEIPT_HOLD_LIMIT`, whatever the Workspace
 *  contains. Trimming only after the loop retained every row first, and then
 *  reported the capped length as the total.
 */
export async function collectRecentReceipts(
  projects: readonly { id: string; name: string }[],
  readPage: (projectId: string) => Promise<readonly PreflightReceipt[]>,
  limit: number = RECEIPT_AGGREGATE_LIMIT,
): Promise<ReceiptCollection> {
  let rows: ReceiptRow[] = [];
  let found = 0;
  let peakHeld = 0;
  for (const project of projects) {
    const page = (await readPage(project.id)).map((receipt) => ({
      receipt,
      projectName: project.name,
    }));
    found += page.length;
    // Measured before the merge, which is where the peak actually is: the
    // merge builds one array of exactly this length before trimming it.
    peakHeld = Math.max(peakHeld, rows.length + page.length);
    rows = orderReceipts([...rows, ...page]).slice(0, limit);
  }
  return { rows, found, peakHeld };
}

function shortRunId(runId: string): string {
  return runId.slice(0, 8);
}

/** Briefing's recent PREFLIGHT Receipts.
 *
 *  This is not yet BLUEPRINT §3's general "recent receipts" item. The Query it
 *  reads filters `runs.kind = 'check_preflight'`, and Beacon already writes
 *  execution Receipts for declared-check Runs into the same store. The general,
 *  cross-kind list needs a Workspace-scoped Query behind the interface; see
 *  `docs/implementation/phase-1-briefing-recent-receipts.md`.
 *
 *  Every row states the kind of Run, the outcome exactly as recorded, the Run
 *  it belongs to, and when it started. It never upgrades an outcome: nothing
 *  here says passed, completed, or succeeded, because a Receipt that does not
 *  say so is not evidence that it happened.
 */
export function BriefingReceipts({
  rows,
  total,
  loading,
  failure,
  scope,
  disabled,
  onRefresh,
}: {
  rows: readonly ReceiptRow[] | null;
  total: number;
  loading: boolean;
  failure: string;
  /** Empty when the reading covered every Project; otherwise what it covered. */
  scope: string;
  /** Unrelated work in flight. Kept apart from `loading` so retiring a Resident
   *  does not label this button "Refreshing…" while no Receipt read is running. */
  disabled: boolean;
  onRefresh: () => void;
}) {
  const styles = useStyles();
  const shown = rows ? orderReceipts(rows).slice(0, RECENT_RECEIPT_LIMIT) : [];
  const hidden = Math.max(0, total - shown.length);

  return (
    <section className={styles.section} aria-labelledby="briefing-receipts-heading">
      <h2 id="briefing-receipts-heading" className={styles.heading}>
        Recent preflight Receipts
      </h2>
      <p className={styles.body}>
        An immutable account of what each preflight Run did. Preflight establishes whether
        execution would be admissible; it runs no check. This list does not include execution
        Receipts, which Beacon also writes.
      </p>

      {failure ? (
        <p className={styles.body} role="status">
          Recent preflight Receipts could not be read. {failure}
        </p>
      ) : loading ? (
        <p className={styles.body} role="status">
          Loading preflight Receipts
        </p>
      ) : shown.length === 0 ? (
        <p className={styles.body} role="status">
          No preflight Receipts yet.
        </p>
      ) : (
        <>
          <ul className={styles.list} aria-label="Recent preflight Receipts">
            {shown.map(({ receipt, projectName }) => (
              <li key={receipt.runId} className={styles.row}>
                <span>
                  <p className={styles.outcome}>Check preflight — {preflightOutcomeCopy[receipt.outcome].summary}</p>
                  <p className={styles.detail}>
                    {projectName} · {new Date(receipt.startedAtMs).toISOString()}
                  </p>
                </span>
                <span className={styles.identity} title={receipt.runId}>
                  Run {shortRunId(receipt.runId)}
                </span>
              </li>
            ))}
          </ul>
          {hidden > 0 ? (
            <p className={styles.truncation}>
              Showing the {shown.length} most recent of {total}. Beacon has no full preflight
              history view yet, so the rest are recorded but not readable here.
            </p>
          ) : null}
        </>
      )}

      {scope ? <p className={styles.truncation}>{scope}.</p> : null}

      <div className={styles.actions}>
        <Button size="small" disabled={disabled || loading} onClick={onRefresh}>
          {loading ? "Refreshing…" : "Refresh preflight Receipts"}
        </Button>
      </div>
    </section>
  );
}
