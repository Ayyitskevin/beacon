-- The opt-in Sample Project is an additive, inert installation record. Existing
-- domain and evidence bytes are copied exactly; no Run or Receipt is created.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v16;

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
        'mission.admitted',
        'run.preflighted',
        'run.executed',
        'sample_project.installed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v16
ORDER BY sequence;

DROP TABLE events_v16;

CREATE TRIGGER team_formation_members_no_insert_after_formation
BEFORE INSERT ON team_formation_members
WHEN EXISTS (
    SELECT 1 FROM events
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
    SELECT 1 FROM events
    WHERE workspace_id = NEW.workspace_id
      AND kind = 'mission.admitted'
      AND json_extract(payload_json, '$.mission.id') = NEW.mission_id
)
BEGIN
    SELECT RAISE(ABORT, 'Mission admission roster is closed');
END;

CREATE TABLE sample_project_installations (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL UNIQUE,
    charter_revision INTEGER NOT NULL CHECK (charter_revision = 1),
    repository_revision INTEGER NOT NULL CHECK (repository_revision = 1),
    repository_path TEXT NOT NULL CHECK (length(repository_path) >= 1),
    base_revision TEXT NOT NULL CHECK (
        length(base_revision) = 40
        AND base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    bundle_version INTEGER NOT NULL CHECK (bundle_version >= 1),
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL,
    digest_value TEXT NOT NULL CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    provenance_source TEXT NOT NULL,
    installed_at_ms INTEGER NOT NULL CHECK (installed_at_ms >= 0),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, repository_revision)
        REFERENCES project_repository_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER sample_project_installations_no_update
BEFORE UPDATE ON sample_project_installations
BEGIN
    SELECT RAISE(ABORT, 'Sample Project installations are immutable');
END;

CREATE TRIGGER sample_project_installations_no_delete
BEFORE DELETE ON sample_project_installations
BEGIN
    SELECT RAISE(ABORT, 'Sample Project installations are immutable');
END;

UPDATE schema_meta SET version = 17 WHERE singleton = 1 AND version = 16;
