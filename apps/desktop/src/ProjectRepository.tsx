import {
  Button,
  Field,
  Input,
  Radio,
  RadioGroup,
  Textarea,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useState } from "react";

import type { ProjectRepository, RepositoryDraft, RepositorySourceKind } from "./session";

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
  summaryDetail: { margin: 0, color: tokens.colorNeutralForeground2, wordBreak: "break-word" },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalL,
    maxWidth: "680px",
    marginTop: tokens.spacingVerticalL,
  },
  checkRow: {
    display: "grid",
    rowGap: tokens.spacingVerticalS,
    ...shorthands.padding(tokens.spacingVerticalM, 0),
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  actions: { display: "flex", columnGap: tokens.spacingHorizontalS },
  history: { marginTop: tokens.spacingVerticalL },
  historySummary: {
    cursor: "pointer",
    fontWeight: tokens.fontWeightSemibold,
    minHeight: "44px",
    display: "flex",
    alignItems: "center",
  },
});

const sourceKindChoices: readonly { value: RepositorySourceKind; label: string }[] = [
  { value: "local_path", label: "Local path" },
  { value: "git_url", label: "Git URL" },
];

const declarationBoundary = "Declared, not verified — Beacon has not opened this repository.";

type CheckRow = { name: string; argumentsText: string; workingDirectory: string };

