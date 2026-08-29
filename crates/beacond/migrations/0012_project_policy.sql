DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v11;

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
FROM events_v11
ORDER BY sequence;

DROP TABLE events_v11;

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

CREATE TABLE project_policy_revisions (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    required_capabilities_json TEXT NOT NULL CHECK (
        json_valid(required_capabilities_json)
        AND json_type(required_capabilities_json) = 'array'
        AND json_array_length(required_capabilities_json) >= 1
    ),
    availability_rule TEXT NOT NULL CHECK (availability_rule = 'current_team_eligibility'),
    track_record_requirements_json TEXT NOT NULL CHECK (
        json_valid(track_record_requirements_json)
        AND json_type(track_record_requirements_json) = 'array'
        AND json_array_length(track_record_requirements_json) >= 1
    ),
    conflict_rule TEXT NOT NULL CHECK (conflict_rule = 'require_no_declared_conflict'),
    ambiguity_rule TEXT NOT NULL CHECK (ambiguity_rule = 'require_operator_decision'),
    authored_by TEXT NOT NULL CHECK (authored_by = 'operator'),
    authored_at_ms INTEGER NOT NULL CHECK (authored_at_ms >= 0),
    PRIMARY KEY (workspace_id, project_id, revision),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

CREATE INDEX project_policy_revisions_latest
ON project_policy_revisions (workspace_id, project_id, revision DESC);

CREATE TRIGGER project_policy_revisions_no_update
BEFORE UPDATE ON project_policy_revisions
BEGIN
    SELECT RAISE(ABORT, 'Project Policy revisions are immutable');
END;

CREATE TRIGGER project_policy_revisions_no_delete
BEFORE DELETE ON project_policy_revisions
BEGIN
    SELECT RAISE(ABORT, 'Project Policy revisions are immutable');
END;

UPDATE schema_meta SET version = 12 WHERE singleton = 1 AND version = 11;
