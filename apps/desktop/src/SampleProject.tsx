import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";
import { useEffect, useState } from "react";

import { installSampleProject, loadSampleProjectStatus, type SampleProjectStatus } from "./session";

const useStyles = makeStyles({
  card: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    marginBottom: tokens.spacingVerticalXXL,
    ...shorthands.padding(tokens.spacingVerticalL, tokens.spacingHorizontalL),
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
    backgroundColor: tokens.colorNeutralBackground1,
  },
  eyebrow: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    fontWeight: tokens.fontWeightSemibold,
    letterSpacing: "0.08em",
    margin: 0,
    textTransform: "uppercase",
  },
  heading: { fontSize: tokens.fontSizeBase500, margin: 0 },
  body: { color: tokens.colorNeutralForeground2, margin: 0, maxWidth: "62ch" },
  details: {
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
    fontSize: tokens.fontSizeBase200,
    overflowWrap: "anywhere",
    margin: 0,
  },
  error: { color: tokens.colorPaletteRedForeground1, margin: 0 },
  action: { justifySelf: "start", marginTop: tokens.spacingVerticalS },
});

export function SampleProject({ onInstalled }: { onInstalled?: (projectId: string) => void }) {
  const classes = useStyles();
  const [status, setStatus] = useState<SampleProjectStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [resultMessage, setResultMessage] = useState("");

  useEffect(() => {
    let active = true;
    void loadSampleProjectStatus().then(
      (loaded) => {
        if (active) setStatus(loaded);
      },
      () => {
        if (active) setError("Beacon could not verify Sample Project status.");
      },
    );
    return () => {
      active = false;
    };
  }, []);

  async function install() {
    setBusy(true);
    setError("");
    setResultMessage("");
    try {
      const result = await installSampleProject();
      setStatus({ state: "installed", installation: result.installation });
      setResultMessage(
        result.outcome === "installed" ? "Sample Project installed" : "Sample Project was already installed",
      );
      onInstalled?.(result.installation.projectId);
    } catch {
      setError("Beacon refused the Sample Project installation. No Project was published.");
    } finally {
      setBusy(false);
    }
  }

  const installation = status?.state === "installed" ? status.installation : null;
  return (
    <section className={classes.card} aria-labelledby="sample-project-heading">
      <p className={classes.eyebrow}>Optional local proving project</p>
      <h2 id="sample-project-heading" className={classes.heading}>
        Sample Project
      </h2>
      <p className={classes.body}>
        Install Beacon&apos;s private bundled Rust task tracker. Setup starts no Run, model, network request, or
        execution, and it never opens one of your repositories.
      </p>
      {status === null && !error && <p role="status">Checking Sample Project status</p>}
      {status?.state === "available" && (
        <>
          <p className={classes.body}>
            Creates one Supervised Project and two real declared checks. You remain in control of every later action.
          </p>
          <p className={classes.details}>
            Bundle v{status.bundleVersion} · {status.bundleDigest.algorithm} {status.bundleDigest.value}
          </p>
          <Button className={classes.action} appearance="primary" disabled={busy} onClick={() => void install()}>
            {busy ? "Installing Sample Project" : "Install Sample Project"}
          </Button>
        </>
      )}
      {installation && (
        <>
          <p className={classes.body}>Installed as a Supervised Project and two declared checks. No work has run.</p>
          <p className={classes.details}>Revision {installation.baseRevision}</p>
        </>
      )}
      {resultMessage && <p role="status">{resultMessage}</p>}
      {error && (
        <p className={classes.error} role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
