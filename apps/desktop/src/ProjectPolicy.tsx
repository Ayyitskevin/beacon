import {
  Button,
  Checkbox,
  Field,
  Input,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useState } from "react";

import type {
  BeaconCapability,
  ProjectPolicy,
  ProjectPolicyDraft,
  TrackRecordEvidenceKind,
} from "./session";

const useStyles = makeStyles({
  section: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    paddingTop: tokens.spacingVerticalL,
  },
  header: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalL,
    rowGap: tokens.spacingVerticalM,
    flexWrap: "wrap",
  },
  heading: { fontSize: tokens.fontSizeBase400, marginTop: 0, marginBottom: tokens.spacingVerticalS },
  body: { color: tokens.colorNeutralForeground2, maxWidth: "62ch" },
  warning: { color: tokens.colorPaletteDarkOrangeForeground1 },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  summary: {
    display: "grid",
    gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
    columnGap: tokens.spacingHorizontalXL,
    rowGap: tokens.spacingVerticalM,
    marginTop: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  summaryHeading: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  summaryDetail: { margin: 0, color: tokens.colorNeutralForeground2 },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalL,
    maxWidth: "680px",
    marginTop: tokens.spacingVerticalL,
  },
  choices: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    ...shorthands.padding(tokens.spacingVerticalS, 0),
  },
  evidenceRow: {
    display: "grid",
    gridTemplateColumns: "minmax(220px, 1fr) minmax(160px, 220px)",
    columnGap: tokens.spacingHorizontalL,
    alignItems: "end",
  },
  actions: { display: "flex", columnGap: tokens.spacingHorizontalS },
  history: {
    marginTop: tokens.spacingVerticalL,
  },
  historySummary: {
    cursor: "pointer",
    fontWeight: tokens.fontWeightSemibold,
    minHeight: "44px",
    display: "flex",
    alignItems: "center",
  },
});

const capabilityChoices: readonly { value: BeaconCapability; label: string }[] = [
  { value: "work.create_mission", label: "Create Missions" },
  { value: "work.form_team", label: "Form Teams" },
  { value: "work.coordinate_mission", label: "Coordinate Missions" },
  { value: "work.assign_work", label: "Assign Work" },
  { value: "artifact.produce", label: "Produce Artifacts" },
  { value: "artifact.evaluate", label: "Evaluate Artifacts" },
  { value: "evidence.develop", label: "Develop Evidence" },
];

const evidenceChoices: readonly {
  value: TrackRecordEvidenceKind;
  label: string;
  countLabel: string;
}[] = [
  { value: "mission_outcome", label: "Mission outcomes", countLabel: "Mission outcome minimum count" },
  { value: "evaluation", label: "Evaluations", countLabel: "Evaluation minimum count" },
  {
    value: "policy_compliance",
    label: "Policy compliance",
    countLabel: "Policy compliance minimum count",
  },
  { value: "spend", label: "Spend records", countLabel: "Spend record minimum count" },
  { value: "receipt", label: "Receipts", countLabel: "Receipt minimum count" },
];

type EvidenceCounts = Record<TrackRecordEvidenceKind, number | "" | null>;

