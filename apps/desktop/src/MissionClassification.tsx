import {
  Button,
  Dropdown,
  Field,
  Option,
  Textarea,
  makeStyles,
  tokens,
} from "@fluentui/react-components";
import { useEffect, useRef, useState } from "react";

import {
  type ClassificationIntentRequest,
  type ClassificationObservation,
  type ClassificationView,
  type MissionView,
  describeLocalAccountError,
  loadMissionClassification,
  recordMissionClassification,
} from "./session";

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalL },
  heading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  body: { fontSize: tokens.fontSizeBase200, marginTop: 0, marginBottom: tokens.spacingVerticalXS },
  block: { marginBottom: tokens.spacingVerticalS },
  row: { display: "flex", columnGap: tokens.spacingHorizontalS, flexWrap: "wrap", alignItems: "end" },
});

/** The two arms, and only two. ADR 0044 has no third, and a default would be this screen deciding
 *  something only the Operator can. */
const IMPACTS: readonly (readonly ["high" | "ordinary", string])[] = [
  ["high", "High"],
  ["ordinary", "Ordinary"],
];

/** The owner's own scalar bound (`mission_impact.rs:52`), counted in Unicode SCALARS: a `.length`
 *  bound would accept half as much astral text as the owner does. */
const REASON_SCALARS = 2_000;
const scalars = (text: string) => Array.from(text).length;

const IMPACT_LABEL: Record<ClassificationView["impact"], string> = {
  high: "High",
  ordinary: "Ordinary",
};

/** What the RECORD says decided it. A legacy `policy_rule` row keeps its own basis and rule identity
 *  (`mission_impact.rs:332-343` admits one, and C5-interface.md forbids relabelling it), and this
 *  line reports what was RECORDED -- it never says a rule was applied, evaluated or re-checked here,
 *  because nothing in this surface runs one. */
function recordedBasis(view: ClassificationView): string {
  return view.basis === "policy_rule" && view.ruleId !== null
    ? `Recorded basis: Policy rule ${view.ruleId}.`
    : "Recorded basis: the Operator's own declaration.";
}

/** This surface's own closed codes get their fixed sentences; every other refusal keeps the meaning
 *  it already had. A session or authentication refusal relabelled as a classification problem would
 *  be a false explanation, so nothing outside this table is intercepted. */
const CODE_COPY: Record<string, string> = {
  classification_stale_policy_revision:
    "The Project Policy changed. Review the Policy and classify again.",
  classification_no_policy_published: "No Team Lead Policy is published for this Project.",
  classification_already_classified:
    "A classification is already recorded for this Mission. Reclassify it by naming the recorded one.",
  classification_predecessor_not_latest:
    "That classification is no longer the latest. Read the classification and name the current one.",
  classification_predecessor_unproven:
    "The classification named as replaced could not be verified.",
  classification_version_ceiling_reached: "This Mission cannot record another classification.",
  classification_text_over_limit: "The reason exceeds the supported length.",
  classification_invalid_details:
    "Some classification details are invalid. Review them and try again.",
  mission_classification_unavailable: "The Mission classification could not be verified.",
};

/** The answers that do NOT settle whether a write happened.
 *
 *  Not a claim about the owner's rollback: the owner returns its `Write`/`WriteReadBackDisagrees`
 *  arms BEFORE `commit`, and dropping the `Err` drops the transaction, so those arms do not leave a
 *  committed record. The reason these two answers are unknown HERE is loss of acknowledgement and
 *  coarse mapping at the desktop: `mission_classification_unavailable` is one outward code over
 *  several owner causes, and `local_request_failed` is the desktop's generic for a protocol, JSON or
 *  I/O fault -- which includes a RESPONSE lost after a commit the daemon really made. A caller that
 *  cannot tell those apart must not announce either outcome. No code at all is the same situation.
 *
 *  Everything else with a recognised code is a definite answer from the daemon -- including the
 *  classification refusals, which the owner makes before writing, and the account and session
 *  refusals, which stop the request before it reaches the owner at all. Those keep the form and their
 *  own meaning. */
const UNKNOWN_WRITE_OUTCOMES: readonly string[] = [
  "mission_classification_unavailable",
  "local_request_failed",
];

