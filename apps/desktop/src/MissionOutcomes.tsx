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
  digest: { fontFamily: tokens.fontFamilyMonospace, fontSize: tokens.fontSizeBase200 },
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

/// What the operator is told when the clipboard cannot be reached.
///
/// Fixed copy, never the thrown value: a `DOMException` string is about a browser API, not about
/// what the operator can now do. The sentence names the fallback that is actually on screen -- the
/// exact bytes are rendered above it -- which is the same shape the object-link copy already uses
/// (`App.tsx:386-397`), where the text is placed in a visible field before the attempt.
const CLIPBOARD_UNREACHABLE =
  "Beacon could not reach the clipboard. The document above is the exact text; select and copy it.";

export function MissionOutcomes({ missionId }: { missionId: string }): React.JSX.Element {
  const styles = useStyles();
  const headingId = useId();
  const [publications, setPublications] = useState<PublicationSummary[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [document, setDocument] = useState<ExportedOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  const [copying, setCopying] = useState(false);
  // Which document any in-flight copy was started for. A clipboard promise that settles after the
  // operator moved on must not drop a result beside bytes it was never about.
  const selectionRef = useRef<string>("");

  useEffect(() => {
    selectionRef.current = `${missionId}#${selected ?? "none"}`;
    setCopyStatus(null);
    setCopying(false);
  }, [missionId, selected]);

  async function copyMarkdown(shown: ExportedOutcome): Promise<void> {
    if (copying) return;
    const startedFor = selectionRef.current;
    // Decoded from the bytes ALREADY on screen: the same array the document renders, so the copy
    // cannot be a different version, a re-render, or a fresh fetch of the newest.
    const text = decode(shown.bytes);
    setCopying(true);
    setCopyStatus(null);
    try {
      if (!navigator.clipboard?.writeText) throw new Error("clipboard unavailable");
      await navigator.clipboard.writeText(text);
      if (selectionRef.current !== startedFor) return;
      setCopyStatus(`Copied version ${shown.publicationVersion}'s stored Markdown.`);
    } catch {
      if (selectionRef.current !== startedFor) return;
      setCopyStatus(CLIPBOARD_UNREACHABLE);
    } finally {
      if (selectionRef.current === startedFor) setCopying(false);
    }
  }

  useEffect(() => {
    let cancelled = false;
    setPublications(null);
    setSelected(null);
    setDocument(null);
    setError(null);
    loadMissionPublications(missionId)
      .then((loaded) => {
        if (cancelled) return;
        setPublications(loaded);
        setSelected(loaded.length > 0 ? loaded[0].publicationVersion : null);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(describeLocalAccountError(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [missionId]);

  // The selected VERSION is what is asked for, every time. Never "the newest": a superseded version
  // is readable at its own number (R12.11), and a view that quietly showed the current document for
  // every row would be showing bytes no Evaluation of that version was ever about.
  useEffect(() => {
    if (selected === null) return undefined;
    let cancelled = false;
    setDocument(null);
    setError(null);
    loadOutcomeDocument(missionId, selected)
      .then((loaded) => {
        if (!cancelled) setDocument(loaded);
      })
      .catch((cause: unknown) => {
        if (!cancelled) setError(describeLocalAccountError(cause));
      });
    return () => {
      cancelled = true;
    };
  }, [missionId, selected]);

  return (
    <section className={styles.section} aria-labelledby={headingId}>
      <h3 className={styles.heading} id={headingId}>
        Outcome Documents
      </h3>
      <p className={styles.body}>
        Every published version of this Mission&apos;s Outcome Document. The document below is the
        exact stored Markdown for the version selected, byte for byte.
      </p>
      {error !== null && <p role="alert">{error}</p>}
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
