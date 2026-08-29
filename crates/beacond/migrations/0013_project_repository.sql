DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v12;

CREATE TABLE events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    kind TEXT NOT NULL CHECK (kind IN (
        'workspace.created',
        'starter_organization.accepted',
        'starter_organization.discarded',
        'project.created',
        'project.charter_revised',
        'project.policy_published',
        'project.repository_bound',
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
FROM events_v12
ORDER BY sequence;

DROP TABLE events_v12;

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

CREATE TABLE project_repository_revisions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    source_kind TEXT NOT NULL CHECK (source_kind IN ('local_path', 'git_url')),
    source TEXT NOT NULL CHECK (length(trim(source)) BETWEEN 1 AND 2000),
    base_revision TEXT NOT NULL CHECK (
        length(base_revision) = 40
        AND base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    declared_checks_json TEXT NOT NULL CHECK (
        json_valid(declared_checks_json)
        AND json_type(declared_checks_json) = 'array'
        AND json_array_length(declared_checks_json) >= 1
    ),
    author_kind TEXT NOT NULL CHECK (author_kind = 'operator'),
    authored_at_ms INTEGER NOT NULL CHECK (authored_at_ms >= 0),
    PRIMARY KEY (workspace_id, project_id, revision),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

CREATE INDEX project_repository_revisions_latest
ON project_repository_revisions (workspace_id, project_id, revision DESC);

CREATE TRIGGER project_repository_revisions_no_update
BEFORE UPDATE ON project_repository_revisions
BEGIN
    SELECT RAISE(ABORT, 'Project Repository revisions are immutable');
END;

CREATE TRIGGER project_repository_revisions_no_delete
BEFORE DELETE ON project_repository_revisions
BEGIN
    SELECT RAISE(ABORT, 'Project Repository revisions are immutable');
END;

UPDATE schema_meta SET version = 13 WHERE singleton = 1 AND version = 12;
