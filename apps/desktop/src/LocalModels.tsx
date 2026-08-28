import { Button, makeStyles, shorthands, tokens } from "@fluentui/react-components";

import type { LocalModelDiscoveryReport } from "./session";

const useStyles = makeStyles({
  panel: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.padding(tokens.spacingVerticalL, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  header: {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    columnGap: tokens.spacingHorizontalL,
  },
  heading: {
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
    fontSize: tokens.fontSizeBase500,
  },
  description: {
    marginTop: 0,
    color: tokens.colorNeutralForeground2,
  },
  status: {
    color: tokens.colorNeutralForeground2,
  },
  list: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    paddingLeft: 0,
    listStyleType: "none",
  },
  model: {
    display: "grid",
    gridTemplateColumns: "minmax(0, 1fr) auto",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground3,
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  modelName: {
    margin: 0,
    fontSize: tokens.fontSizeBase400,
    overflowWrap: "anywhere",
  },
  modelSummary: {
    marginTop: tokens.spacingVerticalXS,
    marginBottom: 0,
    color: tokens.colorNeutralForeground2,
  },
  inactive: {
    ...shorthands.padding("2px", tokens.spacingHorizontalS),
    ...shorthands.borderRadius(tokens.borderRadiusCircular),
    backgroundColor: tokens.colorNeutralBackground4,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    fontWeight: tokens.fontWeightSemibold,
  },
  technical: {
    gridColumn: "1 / -1",
    marginTop: tokens.spacingVerticalS,
    color: tokens.colorNeutralForeground2,
  },
  digest: {
    overflowWrap: "anywhere",
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
  },
});

export function LocalModels({
  report,
  busy,
  onRefresh,
}: {
  report: LocalModelDiscoveryReport | null;
  busy: boolean;
  onRefresh: () => void;
}) {
  const classes = useStyles();
  return (
    <section className={classes.panel} aria-labelledby="local-models-heading">
      <div className={classes.header}>
        <div>
          <h2 id="local-models-heading" className={classes.heading}>
            Local models
          </h2>
          <p className={classes.description}>
            Beacon checks this computer for Ollama models. Discovery does not activate or run them.
          </p>
        </div>
        <Button size="small" disabled={busy} onClick={onRefresh}>
          {report ? "Refresh" : "Check local models"}
        </Button>
      </div>
      <DiscoveryResult report={report} busy={busy} />
    </section>
  );
}

function DiscoveryResult({
  report,
  busy,
}: {
  report: LocalModelDiscoveryReport | null;
  busy: boolean;
}) {
  const classes = useStyles();
  if (busy) return <p role="status">Checking local Ollama</p>;
  if (!report) return <p className={classes.status}>Not checked yet</p>;
  if (report.status === "unavailable") {
    return (
      <p className={classes.status} role="status">
        Ollama is not available at {report.endpoint}. Beacon continues without it.
      </p>
    );
  }
  if (report.status === "invalid_response") {
    return (
      <p className={classes.status} role="status">
        Ollama responded, but Beacon could not verify its installed-model list.
      </p>
    );
  }
  if (report.models.length === 0) {
    return (
      <p className={classes.status} role="status">
        Ollama is available. No installed models were found.
      </p>
    );
  }
  return (
    <>
      <p className={classes.status} role="status">
        {report.models.length} installed {report.models.length === 1 ? "model" : "models"} found
      </p>
      <ul className={classes.list} aria-label="Discovered local models">
        {report.models.map((model) => (
          <li className={classes.model} key={model.name}>
            <div>
              <h3 className={classes.modelName}>{model.name}</h3>
              <p className={classes.modelSummary}>
                {model.details.parameterSize} · {model.details.quantizationLevel} · {formatBytes(model.sizeBytes)}
              </p>
            </div>
            <span className={classes.inactive}>Inactive</span>
            <details className={classes.technical}>
              <summary>Technical details</summary>
              <p>Provider: Ollama · Format: {model.details.format} · Family: {model.details.family}</p>
              <p>Endpoint: {report.endpoint}</p>
              <p className={classes.digest}>Digest: {model.digest}</p>
              <p>Modified: {model.modifiedAt}</p>
            </details>
          </li>
        ))}
      </ul>
    </>
  );
}

function formatBytes(value: number): string {
  if (value < 1_000_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  return `${(value / 1_000_000_000).toFixed(1)} GB`;
}
