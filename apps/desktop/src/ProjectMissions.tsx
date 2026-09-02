import {
  Button,
  Field,
  Textarea,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useMemo, useState } from "react";

import type { AddressableObjectLink } from "./objectLinks";
import type { MissionDraft, MissionView, ProjectView, TeamView } from "./session";

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
  list: { display: "grid", rowGap: tokens.spacingVerticalM, marginTop: tokens.spacingVerticalL },
  mission: {
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
  },
  missionHeader: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "start",
    columnGap: tokens.spacingHorizontalM,
  },
  outcome: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalM,
    maxWidth: "680px",
    marginTop: tokens.spacingVerticalL,
  },
  select: {
    minHeight: "32px",
    color: tokens.colorNeutralForeground1,
    backgroundColor: tokens.colorNeutralBackground1,
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke1),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
    ...shorthands.padding(0, tokens.spacingHorizontalM),
  },
  actions: { display: "flex", columnGap: tokens.spacingHorizontalS },
});

export function ProjectMissions({
  project,
  teams,
  missions,
  linkedMissionId,
  composerOpen,
  busy,
  onOpenComposer,
  onCancel,
  onSubmit,
  onCopyObjectLink,
}: {
  project: ProjectView;
  teams: TeamView[] | null;
  missions: MissionView[] | null;
  linkedMissionId: string | null;
  composerOpen: boolean;
  busy: boolean;
  onOpenComposer: () => void;
  onCancel: () => void;
  onSubmit: (draft: MissionDraft) => void;
  onCopyObjectLink: (link: AddressableObjectLink) => void;
}) {
  const classes = useStyles();
  const eligibleTeams = useMemo(
    () => (teams ?? []).filter((team) => team.members.every((member) => member.eligibleNow)),
    [teams],
  );

  return (
    <section className={classes.section} aria-labelledby="project-missions-heading">
      <div className={classes.header}>
        <div>
          <h3 id="project-missions-heading" className={classes.heading}>
            Missions
          </h3>
          <p className={classes.body}>
            Finite outcomes owned by one Team. Team Lead selection waits for Policy and Track
            Record evidence.
          </p>
        </div>
        {!composerOpen && (
          <Button
            onClick={onOpenComposer}
            disabled={busy || teams === null || eligibleTeams.length === 0}
          >
            Admit Mission
          </Button>
        )}
      </div>
      {teams !== null && teams.length > 0 && eligibleTeams.length === 0 && (
        <p className={classes.warning}>
          No Team is currently eligible. Restore every member’s admitted configuration first.
        </p>
      )}
      {composerOpen ? (
        <MissionComposer
          project={project}
          teams={eligibleTeams}
          busy={busy}
          onCancel={onCancel}
          onSubmit={onSubmit}
        />
      ) : missions === null ? (
        <p role="status">Loading Missions</p>
      ) : missions.length === 0 ? (
        <p className={classes.body}>No Missions have been admitted for this Project.</p>
      ) : (
        <div className={classes.list} aria-label="Project Missions">
          {missions.map((mission) => (
            <article
              className={classes.mission}
              key={mission.id}
              aria-current={linkedMissionId === mission.id ? "true" : undefined}
            >
              <div className={classes.missionHeader}>
                <p className={classes.outcome}>{mission.desiredOutcome}</p>
                <Button
                  size="small"
                  appearance="subtle"
                  disabled={busy}
                  onClick={() =>
                    onCopyObjectLink({
                      kind: "mission",
                      workspaceId: mission.workspaceId,
                      projectId: mission.projectId,
                      missionId: mission.id,
                    })
                  }
                >
                  Copy Mission link
                </Button>
              </div>
              <p className={classes.meta}>
                Operator admitted · Charter revision {mission.charterRevision}
              </p>
              <p className={classes.body}>
                {mission.successConditions.length}{" "}
                {mission.successConditions.length === 1 ? "success condition" : "success conditions"}
                {" · "}{mission.members.length} {mission.members.length === 1 ? "member" : "members"}
              </p>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function MissionComposer({
  project,
  teams,
  busy,
  onCancel,
  onSubmit,
}: {
  project: ProjectView;
  teams: TeamView[];
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: MissionDraft) => void;
}) {
  const classes = useStyles();
  const [teamId, setTeamId] = useState("");
  const [desiredOutcome, setDesiredOutcome] = useState("");
  const [successConditions, setSuccessConditions] = useState("");
  const [constraints, setConstraints] = useState("");
  const [prohibitions, setProhibitions] = useState("");
  const selectedTeam = teams.find((team) => team.id === teamId);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!selectedTeam) return;
    onSubmit({
      projectId: project.id,
      expectedCharterRevision: project.charter.revision,
      teamId: selectedTeam.id,
      desiredOutcome: desiredOutcome.trim(),
      successConditions: lines(successConditions),
      constraints: lines(constraints),
      prohibitions: lines(prohibitions),
      members: selectedTeam.members.map((member) => ({
        residentId: member.residentId,
        expectedConfigurationId: member.currentConfigurationId,
      })),
    });
  }

  return (
    <form className={classes.form} aria-label="Admit Mission" onSubmit={submit}>
      <Field label="Mission Team" required>
        <select
          className={classes.select}
          aria-label="Mission Team"
          value={teamId}
          onChange={(event) => setTeamId(event.currentTarget.value)}
          required
        >
          <option value="">Select an eligible Team</option>
          {teams.map((team) => (
            <option key={team.id} value={team.id}>
              {team.name}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Desired outcome" required>
        <Textarea
          aria-label="Desired outcome"
          value={desiredOutcome}
          maxLength={2000}
          resize="vertical"
          onChange={(_, data) => setDesiredOutcome(data.value)}
          required
        />
      </Field>
      <Field label="Success conditions" hint="One observable condition per line, up to 20." required>
        <Textarea
          aria-label="Success conditions"
          value={successConditions}
          resize="vertical"
          onChange={(_, data) => setSuccessConditions(data.value)}
          required
        />
      </Field>
      <Field label="Mission constraints" hint="Optional. One boundary per line, up to 20.">
        <Textarea
          aria-label="Mission constraints"
          value={constraints}
          resize="vertical"
          onChange={(_, data) => setConstraints(data.value)}
        />
      </Field>
      <Field label="Mission prohibitions" hint="Optional. One forbidden action per line, up to 20.">
        <Textarea
          aria-label="Mission prohibitions"
          value={prohibitions}
          resize="vertical"
          onChange={(_, data) => setProhibitions(data.value)}
        />
      </Field>
      <p className={classes.body}>
        Beacon binds this admission to Charter revision {project.charter.revision} and every Team
        member’s exact current configuration. It creates no Team Lead, Assignment, Run, spending,
        model invocation, or external effect.
      </p>
      <div className={classes.actions}>
        <Button
          appearance="primary"
          type="submit"
          disabled={
            busy ||
            !selectedTeam ||
            !desiredOutcome.trim() ||
            lines(successConditions).length === 0
          }
        >
          Admit Mission
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}

function lines(value: string): string[] {
  return value
    .split("\n")
    .map((item) => item.trim())
    .filter(Boolean);
}
