import { makeStyles, shorthands, tokens } from "@fluentui/react-components";
import { useEffect, useId, useRef, useState } from "react";

import {
  type ExportedOutcome,
  type PublicationSummary,
  describeLocalAccountError,
  loadMissionPublications,
  loadOutcomeDocument,
} from "./session";

const useStyles = makeStyles({
  section: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    paddingTop: tokens.spacingVerticalL,
  },
  heading: { fontSize: tokens.fontSizeBase400, marginTop: 0, marginBottom: tokens.spacingVerticalS },
  body: { color: tokens.colorNeutralForeground2, maxWidth: "62ch" },
  table: { width: "100%", borderCollapse: "collapse", marginTop: tokens.spacingVerticalL },
  cell: {
    textAlign: "left",
    ...shorthands.padding(tokens.spacingVerticalS, tokens.spacingHorizontalM),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    verticalAlign: "top",
  },
  // A digest cell holds 64 unbroken hex characters, and an unbreakable word is a table column's
  // MIN-content width. Two of them put the Document column -- and the only control in the table --
  // past the right edge of a 1280px window, so reaching the button needed horizontal page travel
  // (measured in B22, screenshots 17-outcomes.png and 29-showversion.png).
  //
  // `anywhere` and NOT `break-word`: the two wrap identically once a line is being laid out, but
  // only `anywhere` lets the browser count a broken line when it computes min-content. With
  // `break-word` the column still reserves all 64 characters and the table still overflows. The
  // digits are all still here -- they wrap onto more lines, nothing is abbreviated or hidden.
  digest: {
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    overflowWrap: "anywhere",
  },
  actions: {
    display: "flex",
    alignItems: "center",
    columnGap: tokens.spacingHorizontalM,
    marginTop: tokens.spacingVerticalM,
    flexWrap: "wrap",
  },
  document: {
    marginTop: tokens.spacingVerticalL,
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    backgroundColor: tokens.colorNeutralBackground3,
    fontFamily: tokens.fontFamilyMonospace,
    fontSize: tokens.fontSizeBase200,
    whiteSpace: "pre-wrap",
    overflowX: "auto",
  },
});

/// The exact stored bytes, decoded for display and NOTHING else.
///
/// No Markdown to HTML: the surface has never done that for any other record, and doing it here
/// would put a renderer between an operator and bytes whose whole point is that they are the exact
/// ones that were published and evaluated.
function decode(bytes: number[]): string {
  return new TextDecoder().decode(Uint8Array.from(bytes));
}

/// What the operator is told when the copy does not happen.
///
/// Fixed copy, never the thrown value: a `DOMException` string is about a browser API, not about
/// what the operator can now do. Two things it deliberately does NOT say. It does not claim the
/// clipboard was unreachable -- a `writeText` rejection does not establish reachability, only that
/// this write did not succeed. And it does not say "above": this status renders in the action row,
/// which sits BEFORE the document, so "above" pointed the operator the wrong way.
const CLIPBOARD_COPY_FAILED =
  "Beacon could not copy the Markdown. You can select and copy the displayed document.";

