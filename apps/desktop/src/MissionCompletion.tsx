import { Button, makeStyles, tokens } from "@fluentui/react-components";
import { useEffect, useState } from "react";

import { MissionCompletionCompose } from "./MissionCompletionCompose";

import {
  type CompletionView,
  type MissionView,
  describeLocalAccountError,
  loadMissionCompletion,
} from "./session";

const useStyles = makeStyles({
  section: { marginTop: tokens.spacingVerticalL },
  heading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  body: { fontSize: tokens.fontSizeBase200, marginTop: 0, marginBottom: tokens.spacingVerticalXS },
  list: { marginTop: 0, marginBottom: tokens.spacingVerticalS, paddingLeft: tokens.spacingHorizontalL },
  outcome: { fontWeight: tokens.fontWeightSemibold },
});

/** This read's own closed refusal gets its fixed sentence; everything else keeps the meaning it
 *  already had. A session or authentication refusal relabelled as a completion problem would be a
 *  false explanation, so only the one C3 code is intercepted. */
function describeCompletionError(error: unknown): string {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  if (code === "mission_completion_unavailable") {
    return "The Mission completion could not be verified.";
  }
  return describeLocalAccountError(error);
}

const OUTCOME_LABEL: Record<CompletionView["conditionResults"][number]["outcome"], string> = {
  met: "Met",
  not_met: "Not met",
  not_assessed: "Not assessed",
  assessment_inconclusive: "Assessment inconclusive",
};

/** Neutral by construction. `MissionCompletion` reports what a completion RECORDED and derives no
 *  verdict from it: neither disposition means success (`mission_completion.rs:189-197`), and a
 *  Mission with no completion has not failed -- nobody recorded one. */
const DISPOSITION_LABEL: Record<CompletionView["disposition"], string> = {
  concluded: "Concluded",
  abandoned: "Abandoned",
};

/// The verified completion of ONE Mission, read only.
///
/// Publication is a separate fact: a completion can exist with no published Outcome, and this
/// section says nothing about whether one could be prepared. Support counts are counts of CITED
/// references -- this reader opens none of them, so nothing here is described as verified.
export function MissionCompletion({ mission }: { mission: MissionView }) {
  const classes = useStyles();
  // `undefined` is "not answered yet". `null` is the owner's answer that nothing was recorded --
  // a different fact, and never produced by a failure.
  const [completion, setCompletion] = useState<CompletionView | null | undefined>(undefined);
  const [failure, setFailure] = useState("");
  // Bumped by the operator's explicit Retry and nothing else: no timer, no poll, no automatic loop.
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let active = true;
    setCompletion(undefined);
    setFailure("");
    loadMissionCompletion(mission.id).then(
      (loaded) => {
        // The same lifetime guard on BOTH legs. A response for a Mission the operator has left
        // cannot replace the current one's value, plant a failure over it, or release its pending
        // state.
        if (!active) return;
        setCompletion(loaded);
      },
      (error: unknown) => {
        if (!active) return;
        setFailure(describeCompletionError(error));
      },
    );
    return () => {
      active = false;
    };
  }, [mission.id, attempt]);

  // The EXACT set, not "each one is in range". Uniqueness plus in-range still admits a completion
  // that simply omits a declared condition, and rendering the rest would show a partial assessment
  // as if it were whole -- the omission C3 requires to be unavailable. Every declared ordinal must
  // appear exactly once, and no others.
  const pairable =
    completion == null ||
    (completion.conditionResults.length === mission.successConditions.length &&
      mission.successConditions.every((_condition, ordinal) =>
        completion.conditionResults.some((result) => result.ordinal === ordinal),
      ));

  return (
    <section className={classes.section} aria-label="Mission completion">
      <h4 className={classes.heading}>Completion</h4>
      {failure !== "" || !pairable ? (
        <>
          <p role="alert" className={classes.body}>
            {failure !== "" ? failure : "The Mission completion could not be verified."}
          </p>
          <Button size="small" onClick={() => setAttempt((current) => current + 1)}>
            Retry Mission completion
          </Button>
        </>
      ) : completion === undefined ? (
        <p role="status" className={classes.body}>
          Loading the Mission completion
        </p>
      ) : completion === null ? (
        <>
          <p className={classes.body}>No completion recorded for this Mission.</p>
          {/* Offered only after a SUCCESSFUL null read. An unavailable read never enables it: not
              knowing whether a completion exists is not the same as knowing none does. */}
          <MissionCompletionCompose
            mission={mission}
            onRecorded={() => setAttempt((current) => current + 1)}
          />
        </>
      ) : (
        <>
          <p className={classes.body}>
            <span className={classes.outcome}>{DISPOSITION_LABEL[completion.disposition]}</span>
            {" · Operator disposition · accountable Resident "}
            {completion.accountableResidentId}
          </p>
          <ol className={classes.list} aria-label="Recorded condition results">
            {completion.conditionResults.map((result) => (
              <li key={result.ordinal} className={classes.body}>
                {mission.successConditions[result.ordinal]}
                {" — "}
                <span className={classes.outcome}>{OUTCOME_LABEL[result.outcome]}</span>
                {" · "}
                {result.supportCount} cited support{result.supportCount === 1 ? "" : "s"}
                {result.note !== null && ` · ${result.note}`}
              </li>
            ))}
          </ol>
          <p className={classes.body}>
            {completion.deliveredArtifactCount} delivered Artifact
            {completion.deliveredArtifactCount === 1 ? "" : "s"} recorded ·{" "}
            {completion.unfinishedWorkCount} unfinished item
            {completion.unfinishedWorkCount === 1 ? "" : "s"} recorded
          </p>
        </>
      )}
    </section>
  );
}
