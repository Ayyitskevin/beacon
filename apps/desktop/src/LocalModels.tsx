import { Button, Checkbox, makeStyles, shorthands, tokens } from "@fluentui/react-components";
import { useEffect, useState } from "react";

import type {
  DiscoveredLocalModel,
  LocalModelActivation,
  LocalModelActivationReview,
  LocalModelDiscoveryReport,
  ProviderFeature,
} from "./session";

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
    rowGap: tokens.spacingVerticalS,
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
  badge: {
    ...shorthands.padding("2px", tokens.spacingHorizontalS),
    ...shorthands.borderRadius(tokens.borderRadiusCircular),
    backgroundColor: tokens.colorNeutralBackground4,
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    fontWeight: tokens.fontWeightSemibold,
  },
  modelActions: {
    gridColumn: "1 / -1",
    display: "flex",
    alignItems: "center",
    flexWrap: "wrap",
    gap: tokens.spacingHorizontalS,
  },
  review: {
    gridColumn: "1 / -1",
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
  },
  reviewHeading: {
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
    fontSize: tokens.fontSizeBase400,
  },
  featureList: {
    display: "grid",
    rowGap: tokens.spacingVerticalXS,
    marginTop: tokens.spacingVerticalM,
    marginBottom: tokens.spacingVerticalM,
  },
  technical: {
    gridColumn: "1 / -1",
    color: tokens.colorNeutralForeground2,
  },
  digest: {
    overflowWrap: "anywhere",
    fontFamily: "ui-monospace, SFMono-Regular, Consolas, monospace",
  },
});

export function LocalModels({
  report,
  activations,
  review,
  busy,
  discovering,
  onRefresh,
  onReview,
  onCancelReview,
  onActivate,
  onDeactivate,
}: {
  report: LocalModelDiscoveryReport | null;
  activations: readonly LocalModelActivation[] | null;
  review: LocalModelActivationReview | null;
  busy: boolean;
  discovering: boolean;
  onRefresh: () => void;
  onReview: (modelName: string, expectedDigest: string) => void;
  onCancelReview: () => void;
  onActivate: (
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) => void;
  onDeactivate: (expectedActivationId: string) => void;
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
      <DiscoveryResult
        report={report}
        activations={activations}
        review={review}
        busy={busy}
        discovering={discovering}
        onReview={onReview}
        onCancelReview={onCancelReview}
        onActivate={onActivate}
        onDeactivate={onDeactivate}
      />
    </section>
  );
}

