import {
  Button,
  Field,
  Textarea,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useMemo, useState } from "react";

import { MissionClassification } from "./MissionClassification";
import { MissionCompletion } from "./MissionCompletion";
import { MissionOutcomes } from "./MissionOutcomes";
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
    // The same grid-item rule one level down: each Mission card is an item of `list`, which is
    // itself `display: grid` with an implicit `auto` track. Without this the card's min-content
    // re-establishes the width inside a section that has just been allowed to shrink.
    minWidth: 0,
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
  briefHeading: {
    fontSize: tokens.fontSizeBase300,
    marginTop: tokens.spacingVerticalM,
    marginBottom: tokens.spacingVerticalXS,
  },
  briefList: { marginTop: 0, marginBottom: 0, paddingLeft: tokens.spacingHorizontalXXL },
  details: { marginTop: tokens.spacingVerticalM, color: tokens.colorNeutralForeground2 },
  identity: { fontFamily: tokens.fontFamilyMonospace, fontSize: tokens.fontSizeBase200 },
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
  readFailure,
  onRetryRead,
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
  /** The Teams+Missions read's own failure, already projected by the owner. Empty means no
   *  failure. This panel never interpolates a raw rejection, a path or any user data. */
  readFailure: string;
  /** Ask the owner to re-run that one bundled read. Runs no Command. */
  onRetryRead: () => void;
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
      ) : readFailure ? (
        // A failed read is not an empty Project and not a read still running. Both of those
        // sentences would be false, and one of them was what this panel used to say for ever.
        <>
          <p role="alert">{readFailure}</p>
          <Button size="small" onClick={onRetryRead}>
            Retry Teams and Missions
          </Button>
        </>
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
              <MissionBrief mission={mission} teams={teams} />
              <MissionClassification mission={mission} />
              <MissionCompletion mission={mission} />
              <MissionOutcomes missionId={mission.id} />
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

/// The saved brief, exactly as it was admitted.
///
/// Every value here comes from this Mission's own record. The Project charter carries its own
/// constraints and prohibitions and they are deliberately NOT shown here: a card that substituted
/// them would be describing a different set of boundaries under the Mission's name.
///
/// An empty optional list says so rather than disappearing, because a section that vanishes reads
/// as "not applicable" when the truth is "the Operator recorded none". `successConditions` cannot
/// be empty under the current validator, so nothing here manufactures a default for it.
///
/// The lifecycle line states the recorded value and stops. `MissionLifecycle` has one variant,
/// `Admitted` (`crates/beacond/src/mission.rs:40-53`), and it is ADMISSION metadata: an admitted
/// Mission can already carry published Outcome data, so "started no work on it" was an inference
/// this record cannot support.
function MissionBrief({ mission, teams }: { mission: MissionView; teams: TeamView[] | null }) {
  const classes = useStyles();
  // A name is shown ONLY on an exact id match. The recorded binding is the id; a lookup that finds
  // nothing in the currently loaded list says nothing about whether the admission still holds, so
  // the id is retained and never described as lost.
  const recordedTeam = (teams ?? []).find((team) => team.id === mission.teamId) ?? null;

  return (
    <section aria-label="Mission brief">
      <h4 className={classes.briefHeading}>Success conditions</h4>
      <ol className={classes.briefList} aria-label="Success conditions">
        {mission.successConditions.map((condition, index) => (
          <li key={`${condition}-${index}`} className={classes.body}>
            {condition}
          </li>
        ))}
      </ol>

      <h4 className={classes.briefHeading}>Mission constraints</h4>
      {mission.constraints.length === 0 ? (
        <p className={classes.body}>None recorded.</p>
      ) : (
        <ol className={classes.briefList} aria-label="Mission constraints">
          {mission.constraints.map((constraint, index) => (
            <li key={`${constraint}-${index}`} className={classes.body}>
              {constraint}
            </li>
          ))}
        </ol>
      )}

      <h4 className={classes.briefHeading}>Mission prohibitions</h4>
      {mission.prohibitions.length === 0 ? (
        <p className={classes.body}>None recorded.</p>
      ) : (
        <ol className={classes.briefList} aria-label="Mission prohibitions">
          {mission.prohibitions.map((prohibition, index) => (
            <li key={`${prohibition}-${index}`} className={classes.body}>
              {prohibition}
            </li>
          ))}
        </ol>
      )}

      <details className={classes.details}>
        <summary>Recorded identities</summary>
        <p className={classes.body}>Lifecycle: Admitted.</p>
        <p className={classes.body}>
          Team:{" "}
          {recordedTeam === null ? (
            <>
              <span className={classes.identity}>{mission.teamId}</span> — the recorded Team id. No
              Team in the loaded list matches it here.
            </>
          ) : (
            <>
              {recordedTeam.name} <span className={classes.identity}>{mission.teamId}</span>
            </>
          )}
        </p>
        <ol className={classes.briefList} aria-label="Admitted members">
          {mission.members.map((member) => (
            <li key={member.residentId} className={classes.body}>
              Resident <span className={classes.identity}>{member.residentId}</span> under admission
              configuration <span className={classes.identity}>{member.admissionConfigurationId}</span>
            </li>
          ))}
        </ol>
      </details>
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
