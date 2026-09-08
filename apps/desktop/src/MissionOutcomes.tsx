import { makeStyles, shorthands, tokens } from "@fluentui/react-components";
import { useEffect, useState } from "react";

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

export function MissionOutcomes({ missionId }: { missionId: string }): React.JSX.Element {
  const styles = useStyles();
  const [publications, setPublications] = useState<PublicationSummary[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [document, setDocument] = useState<ExportedOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);

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
    <section className={styles.section} aria-labelledby="mission-outcomes-heading">
      <h3 className={styles.heading} id="mission-outcomes-heading">
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
          <pre className={styles.document} aria-label="Outcome Document">
            {decode(document.bytes)}
          </pre>
        </>
      )}
    </section>
  );
}
