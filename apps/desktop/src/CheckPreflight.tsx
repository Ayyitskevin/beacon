import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import type { PreflightOutcome, PreflightReceipt, Recorded } from "./session";

const useStyles = makeStyles({
  section: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    paddingTop: tokens.spacingVerticalL,
  },
  header: { display: "flex", justifyContent: "space-between", alignItems: "baseline" },
  heading: { fontSize: tokens.fontSizeBase400, marginTop: 0, marginBottom: tokens.spacingVerticalS },
  body: { color: tokens.colorNeutralForeground2, maxWidth: "62ch" },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  outcome: {
    marginTop: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    ...shorthands.borderLeft(tokens.strokeWidthThicker, "solid", tokens.colorPaletteDarkOrangeBorderActive),
    backgroundColor: tokens.colorNeutralBackground2,
  },
  outcomeHeading: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  outcomeDetail: { margin: 0, marginTop: tokens.spacingVerticalXS, color: tokens.colorNeutralForeground2 },
  /* The residual-uncertainty list is the point of a preflight Receipt, so it
     sits above the resolved commands rather than below them. */
  notVerified: { marginTop: tokens.spacingVerticalL },
  notVerifiedList: { margin: 0, paddingLeft: tokens.spacingHorizontalXL, color: tokens.colorNeutralForeground1 },
  checks: { marginTop: tokens.spacingVerticalL },
  checkRow: {
    ...shorthands.padding(tokens.spacingVerticalM, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  command: {
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    wordBreak: "break-all",
  },
  absent: { color: tokens.colorNeutralForeground3, fontStyle: "italic" },
});

/** What each outcome means to the operator. Preflight never runs a check, so
 *  none of these is a check result. */
const outcomeCopy: Record<PreflightOutcome, { title: string; detail: string }> = {
  verified: {
    title: "Execution would be admissible",
    detail: "Every step held and a sandbox is available. This is not a passing check.",
  },
  sandbox_unavailable: {
    title: "Refused — no sandbox",
    detail:
      "Every verification step held. Execution is refused because Beacon has no sandbox capability yet.",
  },
  source_kind_unsupported: {
    title: "Refused — Git URL",
    detail:
      "Confirming a pinned revision in a remote needs a network fetch the operating envelope denies. Bind a local path to preflight it.",
  },
  binding_stale_for_charter: {
    title: "Refused — binding is stale",
    detail: "The Charter was republished after this binding. Rebind before preflighting.",
  },
  repository_unreadable: {
    title: "Refused — repository unreadable",
    detail: "The declared source is missing, unreadable, or not a Git repository.",
  },
  base_revision_unreachable: {
    title: "Refused — base revision absent",
    detail: "The pinned commit is not present in that repository's object store.",
  },
  declared_checks_changed: {
    title: "Refused — the binding moved",
    detail: "The Repository Binding changed while this preflight was inspecting it.",
  },
  working_directory_escapes: {
    title: "Refused — working directory escapes",
    detail: "A check's resolved working directory lies outside the repository root.",
  },
  preflight_failed: {
    title: "Refused — a step errored",
    detail: "A verification step could not complete. This is not the same as a negative answer.",
  },
  preflight_timed_out: {
    title: "Refused — a step did not finish",
    detail: "A verification step ran out of budget, so its answer is unknown rather than negative.",
  },
};

function RecordedValue({ value }: { value: Recorded<string> }) {
  const styles = useStyles();
  if (value.kind === "present") return <span className={styles.command}>{value.value}</span>;
  return <span className={styles.absent}>Not available — {value.reason}</span>;
}

export function CheckPreflightSection({
  receipt,
  busy,
  canPreflight,
  onPreflight,
}: {
  receipt: PreflightReceipt | null;
  busy: boolean;
  canPreflight: boolean;
  onPreflight: () => void;
}) {
  const styles = useStyles();
  const copy = receipt ? outcomeCopy[receipt.outcome] : null;

  return (
    <section className={styles.section} aria-labelledby="check-preflight-heading">
      <div className={styles.header}>
        <h2 id="check-preflight-heading" className={styles.heading}>
          Check preflight
        </h2>
        <Button appearance="secondary" disabled={!canPreflight || busy} onClick={onPreflight}>
          {busy ? "Preflighting…" : "Preflight checks"}
        </Button>
      </div>
      <p className={styles.body}>
        Preflight establishes whether running this Project's Declared Checks would be admissible. It
        reads the repository and refuses to execute anything.
      </p>

      {!canPreflight ? (
        <p className={styles.body}>Bind a repository to this Project to preflight its checks.</p>
      ) : null}

      {receipt && copy ? (
        <>
          <div className={styles.outcome}>
            <p className={styles.outcomeHeading}>{copy.title}</p>
            <p className={styles.outcomeDetail}>{copy.detail}</p>
          </div>

          <div className={styles.notVerified}>
            <p className={styles.meta}>Not verified</p>
            <ul className={styles.notVerifiedList}>
              {receipt.notVerified.map((entry) => (
                <li key={entry}>{entry}</li>
              ))}
            </ul>
          </div>

          <div className={styles.checks}>
            <p className={styles.meta}>
              Commands that would run · sandbox {receipt.sandboxVersion} · repository revision{" "}
              {receipt.repositoryRevision}
            </p>
            {receipt.resolvedChecks.map((check) => (
              <div key={check.name} className={styles.checkRow}>
                <p className={styles.outcomeHeading}>{check.name}</p>
                <p className={styles.command}>{check.argv.join(" ")}</p>
                <p className={styles.outcomeDetail}>
                  Working directory: <RecordedValue value={check.resolvedWorkingDirectory} />
                </p>
              </div>
            ))}
          </div>
        </>
      ) : null}
    </section>
  );
}
