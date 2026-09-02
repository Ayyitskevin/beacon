import { useState } from "react";
import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import type { LiveDataEncryptionReport, Recorded } from "./session";
import { detectGuidePlatform, encryptionGuide } from "./liveDataEncryptionGuide";

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalL },
  heading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  warningHeading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
    color: tokens.colorPaletteDarkOrangeForeground1,
  },
  warning: {
    ...shorthands.borderLeft(
      tokens.strokeWidthThicker,
      "solid",
      tokens.colorPaletteDarkOrangeBorderActive,
    ),
    ...shorthands.padding(tokens.spacingVerticalXS, tokens.spacingHorizontalS),
  },
  body: { margin: 0, color: tokens.colorNeutralForeground2, fontSize: tokens.fontSizeBase200 },
  status: { margin: 0, color: tokens.colorNeutralForeground2, fontSize: tokens.fontSizeBase200 },
  actions: { display: "flex", gap: tokens.spacingHorizontalS, marginTop: tokens.spacingVerticalS },
  detail: { marginTop: tokens.spacingVerticalS },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
    margin: 0,
  },
  list: {
    margin: 0,
    marginTop: tokens.spacingVerticalXS,
    paddingLeft: tokens.spacingHorizontalXL,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
  },
  evidence: { fontFamily: tokens.fontFamilyMonospace, wordBreak: "break-all" },
});

function present(value: Recorded<string>): string | null {
  return value.kind === "present" ? value.value : null;
}

function absent(value: Recorded<string>): string | null {
  return value.kind === "absent" ? value.reason : null;
}

/** The Installation's live-data encryption diagnostic.
 *
 *  This is not an Attention Item: the actor is the operating system, not the
 *  Operator or a named Agent, and an object that may raise an operating-system
 *  notification cannot also be the unobtrusive warning ADR 0017 asks for. It is
 *  a status line in the Installation inspector, quietly visible until the
 *  condition changes, and it offers no way to dismiss it.
 */
export function LiveDataEncryptionStatus({
  report,
  busy,
  onRecheck,
}: {
  report: LiveDataEncryptionReport | null;
  busy: boolean;
  onRecheck: () => void;
}) {
  const styles = useStyles();
  const [showGuide, setShowGuide] = useState(false);
  const [showEvidence, setShowEvidence] = useState(false);
  if (!report) return null;

  const guide = encryptionGuide(detectGuidePlatform(navigator.userAgent));
  const mechanism = present(report.mechanism);
  const reason = absent(report.mechanism);
  const mountPoint = present(report.mountPoint);
  const warned = report.state !== "enabled";

  return (
    <section className={styles.section} aria-labelledby="live-data-encryption-heading">
      <div className={warned ? styles.warning : undefined}>
        <h3
          id="live-data-encryption-heading"
          className={warned ? styles.warningHeading : styles.heading}
        >
          Live data
        </h3>
        <div role="status">
          {report.state === "enabled" ? (
            <p className={styles.status}>
              Operating-system disk encryption detected on the volume holding live data
              {mechanism ? ` — ${mechanism}` : ""}. This is your operating system's encryption;
              Beacon does not encrypt the open live store.
            </p>
          ) : report.state === "disabled" ? (
            // Not "found no encryption": a volume can have one encrypted member
            // and one plain one, and the reading says which. Denying what the
            // same report shows in its evidence teaches the operator to
            // disbelieve the warning.
            <p className={styles.body}>
              <strong>Live data is not fully protected by disk encryption.</strong> At least one
              path to the disk holding this Installation's live data has no operating-system disk
              encryption
              {reason ? `: ${reason}` : ""}. Beacon does not encrypt the open live store itself.
              Turn on your operating system's disk encryption to protect this data at rest.
            </p>
          ) : (
            <p className={styles.body}>
              <strong>Disk-encryption status is unknown.</strong> Beacon could not determine
              whether the volume holding this Installation's live data is encrypted by the
              operating system, so it does not claim that it is
              {reason ? `: ${reason}` : ""}.
            </p>
          )}
        </div>

        {warned ? (
          <div className={styles.actions}>
            <Button
              size="small"
              appearance="secondary"
              aria-expanded={showGuide}
              onClick={() => setShowGuide(!showGuide)}
            >
              How to turn this on
            </Button>
            <Button size="small" appearance="secondary" disabled={busy} onClick={onRecheck}>
              {busy ? "Rechecking…" : "Recheck"}
            </Button>
          </div>
        ) : null}
      </div>

      {showGuide ? (
        <div className={styles.detail}>
          <p className={styles.meta}>{guide.title}</p>
          <ul className={styles.list}>
            {guide.steps.map((step) => (
              <li key={step}>{step}</li>
            ))}
          </ul>
          <p className={styles.body}>{guide.caution}</p>
          <p className={styles.body}>{guide.documentation}</p>
        </div>
      ) : null}

      <div className={styles.detail}>
        <Button
          size="small"
          appearance="subtle"
          aria-expanded={showEvidence}
          onClick={() => setShowEvidence(!showEvidence)}
        >
          What Beacon read
        </Button>
        {showEvidence ? (
          <>
            <p className={styles.body}>
              Volume: {mountPoint ?? "not identified"}
            </p>
            <ul className={styles.list}>
              {report.evidence.map((entry) => (
                <li key={entry.source} className={styles.evidence}>
                  {entry.source} — {entry.observation}
                </li>
              ))}
            </ul>
            <p className={styles.meta}>Not verified</p>
            <ul className={styles.list}>
              {report.notVerified.map((entry) => (
                <li key={entry}>{entry}</li>
              ))}
            </ul>
          </>
        ) : null}
      </div>
    </section>
  );
}
