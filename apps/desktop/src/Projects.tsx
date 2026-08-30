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
  ProjectCharter,
  ProjectCharterDraft,
  ProjectDraft,
  ProjectPolicy,
  ProjectPolicyDraft,
  PreflightReceipt,
  ProjectRepository,
  ProjectSummary,
  ProjectView,
  MissionDraft,
  MissionView,
  ResidentAgentView,
  ResidentConfiguration,
  RepositoryDraft,
  TeamDraft,
  TeamView,
} from "./session";
import { ProjectMissions } from "./ProjectMissions";
import { ProjectPolicySection } from "./ProjectPolicy";
import { CheckPreflightSection } from "./CheckPreflight";
import { ProjectRepositorySection } from "./ProjectRepository";
import { ProjectTeams } from "./ProjectTeams";
import { SampleProject } from "./SampleProject";

const useStyles = makeStyles({
  section: { width: "min(100%, 920px)" },
  header: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalL,
    rowGap: tokens.spacingVerticalM,
    flexWrap: "wrap",
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
  recordHeader: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalL,
    rowGap: tokens.spacingVerticalM,
    flexWrap: "wrap",
  },
  history: {
    marginTop: tokens.spacingVerticalXXL,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    paddingTop: tokens.spacingVerticalL,
  },
  historySummary: {
    cursor: "pointer",
    fontWeight: tokens.fontWeightSemibold,
    minHeight: "44px",
    display: "flex",
    alignItems: "center",
  },
  historyList: {
    display: "grid",
    rowGap: tokens.spacingVerticalL,
    paddingLeft: tokens.spacingHorizontalXL,
  },
  historyEntry: {
    paddingLeft: tokens.spacingHorizontalS,
  },
});