function DiscoveryResult({
  report,
  activations,
  review,
  busy,
  discovering,
  onReview,
  onCancelReview,
  onActivate,
  onDeactivate,
}: {
  report: LocalModelDiscoveryReport | null;
  activations: readonly LocalModelActivation[] | null;
  review: LocalModelActivationReview | null;
  busy: boolean;
  discovering: boolean;
  onReview: (modelName: string, expectedDigest: string) => void;
  onCancelReview: () => void;
  onActivate: (
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) => void;
  onDeactivate: (expectedActivationId: string) => void;
}) {
  const classes = useStyles();
  if (discovering) return <p role="status">Checking local Ollama</p>;
  if (!report) return <p className={classes.status}>Not checked yet</p>;
  if (report.status === "unavailable") {
    return (
      <>
        <p className={classes.status} role="status">
          Ollama is not available at {report.endpoint}. Stored Workspace decisions remain
          revocable.
        </p>
        <StoredActivationList
          activations={activations}
          busy={busy}
          onDeactivate={onDeactivate}
        />
      </>
    );
  }
  if (report.status === "invalid_response") {
    return (
      <>
        <p className={classes.status} role="status">
          Ollama responded, but Beacon could not verify its installed-model list. Stored Workspace
          decisions remain revocable.
        </p>
        <StoredActivationList
          activations={activations}
          busy={busy}
          onDeactivate={onDeactivate}
        />
      </>
    );
  }
  if (report.models.length === 0) {
    return (
      <>
        <p className={classes.status} role="status">
          Ollama is available. No installed models were found.
        </p>
        <StoredActivationList
          activations={activations}
          busy={busy}
          onDeactivate={onDeactivate}
        />
      </>
    );
  }
  return (
    <>
      <p className={classes.status} role="status">
        {report.models.length} installed {report.models.length === 1 ? "model" : "models"} found
      </p>
      <ul className={classes.list} aria-label="Discovered local models">
        {report.models.map((model) => {
          const activation = activations?.find(
            (candidate) =>
              candidate.binding.endpoint === report.endpoint &&
              candidate.binding.modelName === model.name,
          );
          const isActive = activation?.state === "active";
          const matchingReview =
            review?.binding.endpoint === report.endpoint && review.binding.modelName === model.name
              ? review
              : null;
          return (
            <ModelRow
              key={model.name}
              model={model}
              endpoint={report.endpoint}
              activation={activation}
              review={matchingReview}
              isActive={isActive}
              busy={busy}
              onReview={onReview}
              onCancelReview={onCancelReview}
              onActivate={onActivate}
              onDeactivate={onDeactivate}
            />
          );
        })}
      </ul>
      <StoredActivationList
        activations={activations?.filter(
          (activation) =>
            !report.models.some(
              (model) =>
                activation.binding.endpoint === report.endpoint &&
                activation.binding.modelName === model.name,
            ),
        )}
        busy={busy}
        onDeactivate={onDeactivate}
      />
    </>
  );
}

function StoredActivationList({
  activations,
  busy,
  onDeactivate,
}: {
  activations: readonly LocalModelActivation[] | null | undefined;
  busy: boolean;
  onDeactivate: (expectedActivationId: string) => void;
}) {
  const classes = useStyles();
  if (!activations || activations.length === 0) return null;
  return (
    <ul className={classes.list} aria-label="Stored local-model activation decisions">
      {activations.map((activation) => (
        <li className={classes.model} key={activation.id}>
          <div>
            <h3 className={classes.modelName}>{activation.binding.modelName}</h3>
            <p className={classes.modelSummary}>
              Stored decision for {activation.binding.provider}. Provider inspection is not needed
              to revoke it.
            </p>
          </div>
          <span className={classes.badge}>
            {activation.state === "active" ? "Active" : "Inactive"}
          </span>
          {activation.state === "active" && (
            <div className={classes.modelActions}>
              <Button
                size="small"
                disabled={busy}
                onClick={() => onDeactivate(activation.id)}
                aria-label={`Deactivate ${activation.binding.modelName}`}
              >
                Deactivate
              </Button>
              <span className={classes.status}>
                Deactivation is local and does not contact or run the provider.
              </span>
            </div>
          )}
          <details className={classes.technical}>
            <summary>Decision evidence</summary>
            <p>Endpoint: {activation.binding.endpoint}</p>
            <p className={classes.digest}>Model digest: {activation.binding.modelDigest}</p>
            <p className={classes.digest}>
              Inspection digest: {activation.binding.inspectionDigest}
            </p>
          </details>
        </li>
      ))}
    </ul>
  );
}

