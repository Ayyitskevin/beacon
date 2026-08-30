-- Execution Receipt v2 adds a truthful terminal state for dispatches whose
-- evidence is incomplete. Existing states remain valid and existing rows are
-- copied without rewriting their Receipt or event bytes.
DROP TRIGGER runs_no_delete;
DROP TRIGGER runs_single_terminal_transition;
DROP TRIGGER receipts_no_update;
DROP TRIGGER receipts_no_delete;

-- Move the dependent table first. Renaming `runs` then updates this preserved
-- foreign key to `runs_v15`; both old tables stay until both copies exist.
ALTER TABLE receipts RENAME TO receipts_v15;
ALTER TABLE runs RENAME TO runs_v15;
DROP INDEX runs_project_started;
DROP INDEX runs_open_lease;

CREATE TABLE runs (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('check_preflight', 'declared_checks')),
    repository_revision INTEGER NOT NULL CHECK (repository_revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    state TEXT NOT NULL CHECK (state IN (
        'preflighting',
        'refused',
        'verified',
        'incomplete',
        'unconfirmed'
    )),
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
FROM runs_v15;

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
  OR NEW.state NOT IN ('refused', 'verified', 'incomplete', 'unconfirmed')
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

-- Receipt v2 is an expansion. V1 remains admissible and its stored JSON is
-- copied exactly; events are not rebuilt by this migration at all.
CREATE TABLE receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version IN (1, 2)),
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
        'completed',
        'incomplete'
    )),
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    PRIMARY KEY (workspace_id, run_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES runs(workspace_id, id) ON DELETE RESTRICT
) STRICT;

INSERT INTO receipts (workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms)
SELECT workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
FROM receipts_v15;

DROP TABLE receipts_v15;
DROP TABLE runs_v15;

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

UPDATE schema_meta SET version = 16 WHERE singleton = 1 AND version = 15;
