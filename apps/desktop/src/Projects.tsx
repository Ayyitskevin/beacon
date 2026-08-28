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

import type {
  AutonomyProfile,
  ProjectDraft,
  ProjectSummary,
  ProjectView,
} from "./session";

const useStyles = makeStyles({
  section: { width: "min(100%, 920px)" },
  header: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalL,
    marginBottom: tokens.spacingVerticalXL,
  },
  heading: {
    fontSize: tokens.fontSizeHero800,
    lineHeight: tokens.lineHeightHero800,
    letterSpacing: "-0.03em",
    marginTop: 0,
    marginBottom: tokens.spacingVerticalL,
  },
  body: {
    maxWidth: "58ch",
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase400,
    lineHeight: tokens.lineHeightBase400,
  },
  error: { color: tokens.colorPaletteRedForeground1 },
  empty: {
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalXXL, 0),
  },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalL,
    maxWidth: "680px",
  },
  guard: {
    display: "grid",
    gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
    columnGap: tokens.spacingHorizontalM,
    rowGap: tokens.spacingVerticalS,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalM, 0),
  },
  guardLabel: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  guardDetail: { margin: 0, color: tokens.colorNeutralForeground2 },
  list: {
    display: "grid",
    rowGap: tokens.spacingVerticalXS,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
  },
  listButton: {
    justifyContent: "space-between",
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(0),
    ...shorthands.padding(tokens.spacingVerticalL, tokens.spacingHorizontalS),
  },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  charterSection: {
    marginTop: tokens.spacingVerticalXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    paddingTop: tokens.spacingVerticalL,
  },
  sectionHeading: {
    fontSize: tokens.fontSizeBase400,
    marginTop: 0,
    marginBottom: tokens.spacingVerticalS,
  },
  actions: {
    display: "flex",
    columnGap: tokens.spacingHorizontalS,
    alignItems: "center",
    marginTop: tokens.spacingVerticalS,
  },
});

export function ProjectsWorkspace({
  projects,
  selectedProject,
  composerOpen,
  busy,
  error,
  onCreate,
  onCancelCreate,
  onOpenProject,
  onSubmitProject,
}: {
  projects: ProjectSummary[] | null;
  selectedProject: ProjectView | null;
  composerOpen: boolean;
  busy: boolean;
  error: string;
  onCreate: () => void;
  onCancelCreate: () => void;
  onOpenProject: (projectId: string) => void;
  onSubmitProject: (draft: ProjectDraft) => void;
}) {
  const classes = useStyles();
  return (
    <section className={classes.section} aria-labelledby="projects-heading">
      <div className={classes.header}>
        <div>
          <h1 id="projects-heading" className={classes.heading}>
            Projects
          </h1>
          <p className={classes.body}>
            Durable bodies of work governed by an Operator-authored Charter.
          </p>
        </div>
        {!composerOpen && (
          <Button appearance="primary" onClick={onCreate} disabled={busy}>
            Create Project
          </Button>
        )}
      </div>
      {error && (
        <p className={classes.error} role="alert">
          {error}
        </p>
      )}
      {composerOpen ? (
        <ProjectComposer busy={busy} onCancel={onCancelCreate} onSubmit={onSubmitProject} />
      ) : selectedProject ? (
        <ProjectRecord project={selectedProject} />
      ) : projects === null ? (
        <p role="status">Loading Projects</p>
      ) : projects.length === 0 ? (
        <div className={classes.empty}>
          <h2 className={classes.sectionHeading}>No Projects yet</h2>
          <p className={classes.body}>
            Start with the purpose and boundaries. Creating a Project starts no work.
          </p>
        </div>
      ) : (
        <div className={classes.list} aria-label="Project portfolio">
          {projects.map((project) => (
            <Button
              key={project.id}
              className={classes.listButton}
              appearance="subtle"
              onClick={() => onOpenProject(project.id)}
              disabled={busy}
            >
              <span>{project.name}</span>
              <span className={classes.meta}>
                {project.autonomyProfile} · revision {project.charterRevision}
              </span>
            </Button>
          ))}
        </div>
      )}
    </section>
  );
}