export function ProjectRepositorySection({
  projectId,
  charterRevision,
  repository,
  history,
  composerOpen,
  busy,
  onOpenComposer,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  charterRevision: number;
  repository: ProjectRepository | null;
  history: ProjectRepository[] | null;
  composerOpen: boolean;
  busy: boolean;
  onOpenComposer: () => void;
  onCancel: () => void;
  onSubmit: (draft: RepositoryDraft) => void;
}) {
  const classes = useStyles();
  const nextRevision = (repository?.revision ?? 0) + 1;

  return (
    <section className={classes.section} aria-labelledby="project-repository-heading">
      <div className={classes.header}>
        <div>
          <h3 id="project-repository-heading" className={classes.heading}>
            Repository Binding
          </h3>
          <p className={classes.body}>
            The repository this Project works on, its pinned base revision, and the deterministic
            checks it already carries. Binding records a declaration: nothing is cloned, fetched, or
            run.
          </p>
        </div>
        {!composerOpen && (
          <Button onClick={onOpenComposer} disabled={busy}>
            {repository ? "Revise Repository Binding" : "Bind Repository"}
          </Button>
        )}
      </div>

      {composerOpen ? (
        <ProjectRepositoryComposer
          key={`${projectId}:${repository?.revision ?? 0}`}
          repository={repository}
          nextRevision={nextRevision}
          busy={busy}
          onCancel={onCancel}
          onSubmit={onSubmit}
        />
      ) : repository === null ? (
        <p className={classes.body}>No repository has been bound to this Project.</p>
      ) : (
        <>
          <p className={classes.meta}>
            Repository revision {repository.revision} · Charter revision{" "}
            {repository.charterRevision}
          </p>
          {!repository.appliesToCurrentCharter && (
            <p className={classes.warning}>
              The Charter is now revision {charterRevision}. Bind a new Repository revision before
              planning work against it.
            </p>
          )}
          <div className={classes.summary} aria-label="Repository binding">
            <div>
              <p className={classes.summaryHeading}>
                {sourceKindLabel(repository.sourceKind)} source
              </p>
              <p className={classes.summaryDetail}>{repository.source}</p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Pinned base revision</p>
              <p className={classes.summaryDetail}>{repository.baseRevision}</p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Declared checks</p>
              <p className={classes.summaryDetail}>
                {repository.declaredChecks
                  .map((check) => `${check.name}: ${check.argv.join(" ")} (in ${check.workingDirectory})`)
                  .join(" · ")}
              </p>
            </div>
            <div>
              <p className={classes.summaryHeading}>Not verified</p>
              <p className={classes.summaryDetail}>{declarationBoundary}</p>
            </div>
          </div>
          {history !== null && history.length > 0 && (
            <details className={classes.history}>
              <summary className={classes.historySummary}>
                {history.length} preserved Repository{" "}
                {history.length === 1 ? "revision" : "revisions"}
              </summary>
              <ol>
                {[...history].reverse().map((revision) => (
                  <li key={revision.revision}>
                    Revision {revision.revision} · Charter revision {revision.charterRevision} ·{" "}
                    {revision.baseRevision.slice(0, 7)}
                    {revision.revision === repository.revision ? " (current)" : ""}
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

function ProjectRepositoryComposer({
  repository,
  nextRevision,
  busy,
  onCancel,
  onSubmit,
}: {
  repository: ProjectRepository | null;
  nextRevision: number;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: RepositoryDraft) => void;
}) {
  const classes = useStyles();
  const [sourceKind, setSourceKind] = useState<RepositorySourceKind>(
    repository?.sourceKind ?? "local_path",
  );
  const [source, setSource] = useState(repository?.source ?? "");
  const [baseRevision, setBaseRevision] = useState(repository?.baseRevision ?? "");
  const [checkRows, setCheckRows] = useState<CheckRow[]>(() =>
    repository === null
      ? [{ name: "", argumentsText: "", workingDirectory: "." }]
      : repository.declaredChecks.map((check) => ({
          name: check.name,
          argumentsText: check.argv.join("\n"),
          workingDirectory: check.workingDirectory,
        })),
  );

  function updateRow(index: number, patch: Partial<CheckRow>) {
    setCheckRows(checkRows.map((row, position) => (position === index ? { ...row, ...patch } : row)));
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit({
      sourceKind,
      source: source.trim(),
      baseRevision: baseRevision.trim(),
      declaredChecks: [...checkRows]
        .map((row) => ({
          name: row.name.trim(),
          argv: argumentsOf(row.argumentsText),
          workingDirectory: row.workingDirectory.trim(),
        }))
        .sort((left, right) => (left.name < right.name ? -1 : left.name > right.name ? 1 : 0)),
    });
  }

  return (
    <form className={classes.form} aria-label="Bind Repository" onSubmit={submit}>
      <fieldset>
        <legend>Repository</legend>
        <RadioGroup
          aria-label="Source kind"
          value={sourceKind}
          disabled={busy}
          onChange={(_, data) => setSourceKind(data.value as RepositorySourceKind)}
        >
          {sourceKindChoices.map((choice) => (
            <Radio key={choice.value} value={choice.value} label={choice.label} />
          ))}
        </RadioGroup>
        <Field label="Repository source" required>
          <Input
            aria-label="Repository source"
            value={source}
            disabled={busy}
            required
            onChange={(_, data) => setSource(data.value)}
          />
        </Field>
        <Field label="Pinned base revision" required>
          <Input
            aria-label="Pinned base revision"
            value={baseRevision}
            disabled={busy}
            required
            onChange={(_, data) => setBaseRevision(data.value)}
          />
        </Field>
        <p className={classes.body}>{declarationBoundary}</p>
      </fieldset>

      <fieldset>
        <legend>Declared checks</legend>
        <p className={classes.body}>
          Name each deterministic check and its arguments, one argument per line. Beacon stores the
          argument vector; it never runs a shell.
        </p>
        {checkRows.map((row, index) => (
          <div className={classes.checkRow} key={index}>
            <Field label={`Check ${index + 1} name`} required>
              <Input
                aria-label={`Check ${index + 1} name`}
                value={row.name}
                disabled={busy}
                required
                onChange={(_, data) => updateRow(index, { name: data.value })}
              />
            </Field>
            <Field label={`Check ${index + 1} arguments`} required>
              <Textarea
                aria-label={`Check ${index + 1} arguments`}
                value={row.argumentsText}
                disabled={busy}
                required
                onChange={(_, data) => updateRow(index, { argumentsText: data.value })}
              />
            </Field>
            <Field label={`Check ${index + 1} working directory`} required>
              <Input
                aria-label={`Check ${index + 1} working directory`}
                value={row.workingDirectory}
                disabled={busy}
                required
                onChange={(_, data) => updateRow(index, { workingDirectory: data.value })}
              />
            </Field>
            {checkRows.length > 1 && (
              <Button
                type="button"
                disabled={busy}
                onClick={() => setCheckRows(checkRows.filter((_, position) => position !== index))}
              >
                Remove check {index + 1}
              </Button>
            )}
          </div>
        ))}
        <Button
          type="button"
          disabled={busy}
          onClick={() =>
            setCheckRows([...checkRows, { name: "", argumentsText: "", workingDirectory: "." }])
          }
        >
          Add another check
        </Button>
      </fieldset>

      <div className={classes.actions}>
        <Button
          appearance="primary"
          type="submit"
          disabled={busy || !isSubmittable(source, baseRevision, checkRows)}
        >
          Bind Repository revision {nextRevision}
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function argumentsOf(argumentsText: string): string[] {
  return argumentsText
    .split("\n")
    .map((argument) => argument.trim())
    .filter((argument) => argument.length > 0);
}

function isSubmittable(source: string, baseRevision: string, checkRows: CheckRow[]): boolean {
  return (
    source.trim().length > 0 &&
    baseRevision.trim().length > 0 &&
    checkRows.length > 0 &&
    checkRows.every(
      (row) =>
        row.name.trim().length > 0 &&
        row.workingDirectory.trim().length > 0 &&
        argumentsOf(row.argumentsText).length > 0,
    )
  );
}

function sourceKindLabel(kind: RepositorySourceKind): string {
  return sourceKindChoices.find((choice) => choice.value === kind)?.label ?? kind;
}
