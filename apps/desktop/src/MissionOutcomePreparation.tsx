import { Button, Field, Radio, RadioGroup, makeStyles, tokens } from "@fluentui/react-components";
import { useEffect, useId, useRef, useState } from "react";

import {
  type AppendixRequest,
  type PreparedOutcomeCandidate,
  describeLocalAccountError,
  prepareOutcomeCandidate,
} from "./session";

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalL },
  heading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  body: { fontSize: tokens.fontSizeBase200, marginTop: 0, marginBottom: tokens.spacingVerticalS },
  row: { display: "flex", columnGap: tokens.spacingHorizontalS, flexWrap: "wrap", alignItems: "end" },
  identities: { fontSize: tokens.fontSizeBase200, margin: 0, paddingLeft: tokens.spacingHorizontalL },
  digest: { fontFamily: tokens.fontFamilyMonospace, overflowWrap: "anywhere" },
});

/** The two arms of the request, and only two. The owner's vocabulary has no `omit` -- a caller must
 *  not be able to suppress the verification appendix by asking -- and nothing here is preselected:
 *  which appendix rule applies is the Operator's statement, not this screen's. */
const REQUESTS: readonly (readonly [AppendixRequest, string, string])[] = [
  [
    "policy_default",
    "Use Policy default",
    "Beacon includes the verification appendix when the Mission was classified high-impact.",
  ],
  [
    "include",
    "Include verification appendix",
    "Include the verification appendix even when Policy does not require it.",
  ],
];

const REQUEST_LABEL: Record<AppendixRequest, string> = {
  policy_default: "Policy default",
  include: "Include verification appendix",
};

const MODE_LABEL: Record<PreparedOutcomeCandidate["appendixMode"], string> = {
  included: "the verification appendix is included",
  policy_state_not_high_impact: "the Mission is not high-impact, so the appendix is not included",
};

/** This surface's own closed codes get their fixed sentences. Everything else keeps the meaning it
 *  already had: an account or session refusal relabelled as a preparation problem would be a false
 *  explanation, so nothing outside this table is intercepted. */
const CODE_COPY: Record<string, string> = {
  outcome_not_completed: "This Mission has no recorded completion to prepare.",
  outcome_not_classified: "This Mission has no impact classification.",
  outcome_no_classification_at_completion:
    "This Mission was classified only after completion. This completion cannot be prepared as an Outcome, and another classification will not change its boundary.",
  outcome_preparation_unavailable:
    "Beacon could not verify the records needed to prepare this Outcome candidate.",
};

/** The answers that do NOT settle whether a candidate was written.
 *
 *  The three sequencing refusals above are decided BEFORE the owner assembles or writes anything, so
 *  they are definite. `outcome_preparation_unavailable` is not: it is one outward code over several
 *  owner causes, and the contract puts a failed post-commit owner read behind it -- the write is
 *  preserved and the receipt is not. An uncoded failure is the same situation: a transport or shape
 *  fault can follow a commit the daemon really made, and a reply this client refuses to read is a
 *  reply, not an absence. None of these may be announced as "nothing happened". */
function settles(code: string | undefined): boolean {
  return code !== undefined && code !== "outcome_preparation_unavailable";
}

function codeOf(error: unknown): string | undefined {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  return typeof code === "string" ? code : undefined;
}

function describePreparationError(error: unknown): string {
  const code = codeOf(error);
  if (code !== undefined && code in CODE_COPY) return CODE_COPY[code];
  return describeLocalAccountError(error);
}

/** What the Operator submitted. Held so an explicit repeat sends the SAME intent, and never treated
 *  as evidence that it was recorded. */
type Intent = { missionId: string; appendixRequest: AppendixRequest };

