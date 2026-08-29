import {
  Button,
  Checkbox,
  Field,
  Input,
  makeStyles,
  shorthands,
  tokens,
} from "@fluentui/react-components";
import { type FormEvent, useMemo, useState } from "react";

import type {
  ResidentAgentView,
  ResidentConfiguration,
  TeamDraft,
  TeamView,
} from "./session";

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
  body: { color: tokens.colorNeutralForeground2, maxWidth: "58ch" },
  list: { display: "grid", rowGap: tokens.spacingVerticalM, marginTop: tokens.spacingVerticalL },
  team: {
    ...shorthands.border(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderRadius(tokens.borderRadiusMedium),
    ...shorthands.padding(tokens.spacingVerticalM, tokens.spacingHorizontalM),
  },
  teamName: { margin: 0, fontWeight: tokens.fontWeightSemibold },
  meta: {
    color: tokens.colorNeutralForeground2,
    fontSize: tokens.fontSizeBase200,
    textTransform: "uppercase",
    letterSpacing: "0.08em",
  },
  members: { marginBottom: 0 },
  warning: { color: tokens.colorPaletteDarkOrangeForeground1 },
  form: {
    display: "grid",
    rowGap: tokens.spacingVerticalM,
    maxWidth: "680px",
    marginTop: tokens.spacingVerticalL,
  },
  roster: {
    display: "grid",
    rowGap: tokens.spacingVerticalXS,
    ...shorthands.borderTop(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.borderBottom(tokens.strokeWidthThin, "solid", tokens.colorNeutralStroke2),
    ...shorthands.padding(tokens.spacingVerticalS, 0),
  },
  actions: { display: "flex", columnGap: tokens.spacingHorizontalS },
});

export function ProjectTeams({
  projectId,
  teams,
  residents,
  configurations,
  composerOpen,
  busy,
  onOpenComposer,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  teams: TeamView[] | null;
  residents: ResidentAgentView[] | null;
  configurations: ResidentConfiguration[] | null;
  composerOpen: boolean;
  busy: boolean;
  onOpenComposer: () => void;
  onCancel: () => void;
  onSubmit: (draft: TeamDraft) => void;
}) {
  const classes = useStyles();
  const residentById = useMemo(
    () => new Map((residents ?? []).map((resident) => [resident.id, resident])),
    [residents],
  );

  return (
    <section className={classes.section} aria-labelledby="project-teams-heading">
      <div className={classes.header}>
        <div>
          <h3 id="project-teams-heading" className={classes.heading}>
            Teams
          </h3>
          <p className={classes.body}>
            Named groups for this Project. A Team has no permanent lead and formation grants no
            authority.
          </p>
        </div>
        {!composerOpen && (
          <Button onClick={onOpenComposer} disabled={busy}>
            Form Team
          </Button>
        )}
      </div>
      {composerOpen ? (
        <TeamComposer
          projectId={projectId}
          residents={residents}
          configurations={configurations}
          busy={busy}
          onCancel={onCancel}
          onSubmit={onSubmit}
        />
      ) : teams === null ? (
        <p role="status">Loading Teams</p>
      ) : teams.length === 0 ? (
        <p className={classes.body}>No Teams have been formed for this Project.</p>
      ) : (
        <div className={classes.list} aria-label="Project Teams">
          {teams.map((team) => (
            <article className={classes.team} key={team.id}>
              <p className={classes.teamName}>{team.name}</p>
              <p className={classes.meta}>
                Operator formed · {team.members.length} {team.members.length === 1 ? "member" : "members"}
              </p>
              <ul className={classes.members}>
                {team.members.map((member) => {
                  const resident = residentById.get(member.residentId);
                  return (
                    <li key={member.residentId}>
                      {resident?.displayName ?? resident?.role ?? "Resident"}
                      {!member.eligibleNow && (
                        <span className={classes.warning}> — needs configuration attention</span>
                      )}
                    </li>
                  );
                })}
              </ul>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function TeamComposer({
  projectId,
  residents,
  configurations,
  busy,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  residents: ResidentAgentView[] | null;
  configurations: ResidentConfiguration[] | null;
  busy: boolean;
  onCancel: () => void;
  onSubmit: (draft: TeamDraft) => void;
}) {
  const classes = useStyles();
  const [name, setName] = useState("");
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const residentById = new Map((residents ?? []).map((resident) => [resident.id, resident]));
  const eligible = (configurations ?? []).filter((configuration) => configuration.eligibleForTeam);

  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit({
      projectId,
      name: name.trim(),
      members: eligible
        .filter((configuration) => selected.has(configuration.residentId))
        .map((configuration) => ({
          residentId: configuration.residentId,
          expectedConfigurationId: configuration.id,
        })),
    });
  }

  return (
    <form className={classes.form} aria-label="Form Team" onSubmit={submit}>
      <Field label="Team name" required>
        <Input
          aria-label="Team name"
          value={name}
          maxLength={120}
          onChange={(_, data) => setName(data.value)}
          required
        />
      </Field>
      {configurations === null ? (
        <p role="status">Loading eligible Residents</p>
      ) : eligible.length === 0 ? (
        <p className={classes.body}>
          No Resident is currently eligible. Activate and configure one from Briefing first.
        </p>
      ) : (
        <fieldset className={classes.roster}>
          <legend>Eligible Residents</legend>
          {eligible.map((configuration) => {
            const resident = residentById.get(configuration.residentId);
            const label = resident?.displayName
              ? `${resident.displayName} — ${resident.role}`
              : (resident?.role ?? "Resident");
            return (
              <Checkbox
                key={configuration.residentId}
                label={label}
                checked={selected.has(configuration.residentId)}
                onChange={(_, data) => {
                  setSelected((current) => {
                    const next = new Set(current);
                    if (data.checked === true) next.add(configuration.residentId);
                    else next.delete(configuration.residentId);
                    return next;
                  });
                }}
              />
            );
          })}
        </fieldset>
      )}
      <p className={classes.body}>
        Beacon records each selected Resident’s exact current configuration. This starts no
        Mission, Run, spending, model invocation, or external effect.
      </p>
      <div className={classes.actions}>
        <Button
          appearance="primary"
          type="submit"
          disabled={busy || configurations === null || !name.trim() || selected.size === 0}
        >
          Form Team
        </Button>
        <Button type="button" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
