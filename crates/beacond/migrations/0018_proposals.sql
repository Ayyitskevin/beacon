-- Proposal v1 is inert. This migration creates identity and evidence state only: it adds no Run,
-- Receipt, Ruling, dispatch surface, or transport, and it starts nothing.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v17;

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
        'sample_project.installed',
        'proposal.submitted'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v17
ORDER BY sequence;

DROP TABLE events_v17;

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

-- Parent keys the Proposal row binds to. SQLite requires a UNIQUE index over every composite
-- foreign-key parent, so each index below exists to let the schema state a rule the Proposal would
-- otherwise only assert in code: the Mission belongs to the named Project, the requesting Resident
-- is an admitted member at that exact admission Configuration, and the Policy and Repository
-- revisions bind the named Charter revision and pinned base revision.
CREATE UNIQUE INDEX missions_workspace_project_id
ON missions (workspace_id, project_id, id);

CREATE UNIQUE INDEX mission_admission_members_configuration
ON mission_admission_members (workspace_id, mission_id, resident_id, configuration_id);

CREATE UNIQUE INDEX project_policy_revisions_charter
ON project_policy_revisions (workspace_id, project_id, revision, charter_revision);

CREATE UNIQUE INDEX project_repository_revisions_charter_base
ON project_repository_revisions (workspace_id, project_id, revision, charter_revision, base_revision);

CREATE TABLE proposals (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.proposal.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 19 AND 65536),
    project_id TEXT NOT NULL,
    mission_id TEXT NOT NULL,
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    project_policy_revision INTEGER NOT NULL CHECK (project_policy_revision >= 1),
    repository_revision INTEGER NOT NULL CHECK (repository_revision >= 1),
    pinned_base_revision TEXT NOT NULL CHECK (
        length(pinned_base_revision) = 40
        AND pinned_base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    requester_resident_id TEXT NOT NULL,
    requester_configuration_id TEXT NOT NULL,
    bootstrap_basis TEXT NOT NULL CHECK (bootstrap_basis = 'owner'),
    bootstrap_reason TEXT NOT NULL CHECK (length(trim(bootstrap_reason)) BETWEEN 1 AND 500),
    submitted_at_ms INTEGER NOT NULL CHECK (submitted_at_ms >= 0),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, mission_id)
        REFERENCES missions(workspace_id, project_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, charter_revision)
        REFERENCES project_charter_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, project_policy_revision, charter_revision)
        REFERENCES project_policy_revisions(
            workspace_id, project_id, revision, charter_revision
        ) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, repository_revision, charter_revision, pinned_base_revision)
        REFERENCES project_repository_revisions(
            workspace_id, project_id, revision, charter_revision, base_revision
        ) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, mission_id, requester_resident_id, requester_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

CREATE INDEX proposals_by_mission
ON proposals (workspace_id, mission_id, submitted_at_ms, digest_value);

CREATE TRIGGER proposals_no_update
BEFORE UPDATE ON proposals
BEGIN
    SELECT RAISE(ABORT, 'Proposals are immutable');
END;

CREATE TRIGGER proposals_no_delete
BEFORE DELETE ON proposals
BEGIN
    SELECT RAISE(ABORT, 'Proposals are immutable');
END;

UPDATE schema_meta SET version = 18 WHERE singleton = 1 AND version = 17;