export function ProjectsWorkspace({
  projects,
  selectedProject,
  charterHistory,
  projectPolicy,
  projectPolicyHistory,
  projectRepository,
  projectRepositoryHistory,
  teams,
  missions,
  residents,
  teamConfigurations,
  composerOpen,
  charterComposerOpen,
  policyComposerOpen,
  repositoryComposerOpen,
  preflightReceipt,
  teamComposerOpen,
  missionComposerOpen,
  busy,
  error,
  onCreate,
  onCancelCreate,
  onOpenProject,
  onSubmitProject,
  onReviseCharter,
  onCancelCharterRevision,
  onSubmitCharterRevision,
  onOpenPolicyComposer,
  onCancelPolicyComposer,
  onSubmitProjectPolicy,
  onOpenRepositoryComposer,
  onCancelRepositoryComposer,
  onSubmitProjectRepository,
  onPreflightChecks,
  onOpenTeamComposer,
  onCancelTeamComposer,
  onSubmitTeam,
  onOpenMissionComposer,
  onCancelMissionComposer,
  onSubmitMission,
  onSampleProjectInstalled,
}: {
  projects: ProjectSummary[] | null;
  selectedProject: ProjectView | null;
  charterHistory: ProjectCharter[] | null;
  projectPolicy: ProjectPolicy | null;
  projectPolicyHistory: ProjectPolicy[] | null;
  projectRepository: ProjectRepository | null;
  projectRepositoryHistory: ProjectRepository[] | null;
  teams: TeamView[] | null;
  missions: MissionView[] | null;
  residents: ResidentAgentView[] | null;
  teamConfigurations: ResidentConfiguration[] | null;
  composerOpen: boolean;
  charterComposerOpen: boolean;
  policyComposerOpen: boolean;
  repositoryComposerOpen: boolean;
  preflightReceipt: PreflightReceipt | null;
  teamComposerOpen: boolean;
  missionComposerOpen: boolean;
  busy: boolean;
  error: string;
  onCreate: () => void;
  onCancelCreate: () => void;
  onOpenProject: (projectId: string) => void;
  onSubmitProject: (draft: ProjectDraft) => void;
  onReviseCharter: () => void;
  onCancelCharterRevision: () => void;
  onSubmitCharterRevision: (draft: ProjectCharterDraft) => void;
  onOpenPolicyComposer: () => void;
  onCancelPolicyComposer: () => void;
  onSubmitProjectPolicy: (draft: ProjectPolicyDraft) => void;
  onOpenRepositoryComposer: () => void;
  onCancelRepositoryComposer: () => void;
  onSubmitProjectRepository: (draft: RepositoryDraft) => void;
  onPreflightChecks: () => void;
  onOpenTeamComposer: () => void;
  onCancelTeamComposer: () => void;
  onSubmitTeam: (draft: TeamDraft) => void;
  onOpenMissionComposer: () => void;
  onCancelMissionComposer: () => void;
  onSubmitMission: (draft: MissionDraft) => void;
  onSampleProjectInstalled: (projectId: string) => void;
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
        {!composerOpen &&
          !charterComposerOpen &&
          !policyComposerOpen &&
          !repositoryComposerOpen &&
          !teamComposerOpen &&
          !missionComposerOpen && (
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
      {!selectedProject &&
        !composerOpen &&
        !charterComposerOpen &&
        !policyComposerOpen &&
        !repositoryComposerOpen &&
        !teamComposerOpen &&
        !missionComposerOpen && <SampleProject onInstalled={onSampleProjectInstalled} />}
      {composerOpen ? (
        <ProjectComposer busy={busy} onCancel={onCancelCreate} onSubmit={onSubmitProject} />
      ) : charterComposerOpen && selectedProject ? (
        <CharterRevisionComposer
          key={selectedProject.charter.revision}
          project={selectedProject}
          busy={busy}
          onCancel={onCancelCharterRevision}
          onSubmit={onSubmitCharterRevision}
        />
      ) : selectedProject ? (
        <ProjectRecord
          project={selectedProject}
          history={charterHistory}
          policy={projectPolicy}
          policyHistory={projectPolicyHistory}
          repository={projectRepository}
          repositoryHistory={projectRepositoryHistory}
          teams={teams}
          missions={missions}
          residents={residents}
          teamConfigurations={teamConfigurations}
          teamComposerOpen={teamComposerOpen}
          policyComposerOpen={policyComposerOpen}
          repositoryComposerOpen={repositoryComposerOpen}
          preflightReceipt={preflightReceipt}
          missionComposerOpen={missionComposerOpen}
          busy={busy}
          onRevise={onReviseCharter}
          onOpenPolicyComposer={onOpenPolicyComposer}
          onCancelPolicyComposer={onCancelPolicyComposer}
          onSubmitProjectPolicy={onSubmitProjectPolicy}
          onOpenRepositoryComposer={onOpenRepositoryComposer}
          onCancelRepositoryComposer={onCancelRepositoryComposer}
          onSubmitProjectRepository={onSubmitProjectRepository}
          onPreflightChecks={onPreflightChecks}
          onOpenTeamComposer={onOpenTeamComposer}
          onCancelTeamComposer={onCancelTeamComposer}
          onSubmitTeam={onSubmitTeam}
          onOpenMissionComposer={onOpenMissionComposer}
          onCancelMissionComposer={onCancelMissionComposer}
          onSubmitMission={onSubmitMission}
        />
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
  const [charter, setCharter] = useState<CharterFormState>(emptyCharterForm);

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit({
      name: name.trim(),
      charter: draftFromForm(charter),
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
      <CharterFields value={charter} onChange={setCharter} />
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

function CharterRevisionComposer({
  project,
  busy,
  onCancel,
  onSubmit,
}: {
  project: ProjectView;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: ProjectCharterDraft) => void;
}) {
  const classes = useStyles();
  const [charter, setCharter] = useState<CharterFormState>(formFromCharter(project.charter));
  const nextRevision = project.charter.revision + 1;

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit(draftFromForm(charter));
  }

  return (
    <form className={classes.form} aria-label="Revise Project Charter" onSubmit={submit}>
      <div>
        <p className={classes.meta}>Preparing Charter revision {nextRevision}</p>
        <h2 className={classes.sectionHeading}>{project.name}</h2>
        <p className={classes.body}>
          Publishing preserves the current Charter and advances the Project to a new immutable
          revision.
        </p>
      </div>
      <CharterFields value={charter} onChange={setCharter} />
      <CharterGuard />
      <p className={classes.body}>
        This changes Project intent only. It starts no Mission or Run and grants no spending or
        external effects.
      </p>
      <div className={classes.actions}>
        <Button appearance="primary" type="submit" disabled={busy}>
          Publish revision {nextRevision}
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

type CharterFormState = {
  purpose: string;
  desiredOutcomes: string;
  constraints: string;
  prohibitions: string;
  autonomyProfile: AutonomyProfile;
};

const emptyCharterForm: CharterFormState = {
  purpose: "",
  desiredOutcomes: "",
  constraints: "",
  prohibitions: "",
  autonomyProfile: "supervised",
};

function CharterFields({
  value,
  onChange,
}: {
  value: CharterFormState;
  onChange: (value: CharterFormState) => void;
}) {
  return (
    <>
      <Field label="Purpose" hint="Why this Project exists." required>
        <Textarea
          aria-label="Purpose"
          value={value.purpose}
          maxLength={2_000}
          resize="vertical"
          onChange={(_, data) => onChange({ ...value, purpose: data.value })}
          required
        />
      </Field>
      <Field label="Desired outcomes" hint="One concrete outcome per line, up to 20." required>
        <Textarea
          aria-label="Desired outcomes"
          value={value.desiredOutcomes}
          resize="vertical"
          onChange={(_, data) => onChange({ ...value, desiredOutcomes: data.value })}
          required
        />
      </Field>
      <Field label="Constraints" hint="Optional. One boundary per line, up to 20.">
        <Textarea
          aria-label="Constraints"
          value={value.constraints}
          resize="vertical"
          onChange={(_, data) => onChange({ ...value, constraints: data.value })}
        />
      </Field>
      <Field label="Prohibitions" hint="Optional. One forbidden action per line, up to 20.">
        <Textarea
          aria-label="Prohibitions"
          value={value.prohibitions}
          resize="vertical"
          onChange={(_, data) => onChange({ ...value, prohibitions: data.value })}
        />
      </Field>
      <Field label="Autonomy profile">
        <RadioGroup
          value={value.autonomyProfile}
          onChange={(_, data) =>
            onChange({ ...value, autonomyProfile: data.value as AutonomyProfile })
          }
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
    </>
  );
}

function formFromCharter(charter: ProjectCharter): CharterFormState {
  return {
    purpose: charter.purpose,
    desiredOutcomes: charter.desiredOutcomes.join("\n"),
    constraints: charter.constraints.join("\n"),
    prohibitions: charter.prohibitions.join("\n"),
    autonomyProfile: charter.autonomyProfile,
  };
}

function draftFromForm(value: CharterFormState): ProjectCharterDraft {
  return {
    purpose: value.purpose.trim(),
    desiredOutcomes: lines(value.desiredOutcomes),
    constraints: lines(value.constraints),
    prohibitions: lines(value.prohibitions),
    autonomyProfile: value.autonomyProfile,
  };
}

function ProjectRecord({
  project,
  history,
  policy,
  policyHistory,
  repository,
  repositoryHistory,
  teams,
  missions,
  residents,
  teamConfigurations,
  teamComposerOpen,
  policyComposerOpen,
  repositoryComposerOpen,
  preflightReceipt,
  missionComposerOpen,
  busy,
  onRevise,
  onOpenPolicyComposer,
  onCancelPolicyComposer,
  onSubmitProjectPolicy,
  onOpenRepositoryComposer,
  onCancelRepositoryComposer,
  onSubmitProjectRepository,
  onPreflightChecks,
  onOpenTeamComposer,
  onCancelTeamComposer,
  onSubmitTeam,
  onOpenMissionComposer,
  onCancelMissionComposer,
  onSubmitMission,
}: {
  project: ProjectView;
  history: ProjectCharter[] | null;
  policy: ProjectPolicy | null;
  policyHistory: ProjectPolicy[] | null;
  repository: ProjectRepository | null;
  repositoryHistory: ProjectRepository[] | null;
  teams: TeamView[] | null;
  missions: MissionView[] | null;
  residents: ResidentAgentView[] | null;
  teamConfigurations: ResidentConfiguration[] | null;
  teamComposerOpen: boolean;
  policyComposerOpen: boolean;
  repositoryComposerOpen: boolean;
  preflightReceipt: PreflightReceipt | null;
  missionComposerOpen: boolean;
  busy: boolean;
  onRevise: () => void;
  onOpenPolicyComposer: () => void;
  onCancelPolicyComposer: () => void;
  onSubmitProjectPolicy: (draft: ProjectPolicyDraft) => void;
  onOpenRepositoryComposer: () => void;
  onCancelRepositoryComposer: () => void;
  onSubmitProjectRepository: (draft: RepositoryDraft) => void;
  onPreflightChecks: () => void;
  onOpenTeamComposer: () => void;
  onCancelTeamComposer: () => void;
  onSubmitTeam: (draft: TeamDraft) => void;
  onOpenMissionComposer: () => void;
  onCancelMissionComposer: () => void;
  onSubmitMission: (draft: MissionDraft) => void;
}) {
  const classes = useStyles();
  const charter = project.charter;
  return (
    <article aria-labelledby="project-name">
      <div className={classes.recordHeader}>
        <div>
          <p className={classes.meta}>Charter revision {charter.revision}</p>
          <h2 id="project-name" className={classes.heading}>
            {project.name}
          </h2>
        </div>
        <Button onClick={onRevise} disabled={busy}>
          Revise Charter
        </Button>
      </div>
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
      <section className={classes.history} aria-label="Charter revision history">
        <details>
          <summary className={classes.historySummary}>
            {history === null
              ? "Loading revision history"
              : `${history.length} preserved ${history.length === 1 ? "revision" : "revisions"}`}
          </summary>
          {history && (
            <ol className={classes.historyList}>
              {[...history].reverse().map((revision) => (
                <li className={classes.historyEntry} key={revision.revision}>
                  <p className={classes.guardLabel}>
                    Revision {revision.revision}
                    {revision.revision === charter.revision ? " (current)" : ""}
                  </p>
                  <p className={classes.guardDetail}>{revision.purpose}</p>
                  <p className={classes.meta}>{formatAuthoredAt(revision.authoredAtMs)}</p>
                </li>
              ))}
            </ol>
          )}
        </details>
      </section>
      <ProjectPolicySection
        projectId={project.id}
        charterRevision={charter.revision}
        policy={policy}
        history={policyHistory}
        composerOpen={policyComposerOpen}
        busy={busy}
        onOpenComposer={onOpenPolicyComposer}
        onCancel={onCancelPolicyComposer}
        onSubmit={onSubmitProjectPolicy}
      />
      <ProjectRepositorySection
        projectId={project.id}
        charterRevision={charter.revision}
        repository={repository}
        history={repositoryHistory}
        composerOpen={repositoryComposerOpen}
        busy={busy}
        onOpenComposer={onOpenRepositoryComposer}
        onCancel={onCancelRepositoryComposer}
        onSubmit={onSubmitProjectRepository}
      />
      <CheckPreflightSection
        receipt={preflightReceipt}
        busy={busy}
        canPreflight={repository !== null}
        onPreflight={onPreflightChecks}
      />
      <ProjectTeams
        projectId={project.id}
        teams={teams}
        residents={residents}
        configurations={teamConfigurations}
        composerOpen={teamComposerOpen}
        busy={busy}
        onOpenComposer={onOpenTeamComposer}
        onCancel={onCancelTeamComposer}
        onSubmit={onSubmitTeam}
      />
      <ProjectMissions
        project={project}
        teams={teams}
        missions={missions}
        composerOpen={missionComposerOpen}
        busy={busy}
        onOpenComposer={onOpenMissionComposer}
        onCancel={onCancelMissionComposer}
        onSubmit={onSubmitMission}
      />
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

function formatAuthoredAt(timestampMs: number): string {
  const authoredAt = new Date(timestampMs);
  if (Number.isNaN(authoredAt.getTime())) {
    return "Recorded time unavailable";
  }

  return new Intl.DateTimeFormat(undefined, {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(authoredAt);
}
