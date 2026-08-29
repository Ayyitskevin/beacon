DROP TRIGGER team_formation_members_no_insert_after_formation;

ALTER TABLE events RENAME TO events_v10;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded',
        'project.created',
        'project.charter_revised',
        'local_model.activated',
        'local_model.deactivated',
        'resident.activated',
        'resident.suspended',
        'resident.reactivated',
        'resident.retired',
        'resident.configuration_set',
        'resident.configuration_cleared',
        'team.formed',
        'mission.admitted'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v10
ORDER BY sequence;

DROP TABLE events_v10;

CREATE TRIGGER team_formation_members_no_insert_after_formation
BEFORE INSERT ON team_formation_members
WHEN EXISTS (
    SELECT 1
    FROM events
    WHERE workspace_id = NEW.workspace_id
      AND kind = 'team.formed'
      AND json_extract(payload_json, '$.team.id') = NEW.team_id
)
BEGIN
    SELECT RAISE(ABORT, 'Team formation roster is closed');
END;

CREATE UNIQUE INDEX teams_workspace_project_id
ON teams (workspace_id, project_id, id);

CREATE TABLE missions (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    team_id TEXT NOT NULL,
    desired_outcome TEXT NOT NULL CHECK (length(trim(desired_outcome)) BETWEEN 1 AND 2000),
    success_conditions_json TEXT NOT NULL
        CHECK (json_valid(success_conditions_json) AND json_type(success_conditions_json) = 'array'),
    constraints_json TEXT NOT NULL
        CHECK (json_valid(constraints_json) AND json_type(constraints_json) = 'array'),
    prohibitions_json TEXT NOT NULL
        CHECK (json_valid(prohibitions_json) AND json_type(prohibitions_json) = 'array'),
    lifecycle TEXT NOT NULL CHECK (lifecycle = 'admitted'),
    admitted_by TEXT NOT NULL CHECK (admitted_by = 'operator'),
    admitted_at_ms INTEGER NOT NULL CHECK (admitted_at_ms >= 0),
    UNIQUE (workspace_id, id),
    UNIQUE (workspace_id, id, team_id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, team_id)
        REFERENCES teams(workspace_id, project_id, id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX missions_by_project_and_admitted
ON missions (workspace_id, project_id, admitted_at_ms, id);

CREATE TABLE mission_admission_members (
    workspace_id TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    resident_id TEXT NOT NULL,
    configuration_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, mission_id, position),
    UNIQUE (workspace_id, mission_id, resident_id),
    FOREIGN KEY (workspace_id, mission_id, team_id)
        REFERENCES missions(workspace_id, id, team_id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, team_id, resident_id)
        REFERENCES team_formation_members(
            workspace_id, team_id, resident_id
        ) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, resident_id, configuration_id)
        REFERENCES resident_configuration_decisions(
            workspace_id, resident_id, id
        ) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER missions_no_update
BEFORE UPDATE ON missions
BEGIN
    SELECT RAISE(ABORT, 'Missions are immutable at admission');
END;

CREATE TRIGGER missions_no_delete
BEFORE DELETE ON missions
BEGIN
    SELECT RAISE(ABORT, 'Missions are immutable at admission');
END;

CREATE TRIGGER mission_admission_members_no_update
BEFORE UPDATE ON mission_admission_members
BEGIN
    SELECT RAISE(ABORT, 'Mission admission members are immutable');
END;

CREATE TRIGGER mission_admission_members_no_insert_after_admission
BEFORE INSERT ON mission_admission_members
WHEN EXISTS (
    SELECT 1
    FROM events
    WHERE workspace_id = NEW.workspace_id
      AND kind = 'mission.admitted'
      AND json_extract(payload_json, '$.mission.id') = NEW.mission_id
)
BEGIN
    SELECT RAISE(ABORT, 'Mission admission roster is closed');
END;

CREATE TRIGGER mission_admission_members_no_delete
BEFORE DELETE ON mission_admission_members
BEGIN
    SELECT RAISE(ABORT, 'Mission admission members are immutable');
END;

UPDATE schema_meta SET version = 11 WHERE singleton = 1 AND version = 10;
