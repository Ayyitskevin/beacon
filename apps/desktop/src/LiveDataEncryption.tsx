import { useState } from "react";
import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import { renderConclusion, renderMechanisms } from "./liveDataEncryptionSentence";
import type { LiveDataEncryptionReport } from "./session";
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

  // A reading that has not happened yet renders nothing. A reading that FAILED
  // arrives as a report whose reason is `initial_read_failed`, so the failure
  // and the retry go through the same shape and the same renderer as every
  // other answer -- there is no second surface to keep honest.
  if (!report) return null;

  const guide = encryptionGuide(detectGuidePlatform(navigator.userAgent));
  // Rendered from the closed conclusion, never read as text off the wire.
  const mechanism = renderMechanisms(report.conclusion);
  const reason = renderConclusion(report.conclusion);
  const mountPoint = report.mountPoint
    ? `${report.mountPoint.value}${report.mountPoint.shortened ? "…" : ""}`
    : null;
  const warned = report.conclusion.state !== "enabled";
  const failedRead = report.conclusion.reason === "initial_read_failed";

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
          {report.conclusion.state === "enabled" ? (
            <p className={styles.status}>
              Operating-system disk encryption detected on the volume holding live data
              {mechanism ? ` — ${mechanism}` : ""}. This is your operating system's encryption;
              Beacon does not encrypt the open live store.
            </p>
          ) : report.conclusion.state === "disabled" ? (
            // Not "found no encryption": a volume can have one encrypted member
            // and one plain one, and the reading says which. Denying what the
            // same report shows in its evidence teaches the operator to
            // disbelieve the warning.
            <p className={styles.body}>
              <strong>Live data is not fully protected by disk encryption.</strong> At least one
              path to the disk holding this Installation's live data has no operating-system disk
              encryption: {reason}. Beacon does not encrypt the open live store itself. Turn on
              your operating system's disk encryption to protect this data at rest.
            </p>
          ) : failedRead ? (
            // The diagnostic itself did not run. Named apart from an inconclusive
            // reading, because "Beacon could not determine" reads as an answer
            // about the volume and this is an answer about the reading.
            <p className={styles.body}>
              <strong>Disk-encryption status could not be read.</strong> {reason}. Beacon does not
              claim this Installation's live data is encrypted.
            </p>
          ) : (
            <p className={styles.body}>
              <strong>Disk-encryption status is unknown.</strong> Beacon could not determine
              whether the volume holding this Installation's live data is encrypted by the
              operating system, so it does not claim that it is: {reason}.
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
                  {entry.truncated ? " …(shortened)" : ""}
                </li>
              ))}
              {report.omittedEvidence > 0 ? (
                <li className={styles.evidence}>
                  …and {report.omittedEvidence} further reading
                  {report.omittedEvidence === 1 ? "" : "s"} this report did not carry
                </li>
              ) : null}
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
