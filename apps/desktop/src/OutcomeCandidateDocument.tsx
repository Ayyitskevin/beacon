import { Button, makeStyles, tokens } from "@fluentui/react-components";
import { useEffect, useId, useRef, useState } from "react";

import {
  type PreparedOutcomeCandidate,
  describeLocalAccountError,
  readOutcomeCandidateDocument,
} from "./session";

const useStyles = makeStyles({
  block: { marginTop: tokens.spacingVerticalM },
  heading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalXS,
  },
  body: { fontSize: tokens.fontSizeBase200, marginTop: 0, marginBottom: tokens.spacingVerticalS },
  document: {
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
    maxHeight: "24rem",
    overflowY: "auto",
    margin: 0,
  },
});

/** This surface's two closed codes. Everything else keeps the meaning it already had: an account or
 *  session refusal relabelled as a candidate problem would be a false explanation. */
const CODE_COPY: Record<string, string> = {
  outcome_candidate_not_found: "That Outcome candidate is not available in this Workspace.",
  outcome_candidate_unavailable: "Beacon could not verify this Outcome candidate.",
};

function codeOf(error: unknown): string | undefined {
  const code =
    typeof error === "object" && error !== null && !Array.isArray(error)
      ? (error as Record<string, unknown>).code
      : undefined;
  return typeof code === "string" ? code : undefined;
}

/** The codes this surface FORWARDS unchanged, so the session and transport keep the meanings they
 *  already have. Membership in a known vocabulary, never the presence of a `code` field.
 *
 *  MEASURED, and it is the same lesson root taught the C6 lane: "it has a code" is not a predicate.
 *  A fatal `TextDecoder` failure is a Node `TypeError` carrying
 *  `code: "ERR_ENCODING_INVALID_ENCODED_DATA"`, so a presence test routed an invalid-UTF-8 reply --
 *  this client refusing bytes -- into the account describer, and the Operator was told "Beacon could
 *  not complete that local request." My own bar caught it. */
const FORWARDED_CODES: ReadonlySet<string> = new Set([
  "not_authenticated",
  "authentication_failed",
  "authentication_throttled",
  "credential_disabled",
  "invalid_session_timeout",
  "account_service_unavailable",
  "local_request_failed",
]);

/** A shape, binding, base64 or UTF-8 fault is this client refusing a reply, and it is UNAVAILABLE --
 *  never an absence, and never the raw diagnostic, which stays in the thrown error for a developer
 *  and out of the Operator's sentence. Anything this surface does not recognise lands here too. */
function describeReadError(error: unknown): string {
  const code = codeOf(error);
  if (code !== undefined && code in CODE_COPY) return CODE_COPY[code];
  if (code !== undefined && FORWARDED_CODES.has(code)) return describeLocalAccountError(error);
  return CODE_COPY.outcome_candidate_unavailable;
}

export function OutcomeCandidateDocument({
  receipt,
}: {
  receipt: PreparedOutcomeCandidate;
}): React.JSX.Element {
  const styles = useStyles();
  const headingId = useId();
  const documentId = useId();
  const [pending, setPending] = useState(false);
  const [text, setText] = useState<string | null>(null);
  const [failure, setFailure] = useState("");

  /** ONE lifetime counter, read at CALLBACK time through a ref. Bumped by a changed held identity,
   *  by a newer attempt and by unmount.
   *
   *  Not a comparison against a captured identity: Mission A to B and back to A holds the SAME
   *  digest both times, so a value comparison would happily attach the first attempt's reply to the
   *  second receipt. A counter cannot: it only ever moves forward. */
  const live = useRef({ generation: 0, identity: `${receipt.missionId}/${receipt.manifestDigest.value}` });

  function fresh(): number {
    live.current.generation += 1;
    return live.current.generation;
  }

  function stale(mine: number): boolean {
    return live.current.generation !== mine;
  }

  // A different held candidate inherits NOTHING: not text, not a failure, not a pending read.
  useEffect(() => {
    const identity = `${receipt.missionId}/${receipt.manifestDigest.value}`;
    if (live.current.identity === identity) return;
    live.current.identity = identity;
    live.current.generation += 1;
    setPending(false);
    setText(null);
    setFailure("");
  }, [receipt.missionId, receipt.manifestDigest.value]);

  useEffect(() => {
    const lifetime = live;
    return () => {
      lifetime.current.generation += 1;
    };
  }, []);

  /** The explicit read, and the explicit retry: the SAME held identity, read again. Never a
   *  re-prepare, never a write, and never on a timer. */
  function read(): void {
    if (pending) return;
    const mine = fresh();
    setPending(true);
    setText(null);
    setFailure("");
    readOutcomeCandidateDocument(receipt).then(
      (loaded) => {
        if (stale(mine)) return;
        setPending(false);
        setText(loaded.text);
      },
      (error: unknown) => {
        if (stale(mine)) return;
        setPending(false);
        setFailure(describeReadError(error));
      },
    );
  }

  return (
    <section className={styles.block} aria-labelledby={headingId}>
      <h5 className={styles.heading} id={headingId}>
        Candidate document
      </h5>
      <p className={styles.body}>
        This is the stored candidate text. Viewing it does not evaluate or publish it.
      </p>
      <Button disabled={pending} onClick={read}>
        {failure === "" ? "View candidate document" : "Read again"}
      </Button>
      {pending && <p className={styles.body}>Reading candidate document…</p>}
      {!pending && text === null && failure === "" && (
        <p className={styles.body}>Candidate document is not loaded.</p>
      )}
      {failure !== "" && <p className={styles.body}>{failure}</p>}
      {text !== null && (
        <>
          <p className={styles.body} id={documentId}>
            Stored candidate text
          </p>
          {/* TEXT, never markup: the content is set as a child string, so a document that contains
              `<em>` shows those characters. No Markdown is evaluated and no HTML is injected --
              letting a document decide how it appears is the opposite of showing what was stored. */}
          <pre className={styles.document} aria-labelledby={documentId} tabIndex={0}>
            {text}
          </pre>
        </>
      )}
    </section>
  );
}