export function ProjectPolicySection({
  projectId,
  charterRevision,
  policy,
  history,
  composerOpen,
  busy,
  onOpenComposer,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  charterRevision: number;
  policy: ProjectPolicy | null;
  history: ProjectPolicy[] | null;
  composerOpen: boolean;
  busy: boolean;
  onOpenComposer: () => void;
  onCancel: () => void;
  onSubmit: (draft: ProjectPolicyDraft) => void;
}) {
  const classes = useStyles();
  const nextRevision = (policy?.revision ?? 0) + 1;

  return (
    <section className={classes.section} aria-labelledby="project-policy-heading">
      <div className={classes.header}>
        <div>
          <h3 id="project-policy-heading" className={classes.heading}>
            Team Lead Policy
          </h3>
          <p className={classes.body}>
            Operator-authored criteria for a future Team Lead decision. Policy publication does not
            select a Lead or grant authority.
          </p>
        </div>
        {!composerOpen && (
          <Button onClick={onOpenComposer} disabled={busy}>
            {policy ? "Revise Team Lead Policy" : "Publish Team Lead Policy"}
          </Button>
        )}
      </div>

      {composerOpen ? (
        <ProjectPolicyComposer
          key={`${projectId}:${policy?.revision ?? 0}`}
          policy={policy}
          nextRevision={nextRevision}
          busy={busy}
          onCancel={onCancel}
          onSubmit={onSubmit}
        />
      ) : policy === null ? (
        <p className={classes.body}>No Team Lead Policy has been published.</p>
      ) : (
        <>
          <p className={classes.meta}>
            Policy revision {policy.revision} · Charter revision {policy.charterRevision}
          </p>
          {!policy.appliesToCurrentCharter && (
            <p className={classes.warning}>
              The Charter is now revision {charterRevision}. Publish a new Policy revision before
              evaluating candidates.
            </p>
          )}
          <div className={classes.summary} aria-label="Team Lead selection criteria">
            <div>
              <p className={classes.summaryHeading}>No Lead selected</p>
              <p className={classes.summaryDetail}>
                This revision declares criteria only; it creates no appointment or authority.
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Current Team eligibility required</p>
              <p className={classes.summaryDetail}>
                Only currently eligible Team members are available for later evaluation.
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Required capabilities</p>
              <p className={classes.summaryDetail}>
                {policy.teamLeadSelection.requiredCapabilities
                  .map((capability) => capabilityLabel(capability))
                  .join(", ")}
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Track Record thresholds</p>
              <p className={classes.summaryDetail}>
                {policy.teamLeadSelection.trackRecordRequirements
                  .map(
                    (requirement) =>
                      `${evidenceLabel(requirement.evidenceKind)}: ${requirement.minimumCount}`,
                  )
                  .join(" · ")}
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>No declared leadership conflict</p>
              <p className={classes.summaryDetail}>
                A declared conflict makes a candidate ineligible for the decision.
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Operator adjudication</p>
              <p className={classes.summaryDetail}>
                Operator decides whenever eligible candidates remain ambiguous.
              </p>
            </div>
          </div>
          {history !== null && history.length > 0 && (
            <details className={classes.history}>
              <summary className={classes.historySummary}>
                {history.length} preserved Policy {history.length === 1 ? "revision" : "revisions"}
              </summary>
              <ol>
                {[...history].reverse().map((revision) => (
                  <li key={revision.revision}>
                    Revision {revision.revision} · Charter revision {revision.charterRevision}
                    {revision.revision === policy.revision ? " (current)" : ""}
                  </li>
                ))}
              </ol>
            </details>
          )}
        </>
      )}
    </section>
  );
}