export function MissionOutcomePreparation({ missionId }: { missionId: string }): React.JSX.Element {
  const styles = useStyles();
  const headingId = useId();
  const [choice, setChoice] = useState<"" | AppendixRequest>("");
  const [pending, setPending] = useState(false);
  const [receipt, setReceipt] = useState<PreparedOutcomeCandidate | null>(null);
  const [failure, setFailure] = useState("");
  const [outcome, setOutcome] = useState<"" | "prepared" | "refused" | "unknown">("");
  const [submitted, setSubmitted] = useState<Intent | null>(null);

  /** ONE lifetime counter, read at CALLBACK time through a ref.
   *
   *  Not a comparison against a captured `mission.id`: both sides of that comparison come from the
   *  same render, so they move together and can never disagree. A generation is bumped by a Mission
   *  change, a changed choice, a newer attempt and unmount; a leg whose generation is no longer
   *  current writes nothing. */
  const live = useRef({ generation: 0, missionId });

  function fresh(): number {
    live.current.generation += 1;
    return live.current.generation;
  }

  function stale(mine: number): boolean {
    return live.current.generation !== mine;
  }

  function forget(): void {
    setPending(false);
    setReceipt(null);
    setFailure("");
    setOutcome("");
    setSubmitted(null);
  }

  // A different Mission inherits NOTHING: not a choice, not a receipt, not a pending outcome. A
  // Mission that comes BACK inherits nothing either -- the generation moved on both times.
  useEffect(() => {
    if (live.current.missionId === missionId) return;
    live.current.missionId = missionId;
    live.current.generation += 1;
    setChoice("");
    forget();
  }, [missionId]);

  // Unmount retires the current generation. `setState` after unmount is a silent no-op, but a reply
  // that arrives later must not be treated as this screen's answer by anything else either.
  useEffect(() => {
    const lifetime = live;
    return () => {
      lifetime.current.generation += 1;
    };
  }, []);

  function send(intent: Intent): void {
    if (pending) return;
    const mine = fresh();
    setPending(true);
    setReceipt(null);
    setFailure("");
    setOutcome("");
    setSubmitted(intent);
    prepareOutcomeCandidate(intent).then(
      (prepared) => {
        if (stale(mine)) return;
        setPending(false);
        setReceipt(prepared);
        setOutcome("prepared");
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setPending(false);
        setFailure(describePreparationError(error));
        setOutcome(settles(codeOf(error)) ? "refused" : "unknown");
      },
    );
  }

  /** A different appendix choice is a DIFFERENT intent. The generation moves, so a reply to the old
   *  one cannot land, and the feedback it would have written is cleared rather than left standing
   *  beside a choice it was never about. */
  function chooseAppendix(next: AppendixRequest): void {
    if (next === choice) return;
    live.current.generation += 1;
    setChoice(next);
    forget();
  }

  return (
    <section className={styles.section} aria-labelledby={headingId}>
      <h4 className={styles.heading} id={headingId}>
        Prepare Outcome candidate
      </h4>
      <p className={styles.body}>
        Create a candidate from this Mission’s recorded completion and evidence. This does not
        evaluate or publish it.
      </p>

      <Field label="Verification appendix">
        <RadioGroup
          value={choice}
          onChange={(_, data) => chooseAppendix(data.value as AppendixRequest)}
        >
          {REQUESTS.map(([value, label, hint]) => (
            <Radio key={value} value={value} label={`${label} — ${hint}`} />
          ))}
        </RadioGroup>
      </Field>

      <div className={styles.row}>
        <Button
          appearance="primary"
          disabled={choice === "" || pending}
          onClick={() => {
            if (choice === "") return;
            send({ missionId, appendixRequest: choice });
          }}
        >
          Prepare candidate
        </Button>
        {pending && <span className={styles.body}>Preparing Outcome candidate…</span>}
      </div>

      {outcome === "" && !pending && <p className={styles.body}>Preparation status is not loaded.</p>}

      {outcome === "prepared" && receipt !== null && (
        <div>
          <p className={styles.body}>
            Outcome candidate prepared. Preparing does not evaluate or publish it.
          </p>
          <p className={styles.body}>
            The candidate is prepared, but this screen cannot display its document yet.
          </p>
          <ul className={styles.identities}>
            <li>
              <span>{`Appendix: ${REQUEST_LABEL[receipt.appendixRequest]} — ${MODE_LABEL[receipt.appendixMode]}.`}</span>
            </li>
            <li>
              Manifest: <span className={styles.digest}>{receipt.manifestId}</span>
            </li>
            <li>
              Manifest digest: <span className={styles.digest}>{receipt.manifestDigest.value}</span>
            </li>
            <li>
              Candidate Artifact digest:{" "}
              <span className={styles.digest}>{receipt.candidateArtifactDigest.value}</span>
            </li>
            <li>
              Candidate content digest:{" "}
              <span className={styles.digest}>{receipt.candidateContentDigest.value}</span>
            </li>
            <li>
              Recorded content size: <span>{`${receipt.candidateContentBytes} bytes`}</span>
            </li>
          </ul>
        </div>
      )}

      {outcome === "refused" && <p className={styles.body}>{failure}</p>}

      {outcome === "unknown" && (
        <div>
          <p className={styles.body}>{failure}</p>
          <p className={styles.body}>
            Beacon could not confirm whether the candidate was prepared. No automatic retry was made.
            You may explicitly prepare again with the same appendix choice.
          </p>
          {submitted !== null && (
            <Button disabled={pending} onClick={() => send(submitted)}>
              Prepare again with the same choice
            </Button>
          )}
        </div>
      )}
    </section>
  );
}