function ProjectComposer({
  busy,
  onCancel,
  onSubmit,
}: {
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: ProjectDraft) => void;
}) {
  const classes = useStyles();
  const [name, setName] = useState("");
  const [purpose, setPurpose] = useState("");
  const [desiredOutcomes, setDesiredOutcomes] = useState("");
  const [constraints, setConstraints] = useState("");
  const [prohibitions, setProhibitions] = useState("");
  const [autonomyProfile, setAutonomyProfile] =
    useState<AutonomyProfile>("supervised");

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit({
      name: name.trim(),
      charter: {
        purpose: purpose.trim(),
        desiredOutcomes: lines(desiredOutcomes),
        constraints: lines(constraints),
        prohibitions: lines(prohibitions),
        autonomyProfile,
      },
    });
  }

  return (
    <form className={classes.form} aria-label="Create Project" onSubmit={submit}>
      <Field label="Project name" required>
        <Input
          aria-label="Project name"
          value={name}
          maxLength={120}
          onChange={(_, data) => setName(data.value)}
          required
        />
      </Field>
      <Field label="Purpose" hint="Why this Project exists." required>
        <Textarea
          aria-label="Purpose"
          value={purpose}
          maxLength={2_000}
          resize="vertical"
          onChange={(_, data) => setPurpose(data.value)}
          required
        />
      </Field>
      <Field label="Desired outcomes" hint="One concrete outcome per line, up to 20." required>
        <Textarea
          aria-label="Desired outcomes"
          value={desiredOutcomes}
          resize="vertical"
          onChange={(_, data) => setDesiredOutcomes(data.value)}
          required
        />
      </Field>
      <Field label="Constraints" hint="Optional. One boundary per line, up to 20.">
        <Textarea
          aria-label="Constraints"
          value={constraints}
          resize="vertical"
          onChange={(_, data) => setConstraints(data.value)}
        />
      </Field>
      <Field label="Prohibitions" hint="Optional. One forbidden action per line, up to 20.">
        <Textarea
          aria-label="Prohibitions"
          value={prohibitions}
          resize="vertical"
          onChange={(_, data) => setProhibitions(data.value)}
        />
      </Field>
      <Field label="Autonomy profile">
        <RadioGroup
          value={autonomyProfile}
          onChange={(_, data) => setAutonomyProfile(data.value as AutonomyProfile)}
        >
          <Radio
            value="supervised"
            label="Supervised — Agents may prepare work; every external effect requires a Ruling."
          />
          <Radio
            value="observe"
            label="Observe — research and evaluation only; no external mutation."
          />
        </RadioGroup>
      </Field>
      <CharterGuard />
      <p className={classes.body}>
        Revision 1 is preserved immutably. Creating it starts no Mission, Run, model, or network
        activity.
      </p>
      <div className={classes.actions}>
        <Button appearance="primary" type="submit" disabled={busy}>
          Create Project
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function ProjectRecord({ project }: { project: ProjectView }) {
  const classes = useStyles();
  const charter = project.charter;
  return (
    <article aria-labelledby="project-name">
      <p className={classes.meta}>Charter revision {charter.revision}</p>
      <h2 id="project-name" className={classes.heading}>
        {project.name}
      </h2>
      <p className={classes.body}>{charter.purpose}</p>
      <section className={classes.charterSection} aria-labelledby="outcomes-heading">
        <h3 id="outcomes-heading" className={classes.sectionHeading}>
          Desired outcomes
        </h3>
        <CharterItems values={charter.desiredOutcomes} />
      </section>
      {charter.constraints.length > 0 && (
        <section className={classes.charterSection} aria-labelledby="constraints-heading">
          <h3 id="constraints-heading" className={classes.sectionHeading}>
            Constraints
          </h3>
          <CharterItems values={charter.constraints} />
        </section>
      )}
      {charter.prohibitions.length > 0 && (
        <section className={classes.charterSection} aria-labelledby="prohibitions-heading">
          <h3 id="prohibitions-heading" className={classes.sectionHeading}>
            Prohibitions
          </h3>
          <CharterItems values={charter.prohibitions} />
        </section>
      )}
      <section className={classes.charterSection} aria-labelledby="authority-heading">
        <h3 id="authority-heading" className={classes.sectionHeading}>
          Authority envelope
        </h3>
        <p className={classes.body}>
          {charter.autonomyProfile === "supervised" ? "Supervised" : "Observe"}
        </p>
        <CharterGuard />
        <p className={classes.body}>No work has started.</p>
      </section>
    </article>
  );
}

function CharterGuard() {
  const classes = useStyles();
  return (
    <div className={classes.guard} aria-label="Initial authority boundaries">
      <div>
        <p className={classes.guardLabel}>No spend</p>
        <p className={classes.guardDetail}>No budget is delegated.</p>
      </div>
      <div>
        <p className={classes.guardLabel}>External effects prohibited</p>
        <p className={classes.guardDetail}>No mutation may leave Beacon.</p>
      </div>
    </div>
  );
}

function CharterItems({ values }: { values: string[] }) {
  return (
    <ul>
      {values.map((value) => (
        <li key={value}>{value}</li>
      ))}
    </ul>
  );
}

function lines(value: string): string[] {
  return value
    .split(/\r?\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
}