function codeOf(error: unknown): string | undefined {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  return typeof code === "string" ? code : undefined;
}

function describeClassificationError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  if (typeof code === "string" && code in CODE_COPY) return CODE_COPY[code];
  return describeLocalAccountError(error);
}

/** The Policy citation a NEW classification would make, in the future tense. Distinct from the
 *  historical citation on a recorded record, which is a different fact about a different Policy
 *  revision. */
function policyToCite(observation: ClassificationObservation): string {
  const policy = observation.publishedPolicy;
  if (policy === null) return "";
  return `This classification will cite Policy revision ${policy.policyRevision} (Charter revision ${policy.charterRevision}).`;
}

/// The Mission's impact classification: one snapshot, and the Operator's own judgment.
///
/// A classification is not a completion, not a preparation and not a publication, and recording one
/// says nothing about whether an Outcome could be prepared or published.
export function MissionClassification({
  mission,
  onOpenProjectPolicy,
}: {
  mission: MissionView;
  /** Navigation, not a classification: the Policy surface already exists and owns publication. */
  onOpenProjectPolicy?: () => void;
}) {
  const classes = useStyles();
  // `undefined` is "not answered yet"; an answered observation always carries its own cursor.
  const [observation, setObservation] = useState<ClassificationObservation | undefined>(undefined);
  const [readFailure, setReadFailure] = useState("");
  const [impact, setImpact] = useState<"" | "high" | "ordinary">("");
  const [reason, setReason] = useState("");
  const [writeFailure, setWriteFailure] = useState("");
  const [recorded, setRecorded] = useState<ClassificationView | null>(null);
  const [pending, setPending] = useState(false);
  /** What was actually submitted, kept so a later read can be compared field for field. Never a
   *  claim that it was recorded. */
  const [submitted, setSubmitted] = useState<ClassificationIntentRequest | null>(null);
  const [outcome, setOutcome] = useState<"" | "recorded" | "unknown">("");
  /** The observation taken AFTER an unknown outcome, which is a different fact from the one the form
   *  was built from. */
  const [recovery, setRecovery] = useState<ClassificationObservation | null>(null);
  const [recoveryFailure, setRecoveryFailure] = useState("");
  const [recoveryPending, setRecoveryPending] = useState(false);
  /** ONE lifetime counter, read at CALLBACK time through a ref.
   *
   *  Not a comparison against a captured `mission.id`: both sides of that comparison come from the
   *  same render, so they move together and it can never disagree -- the defect C4 measured. A
   *  generation is bumped by a Mission change, a newer leg and unmount, and a leg whose generation is
   *  no longer current writes nothing and starts nothing. */
  const live = useRef({ generation: 0, missionId: mission.id });

  function fresh(): number {
    live.current.generation += 1;
    return live.current.generation;
  }

  function stale(mine: number): boolean {
    return live.current.generation !== mine;
  }

  function resetJudgment() {
    setImpact("");
    setReason("");
    setWriteFailure("");
    setRecorded(null);
    setPending(false);
    setSubmitted(null);
    setOutcome("");
    setRecovery(null);
    setRecoveryFailure("");
    setRecoveryPending(false);
  }

  // A different Mission inherits NOTHING: not an impact, not a reason, not a pending outcome.
  useEffect(() => {
    if (live.current.missionId === mission.id) return;
    live.current.missionId = mission.id;
    live.current.generation += 1;
    resetJudgment();
  }, [mission.id]);

  // Unmount retires the current generation. `setState` after unmount is a silent no-op, but a
  // read-back is a NEW request and a parent callback is the parent's: neither is silent.
  useEffect(() => {
    const lifetime = live;
    return () => {
      lifetime.current.generation += 1;
    };
  }, []);
  // Bumped by the Operator's explicit Retry and nothing else: no timer, no poll, no automatic loop.
  const [readAttempt, setReadAttempt] = useState(0);

  useEffect(() => {
    const mine = fresh();
    setObservation(undefined);
    setReadFailure("");
    loadMissionClassification(mission.id).then(
      (loaded) => {
        if (stale(mine)) return;
        setObservation(loaded);
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setReadFailure(describeClassificationError(error));
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `fresh`/`stale` read the ref, not state
  }, [mission.id, readAttempt]);

  /** The Operator's judgment is complete, and within the owner's own bound. */
  function ready(current: ClassificationObservation): boolean {
    return (
      !pending &&
      current.publishedPolicy !== null &&
      impact !== "" &&
      reason.trim() === reason &&
      reason !== "" &&
      scalars(reason) <= REASON_SCALARS
    );
  }

  function submit(event: React.FormEvent) {
    event.preventDefault();
    if (observation === undefined || !ready(observation)) return;
    const policyRevision = observation.publishedPolicy?.policyRevision ?? null;
    const request: ClassificationIntentRequest = {
      missionId: mission.id,
      impact: impact as "high" | "ordinary",
      reason,
      // The revision the Operator was shown, echoed back: a Policy published mid-form must not
      // silently change what the record says decided.
      expectedPolicyRevision: policyRevision,
      // The predecessor comes from the verified read, never from anything this screen invented: a
      // first classification replaces nothing, and a reclassification names the record it replaces.
      supersedes:
        observation.latest === null
          ? { kind: "none" }
          : { kind: "record", predecessorDigest: observation.latest.digest.value },
    };
    setPending(true);
    setWriteFailure("");
    setOutcome("");
    setRecovery(null);
    setRecoveryFailure("");
    setSubmitted(request);
    const mine = fresh();
    recordMissionClassification(request).then(
      (view) => {
        if (stale(mine)) return;
        setPending(false);
        setRecorded(view);
        setOutcome("recorded");
        // One read, caused by this success and nothing else: the panel's record and its cursor are
        // both a snapshot, and the snapshot the form was built from is now older than the store.
        setReadAttempt((current) => current + 1);
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setPending(false);
        const code = codeOf(error);
        if (code === undefined || UNKNOWN_WRITE_OUTCOMES.includes(code)) {
          // UNKNOWN. Not failed, not recorded, and never retried automatically.
          setOutcome("unknown");
          return;
        }
        // A definite answer: nothing was written. The form is preserved and the Operator decides.
        setWriteFailure(describeClassificationError(error));
      },
    );
  }

  /** A second READ of the same Mission, run only when the Operator asks. It writes nothing. */
  function readBack() {
    if (recoveryPending) return;
    const mine = fresh();
    setRecoveryPending(true);
    setRecoveryFailure("");
    setRecovery(null);
    loadMissionClassification(mission.id).then(
      (loaded) => {
        if (stale(mine)) return;
        setRecoveryPending(false);
        setRecovery(loaded);
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setRecoveryPending(false);
        setRecoveryFailure(describeClassificationError(error));
      },
    );
  }

  /** Does this record hold the content that was submitted? EVERY authored field and every echoed
   *  binding, not impact and reason alone -- and a match is a match of CONTENT, never attribution. */
  function holdsSubmittedContent(view: ClassificationView): boolean {
    if (submitted === null) return false;
    const sameSupersedes =
      submitted.supersedes.kind === "none"
        ? view.supersedes.kind === "none"
        : view.supersedes.kind === "record" &&
          view.supersedes.predecessorDigest === submitted.supersedes.predecessorDigest;
    return (
      view.impact === submitted.impact &&
      view.reason === submitted.reason &&
      view.policy.policyRevision === submitted.expectedPolicyRevision &&
      sameSupersedes
    );
  }

  return (
    <section className={classes.section} aria-label="Mission impact classification">
      <h4 className={classes.heading}>Impact classification</h4>
      {readFailure !== "" ? (
        // An unreadable observation is NOT an empty one: nothing here says a classification does or
        // does not exist, and no form is offered on a snapshot nobody could take.
        <>
          <p role="alert" className={classes.body}>
            {readFailure}
          </p>
          <Button size="small" onClick={() => setReadAttempt((current) => current + 1)}>
            Retry the read
          </Button>
        </>
      ) : observation === undefined ? (
        <p role="status" className={classes.body}>
          Reading the classification
        </p>
      ) : (
        <>
          {observation.latest === null ? (
            <p className={classes.body}>
              No classification is recorded for this Mission as of event{" "}
              {observation.observedAtEventSeq}.
            </p>
          ) : (
            <>
              <p className={classes.body}>
                Recorded {IMPACT_LABEL[observation.latest.impact]} impact, version{" "}
                {observation.latest.version}, citing Policy revision{" "}
                {observation.latest.policy.policyRevision} (Charter revision{" "}
                {observation.latest.policy.charterRevision}).
              </p>
              <p className={classes.body}>{recordedBasis(observation.latest)}</p>
              <p className={classes.body}>{observation.latest.reason}</p>
              <p className={classes.body}>
                Content identity {observation.latest.digest.value}, as of event{" "}
                {observation.observedAtEventSeq}.
              </p>
            </>
          )}
          {observation.publishedPolicy === null ? (
            // A prerequisite, not a refusal: the Operator is sent to the surface that can satisfy it.
            <>
              <p className={classes.body}>
                No Team Lead Policy is published for this Project. A classification cites the Policy
                that decided it.
              </p>
              {onOpenProjectPolicy !== undefined && (
                <Button size="small" onClick={onOpenProjectPolicy}>
                  Open Project Policy
                </Button>
              )}
            </>
          ) : (
            <>
              <p className={classes.body}>{policyToCite(observation)}</p>
              <p className={classes.body}>
                A classification recorded now does not change an Outcome already cut at an earlier
                event.
              </p>
              <form aria-label="Record impact classification" onSubmit={submit}>
                <Field label="Impact" className={classes.block}>
                  <Dropdown
                    aria-label="Impact"
                    placeholder="Choose the impact this Mission had"
                    selectedOptions={impact === "" ? [] : [impact]}
                    onOptionSelect={(_event, data) =>
                      setImpact((data.optionValue ?? "") as "" | "high" | "ordinary")
                    }
                  >
                    {IMPACTS.map(([value, label]) => (
                      <Option key={value} value={value}>
                        {label}
                      </Option>
                    ))}
                  </Dropdown>
                </Field>
                <Field label="Reason" className={classes.block}>
                  <Textarea
                    aria-label="Reason"
                    value={reason}
                    onChange={(_event, data) => setReason(data.value)}
                  />
                </Field>
                {writeFailure !== "" && (
                  <p role="alert" className={classes.body}>
                    {writeFailure}
                  </p>
                )}
                {pending && (
                  <p role="status" className={classes.body}>
                    Recording the classification
                  </p>
                )}
                <div className={classes.row}>
                  <Button appearance="primary" type="submit" disabled={!ready(observation)}>
                    Record classification
                  </Button>
                </div>
              </form>
              {outcome === "recorded" && recorded !== null && (
                <p role="status" className={classes.body}>
                  A classification is recorded at version {recorded.version}.
                </p>
              )}
              {outcome === "unknown" && (
                <>
                  <p role="alert" className={classes.body}>
                    Whether your submission was recorded is still unknown.
                  </p>
                  {recoveryPending && (
                    <p role="status" className={classes.body}>
                      Reading the classification
                    </p>
                  )}
                  {recoveryFailure !== "" && (
                    <p role="alert" className={classes.body}>
                      {recoveryFailure}
                    </p>
                  )}
                  {recovery !== null &&
                    (recovery.latest === null ? (
                      <p className={classes.body}>
                        No classification is recorded for this Mission as of event{" "}
                        {recovery.observedAtEventSeq}.
                      </p>
                    ) : holdsSubmittedContent(recovery.latest) ? (
                      <p className={classes.body}>
                        A recorded classification matches what you submitted. This is not proof that
                        this attempt recorded it.
                      </p>
                    ) : (
                      // States the observation and nothing about our write: the attempt may have
                      // committed and a further classification may have followed it.
                      <p className={classes.body}>
                        The latest recorded classification for this Mission is version{" "}
                        {recovery.latest.version}, {IMPACT_LABEL[recovery.latest.impact]}, as of event{" "}
                        {recovery.observedAtEventSeq}.
                      </p>
                    ))}
                  <Button size="small" onClick={readBack} disabled={recoveryPending}>
                    Read the classification
                  </Button>
                </>
              )}
            </>
          )}
        </>
      )}
    </section>
  );
}