function ModelRow({
  model,
  endpoint,
  activation,
  review,
  isActive,
  busy,
  onReview,
  onCancelReview,
  onActivate,
  onDeactivate,
}: {
  model: DiscoveredLocalModel;
  endpoint: string;
  activation: LocalModelActivation | undefined;
  review: LocalModelActivationReview | null;
  isActive: boolean;
  busy: boolean;
  onReview: (modelName: string, expectedDigest: string) => void;
  onCancelReview: () => void;
  onActivate: (
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) => void;
  onDeactivate: (expectedActivationId: string) => void;
}) {
  const classes = useStyles();
  const installedModelChanged = isActive && activation?.binding.modelDigest !== model.digest;
  return (
    <li className={classes.model}>
      <div>
        <h3 className={classes.modelName}>{model.name}</h3>
        <p className={classes.modelSummary}>
          {model.details.parameterSize} · {model.details.quantizationLevel} ·{" "}
          {formatBytes(model.sizeBytes)}
        </p>
      </div>
      <span className={classes.badge}>{isActive ? "Active" : "Inactive"}</span>
      <div className={classes.modelActions}>
        {isActive && activation ? (
          <Button
            size="small"
            disabled={busy}
            onClick={() => onDeactivate(activation.id)}
            aria-label={`Deactivate ${model.name}`}
          >
            Deactivate
          </Button>
        ) : (
          <Button
            size="small"
            appearance="primary"
            disabled={busy}
            onClick={() => onReview(model.name, model.digest)}
            aria-label={`Review ${model.name}`}
          >
            Review
          </Button>
        )}
        <span className={classes.status}>
          Activation only makes approved features eligible for a later Route. It does not create a
          Route or run the model.
        </span>
      </div>
      {installedModelChanged && (
        <p className={classes.technical} role="alert">
          The installed digest changed after activation. Deactivate this decision and review the
          current model before using it in a Route.
        </p>
      )}
      {review && (
        <ActivationReview
          review={review}
          busy={busy}
          onCancel={onCancelReview}
          onActivate={onActivate}
        />
      )}
      <details className={classes.technical}>
        <summary>Technical details</summary>
        <p>Provider: Ollama · Format: {model.details.format} · Family: {model.details.family}</p>
        <p>Endpoint: {endpoint}</p>
        <p className={classes.digest}>Digest: {model.digest}</p>
        <p>Modified: {model.modifiedAt}</p>
      </details>
    </li>
  );
}

function ActivationReview({
  review,
  busy,
  onCancel,
  onActivate,
}: {
  review: LocalModelActivationReview;
  busy: boolean;
  onCancel: () => void;
  onActivate: (
    review: LocalModelActivationReview,
    approvedFeatures: readonly ProviderFeature[],
  ) => void;
}) {
  const classes = useStyles();
  const [approved, setApproved] = useState<ProviderFeature[]>([...review.providerFeatures]);
  useEffect(() => setApproved([...review.providerFeatures]), [review]);

  return (
    <section className={classes.review} aria-labelledby="local-model-review-heading">
      <h4 id="local-model-review-heading" className={classes.reviewHeading}>
        Review {review.binding.modelName}
      </h4>
      <p className={classes.description}>
        These features are advertised by Ollama, not Beacon permissions. Choose only what may be
        considered when an Operator creates a later Route.
      </p>
      <div className={classes.featureList}>
        {review.providerFeatures.map((feature) => (
          <Checkbox
            key={feature}
            label={`Approve ${feature}`}
            checked={approved.includes(feature)}
            onChange={(_, data) =>
              setApproved((current) =>
                data.checked
                  ? [...current, feature].sort()
                  : current.filter((candidate) => candidate !== feature),
              )
            }
          />
        ))}
      </div>
      {review.unrecognizedFeatures.map((feature) => (
        <p className={classes.status} key={feature}>
          {feature} is advertised but is not recognized by Beacon and cannot be approved.
        </p>
      ))}
      <p className={classes.status}>
        Activation does not grant an Agent capability, create a Route, or run the model.
      </p>
      <details className={classes.technical}>
        <summary>Review evidence</summary>
        <p className={classes.digest}>Model digest: {review.binding.modelDigest}</p>
        <p className={classes.digest}>Inspection digest: {review.binding.inspectionDigest}</p>
      </details>
      <div className={classes.modelActions}>
        <Button
          appearance="primary"
          disabled={busy || approved.length === 0}
          onClick={() => onActivate(review, approved)}
        >
          Activate for this Workspace
        </Button>
        <Button disabled={busy} onClick={onCancel}>
          Cancel review
        </Button>
      </div>
    </section>
  );
}

function formatBytes(value: number): string {
  if (value < 1_000_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  return `${(value / 1_000_000_000).toFixed(1)} GB`;
}