export function MissionOutcomes({ missionId }: { missionId: string }): React.JSX.Element {
  const styles = useStyles();
  const headingId = useId();
  const [publications, setPublications] = useState<PublicationSummary[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [document, setDocument] = useState<ExportedOutcome | null>(null);
  // WHICH read failed, not just that one did. A history failure and a document failure are
  // different states: the first means the version list never arrived, the second means one selected
  // version's bytes did not. Retry below is offered for the document one ONLY, so it can never
  // silently re-ask for history or imply nothing is published.
  const [failure, setFailure] = useState<{ text: string; source: "history" | "document" } | null>(
    null,
  );
  // Bumped by the operator's explicit Retry, and nothing else. It is a dependency of the document
  // effect, so a retry re-runs exactly that read -- same Mission, same selected version -- through
  // the same `loadOutcomeDocument` the first attempt used. No generic retry and no poll.
  const [documentAttempt, setDocumentAttempt] = useState(0);
  // The same idiom as `documentAttempt`, for the OTHER read. Bumped by the operator's explicit
  // Retry and nothing else, and a dependency of the history effect, so a retry re-runs exactly that
  // read -- same Mission, same `loadMissionPublications` -- with no timer, poll or generic retry.
  const [historyAttempt, setHistoryAttempt] = useState(0);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  // Attempt identity, NOT selection identity. A `missionId#version` token is stable across
  // 1 -> 2 -> 1, so an older attempt's token matched again on return and could release a newer
  // pending one: the button re-enabled and stale feedback replaced the live attempt's state
  // (Astra's B11 P1). A monotonic counter cannot collide with itself, so returning to a version
  // never resurrects an attempt that belongs to an earlier visit.
  const nextAttempt = useRef(0);
  // The one attempt currently permitted to write status or release the control. `0` means none:
  // a selection change invalidates every attempt still in flight.
  const activeAttempt = useRef(0);

  useEffect(() => {
    activeAttempt.current = 0;
    setCopyStatus(null);
    setCopying(false);
  }, [missionId, selected]);

  async function copyMarkdown(shown: ExportedOutcome): Promise<void> {
    if (copying) return;
    nextAttempt.current += 1;
    const attempt = nextAttempt.current;
    activeAttempt.current = attempt;
    // Decoded from the bytes ALREADY on screen: the same array the document renders, so the copy
    // cannot be a different version, a re-render, or a fresh fetch of the newest.
    const text = decode(shown.bytes);
    setCopying(true);
    setCopyStatus(null);
    try {
      if (!navigator.clipboard?.writeText) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(text);
      if (activeAttempt.current !== attempt) return;
      setCopyStatus(`Copied version ${shown.publicationVersion}'s stored Markdown.`);
    } catch {
      if (activeAttempt.current !== attempt) return;
      setCopyStatus(CLIPBOARD_COPY_FAILED);
    } finally {
      if (activeAttempt.current === attempt) setCopying(false);
    }
  }

  useEffect(() => {
    let cancelled = false;
    setPublications(null);
    setSelected(null);
    setDocument(null);
    setFailure(null);
    loadMissionPublications(missionId)
      .then((loaded) => {
        if (cancelled) return;
        setPublications(loaded);
        setSelected(loaded.length > 0 ? loaded[0].publicationVersion : null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setFailure({ text: describeLocalAccountError(cause), source: "history" });
      });
    // The existing lifetime guard is what makes the retry safe: the cleanup runs before the next
    // attempt starts, so a promise from an earlier attempt -- or an earlier Mission -- finds
    // `cancelled` true and writes nothing.
    return () => {
      cancelled = true;
    };
  }, [missionId, historyAttempt]);

  // The selected VERSION is what is asked for, every time. Never "the newest": a superseded version
  // is readable at its own number (R12.11), and a view that quietly showed the current document for
  // every row would be showing bytes no Evaluation of that version was ever about.
  useEffect(() => {
    if (selected === null) return undefined;
    let cancelled = false;
    setDocument(null);
    // Only the stale DOCUMENT failure is cleared. A history failure is not this read's to answer,
    // and wiping it here would leave the operator with no record that the list never arrived.
    setFailure((current) => (current?.source === "document" ? null : current));
    loadOutcomeDocument(missionId, selected)
      .then((loaded) => {
        if (!cancelled) setDocument(loaded);
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          setFailure({ text: describeLocalAccountError(cause), source: "document" });
        }
      });
    // The existing lifetime guard does the work a retry needs: the cleanup runs before the next
    // attempt starts, so a promise from an earlier attempt -- or an earlier selection, or an
    // earlier Mission -- finds `cancelled` true and writes nothing.
    return () => {
      cancelled = true;
    };
  }, [missionId, selected, documentAttempt]);

  return (
    <section className={styles.section} aria-labelledby={headingId}>
      <h3 className={styles.heading} id={headingId}>
        Outcome Documents
      </h3>
      <p className={styles.body}>
        Every published version of this Mission&apos;s Outcome Document. The document below is the
        exact stored Markdown for the version selected, byte for byte.
      </p>
      {failure !== null && <p role="alert">{failure.text}</p>}
      {failure?.source === "document" && selected !== null && (
        <button
          type="button"
          onClick={() => {
            setFailure(null);
            setDocumentAttempt((attempt) => attempt + 1);
          }}
        >
          Retry version {selected}
        </button>
      )}
      {failure?.source === "history" && (
        <button
          type="button"
          onClick={() => {
            setFailure(null);
            setHistoryAttempt((attempt) => attempt + 1);
          }}
        >
          Retry publication history
        </button>
      )}
      {selected !== null && document === null && failure === null && (
        <p role="status">Loading version {selected}&apos;s stored Markdown…</p>
      )}
      {/* A history read that has not answered yet is neither a failure nor an empty Mission, and
          saying nothing is what made a failed list look like one. The same line covers the first
          load and a retry: they are the same read, and inventing a second wording would be
          claiming a distinction the reader does not have. While it shows, the Retry control is
          gone -- the failure it renders for is cleared -- so a second click cannot start a second
          live request. */}
      {publications === null && failure === null && (
        <p role="status">Loading this Mission&apos;s publication history…</p>
      )}
      {publications !== null && publications.length === 0 && (
        <p className={styles.body}>This Mission has no published Outcome.</p>
      )}
      {publications !== null && publications.length > 0 && (
        <table className={styles.table}>
          <caption className={styles.body}>Publication history, newest first</caption>
          <thead>
            <tr>
              <th className={styles.cell} scope="col">
                Version
              </th>
              <th className={styles.cell} scope="col">
                Candidate digest
              </th>
              <th className={styles.cell} scope="col">
                Evaluation digest
              </th>
              <th className={styles.cell} scope="col">
                Document
              </th>
            </tr>
          </thead>
          <tbody>
            {publications.map((publication) => (
              <tr key={publication.publicationId}>
                <th className={styles.cell} scope="row">
                  {publication.publicationVersion}
                  {publication.isCurrent ? " (current)" : ""}
                </th>
                <td className={`${styles.cell} ${styles.digest}`}>
                  {publication.candidateArtifactDigest}
                </td>
                <td className={`${styles.cell} ${styles.digest}`}>
                  {publication.evaluationDigest}
                </td>
                <td className={styles.cell}>
                  <button
                    type="button"
                    onClick={() => setSelected(publication.publicationVersion)}
                    aria-pressed={selected === publication.publicationVersion}
                  >
                    Show version {publication.publicationVersion}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {document !== null && (
        <>
          <p className={styles.body}>
            Version {document.publicationVersion} of {document.publicationVersionCount} ·{" "}
            <span className={styles.digest}>{document.digest.value}</span> · {document.mediaType}
          </p>
          <div className={styles.actions}>
            <button type="button" onClick={() => void copyMarkdown(document)} disabled={copying}>
              Copy Markdown
            </button>
            {copyStatus !== null && <span role="status">{copyStatus}</span>}
          </div>
          <pre className={styles.document} aria-label="Outcome Document">
            {decode(document.bytes)}
          </pre>
        </>
      )}
    </section>
  );
}
