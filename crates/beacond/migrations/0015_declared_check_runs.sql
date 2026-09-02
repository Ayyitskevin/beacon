DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v14;

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
        'run.executed'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v14
ORDER BY sequence;

DROP TABLE events_v14;

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

-- A Run is now either a preflight or an execution of the Declared Checks that
-- a preflight admitted. Both share the lease and the single-terminal-transition
-- rule, so they share the table rather than growing a parallel one.
DROP TRIGGER runs_no_delete;
DROP TRIGGER runs_single_terminal_transition;

ALTER TABLE runs RENAME TO runs_v14;

CREATE TABLE runs (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('check_preflight', 'declared_checks')),
    repository_revision INTEGER NOT NULL CHECK (repository_revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    state TEXT NOT NULL CHECK (state IN ('preflighting', 'refused', 'verified', 'unconfirmed')),
    lease_expires_at_ms INTEGER NOT NULL CHECK (lease_expires_at_ms >= 0),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    ended_at_ms INTEGER CHECK (ended_at_ms IS NULL OR ended_at_ms >= started_at_ms),
    PRIMARY KEY (workspace_id, id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, repository_revision)
        REFERENCES project_repository_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

INSERT INTO runs (
    workspace_id, id, project_id, kind, repository_revision, charter_revision,
    state, lease_expires_at_ms, started_at_ms, ended_at_ms
)
SELECT
    workspace_id, id, project_id, kind, repository_revision, charter_revision,
    state, lease_expires_at_ms, started_at_ms, ended_at_ms
FROM runs_v14;

-- An execution Receipt records that the checks RAN, never that they passed:
-- a successful process exit is not a successful Mission.
-- Its renamed foreign key still targets runs_v14, so move the dependent rows first.
ALTER TABLE receipts RENAME TO receipts_v14;

CREATE TABLE receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version = 1),
    outcome TEXT NOT NULL CHECK (outcome IN (
        'source_kind_unsupported',
        'binding_stale_for_charter',
        'repository_unreadable',
        'base_revision_unreachable',
        'declared_checks_changed',
        'working_directory_escapes',
        'sandbox_unavailable',
        'preflight_failed',
        'preflight_timed_out',
        'verified',
        'not_admissible',
        'worktree_unavailable',
        'completed'
    )),
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    PRIMARY KEY (workspace_id, run_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES runs(workspace_id, id) ON DELETE RESTRICT
) STRICT;

INSERT INTO receipts (workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms)
SELECT workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
FROM receipts_v14;

DROP TABLE receipts_v14;
DROP TABLE runs_v14;

CREATE INDEX runs_project_started
ON runs (workspace_id, project_id, started_at_ms DESC);

CREATE INDEX runs_open_lease
ON runs (state, lease_expires_at_ms)
WHERE state = 'preflighting';

CREATE TRIGGER runs_no_delete
BEFORE DELETE ON runs
BEGIN
    SELECT RAISE(ABORT, 'Runs are immutable once opened');
END;

CREATE TRIGGER runs_single_terminal_transition
BEFORE UPDATE ON runs
WHEN OLD.state <> 'preflighting'
  OR NEW.state NOT IN ('refused', 'verified', 'unconfirmed')
  OR OLD.ended_at_ms IS NOT NULL
  OR NEW.ended_at_ms IS NULL
  OR NEW.id <> OLD.id
  OR NEW.workspace_id <> OLD.workspace_id
  OR NEW.project_id <> OLD.project_id
  OR NEW.kind <> OLD.kind
  OR NEW.repository_revision <> OLD.repository_revision
  OR NEW.charter_revision <> OLD.charter_revision
  OR NEW.started_at_ms <> OLD.started_at_ms
  OR NEW.lease_expires_at_ms <> OLD.lease_expires_at_ms
BEGIN
    SELECT RAISE(ABORT, 'A Run advances once from preflighting to a terminal state');
END;

CREATE TRIGGER receipts_no_update
BEFORE UPDATE ON receipts
BEGIN
    SELECT RAISE(ABORT, 'Receipts are immutable');
END;

CREATE TRIGGER receipts_no_delete
BEFORE DELETE ON receipts
BEGIN
    SELECT RAISE(ABORT, 'Receipts are immutable');
END;

UPDATE schema_meta SET version = 15 WHERE singleton = 1 AND version = 14;
