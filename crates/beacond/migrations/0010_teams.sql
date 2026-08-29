ALTER TABLE events RENAME TO events_v9;

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
        'team.formed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v9
ORDER BY sequence;

DROP TABLE events_v9;

CREATE TABLE teams (
    id TEXT PRIMARY KEY CHECK (length(id) = 36),
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    name TEXT NOT NULL CHECK (length(trim(name)) BETWEEN 1 AND 120),
    formed_by TEXT NOT NULL CHECK (formed_by = 'operator'),
    formed_at_ms INTEGER NOT NULL CHECK (formed_at_ms >= 0),
    UNIQUE (workspace_id, id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX teams_by_project_and_formed
ON teams (workspace_id, project_id, formed_at_ms, id);

CREATE TABLE team_formation_members (
    workspace_id TEXT NOT NULL,
    team_id TEXT NOT NULL,
    position INTEGER NOT NULL CHECK (position >= 0),
    resident_id TEXT NOT NULL,
    configuration_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, team_id, position),
    UNIQUE (workspace_id, team_id, resident_id),
    FOREIGN KEY (workspace_id, team_id)
        REFERENCES teams(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, resident_id, configuration_id)
        REFERENCES resident_configuration_decisions(
            workspace_id, resident_id, id
        ) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER teams_no_update
BEFORE UPDATE ON teams
BEGIN
    SELECT RAISE(ABORT, 'Teams are immutable');
END;

CREATE TRIGGER teams_no_delete
BEFORE DELETE ON teams
BEGIN
    SELECT RAISE(ABORT, 'Teams are immutable');
END;

CREATE TRIGGER team_formation_members_no_update
BEFORE UPDATE ON team_formation_members
BEGIN
    SELECT RAISE(ABORT, 'Team formation members are immutable');
END;

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

CREATE TRIGGER team_formation_members_no_delete
BEFORE DELETE ON team_formation_members
BEGIN
    SELECT RAISE(ABORT, 'Team formation members are immutable');
END;

UPDATE schema_meta SET version = 10 WHERE singleton = 1 AND version = 9;