function ProjectPolicyComposer({
  policy,
  nextRevision,
  busy,
  onCancel,
  onSubmit,
}: {
  policy: ProjectPolicy | null;
  nextRevision: number;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: ProjectPolicyDraft) => void;
}) {
  const classes = useStyles();
  const [requiredCapabilities, setRequiredCapabilities] = useState<Set<BeaconCapability>>(
    () => new Set(policy?.teamLeadSelection.requiredCapabilities ?? ["work.coordinate_mission"]),
  );
  const [evidenceCounts, setEvidenceCounts] = useState<EvidenceCounts>(() => {
    const counts: EvidenceCounts = {
      mission_outcome: policy ? null : 1,
      evaluation: null,
      policy_compliance: null,
      spend: null,
      receipt: null,
    };
    for (const requirement of policy?.teamLeadSelection.trackRecordRequirements ?? []) {
      counts[requirement.evidenceKind] = requirement.minimumCount;
    }
    return counts;
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit({
      teamLeadSelection: {
        requiredCapabilities: capabilityChoices
          .map((choice) => choice.value)
          .filter((capability) => requiredCapabilities.has(capability)),
        availabilityRule: "current_team_eligibility",
        trackRecordRequirements: evidenceChoices.flatMap((choice) => {
          const minimumCount = evidenceCounts[choice.value];
          return typeof minimumCount === "number"
            ? [{ evidenceKind: choice.value, minimumCount }]
            : [];
        }),
        conflictRule: "require_no_declared_conflict",
        ambiguityRule: "require_operator_decision",
      },
    });
  }

  return (
    <form className={classes.form} aria-label="Publish Team Lead Policy" onSubmit={submit}>
      <fieldset>
        <legend>Required capabilities</legend>
        <div className={classes.choices}>
          {capabilityChoices.map((choice) => {
            const mandatory = choice.value === "work.coordinate_mission";
            return (
              <Checkbox
                key={choice.value}
                label={`${choice.label}${mandatory ? " — required" : ""}`}
                checked={mandatory || requiredCapabilities.has(choice.value)}
                disabled={mandatory || busy}
                onChange={(_, data) => {
                  const next = new Set(requiredCapabilities);
                  if (data.checked === true) next.add(choice.value);
                  else next.delete(choice.value);
                  setRequiredCapabilities(next);
                }}
              />
            );
          })}
        </div>
      </fieldset>

      <fieldset>
        <legend>Track Record evidence</legend>
        <p className={classes.body}>Select at least one named evidence kind and its minimum count.</p>
        <div className={classes.choices}>
          {evidenceChoices.map((choice) => {
            const selected = evidenceCounts[choice.value] !== null;
            return (
              <div className={classes.evidenceRow} key={choice.value}>
                <Checkbox
                  label={choice.label}
                  checked={selected}
                  disabled={busy}
                  onChange={(_, data) =>
                    setEvidenceCounts({
                      ...evidenceCounts,
                      [choice.value]: data.checked === true ? 1 : null,
                    })
                  }
                />
                {selected && (
                  <Field label={choice.countLabel} required>
                    <Input
                      aria-label={choice.countLabel}
                      type="number"
                      min={1}
                      step={1}
                      value={String(evidenceCounts[choice.value] ?? "")}
                      disabled={busy}
                      required
                      onChange={(_, data) => {
                        if (data.value === "") {
                          setEvidenceCounts({ ...evidenceCounts, [choice.value]: "" });
                          return;
                        }
                        const count = Number(data.value);
                        setEvidenceCounts({
                          ...evidenceCounts,
                          [choice.value]: Number.isInteger(count) && count >= 1 ? count : "",
                        });
                      }}
                    />
                  </Field>
                )}
              </div>
            );
          })}
        </div>
      </fieldset>

      <div className={classes.summary} aria-label="Fixed Team Lead safeguards">
        <div>
          <p className={classes.summaryHeading}>Availability</p>
          <p className={classes.summaryDetail}>Current Team eligibility is required.</p>
        </div>
        <div>
          <p className={classes.summaryHeading}>Conflict</p>
          <p className={classes.summaryDetail}>No declared leadership conflict is allowed.</p>
        </div>
        <div>
          <p className={classes.summaryHeading}>Ambiguity</p>
          <p className={classes.summaryDetail}>The Operator makes the final decision.</p>
        </div>
        <div>
          <p className={classes.summaryHeading}>Authority</p>
          <p className={classes.summaryDetail}>This does not select a Lead or grant authority.</p>
        </div>
      </div>

      <div className={classes.actions}>
        <Button appearance="primary" type="submit" disabled={busy || !hasEvidence(evidenceCounts)}>
          Publish Policy revision {nextRevision}
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function hasEvidence(counts: EvidenceCounts): boolean {
  return Object.values(counts).some((count) => typeof count === "number" && count >= 1);
}

function capabilityLabel(capability: BeaconCapability): string {
  return capabilityChoices.find((choice) => choice.value === capability)?.label ?? capability;
}

function evidenceLabel(kind: TrackRecordEvidenceKind): string {
  return evidenceChoices.find((choice) => choice.value === kind)?.label ?? kind;
}
