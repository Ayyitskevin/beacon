-- R46: `pre_image_unreadable` joins the Receipt outcome vocabulary.
--
-- The vocabulary is closed in TWO places -- `DispatchOutcome` and this CHECK -- and a new outcome
-- that reached only the first is a refusal the product can decide and the store will not accept.
-- Measured, not anticipated: the constructed-fault test refused correctly and then failed on
-- `CHECK constraint failed: outcome IN (...)`.
--
-- SQLite cannot alter a CHECK in place, so the table is rebuilt: 0022's DDL copied verbatim with
-- the one list extended, rows carried across, triggers recreated byte-exactly. No column or key
-- changes.
--
-- AND `evaluation_cited_runs` IS REBUILT WITH IT, because it carries a foreign key INTO `receipts`
-- and a modern `ALTER TABLE ... RENAME` REWRITES that reference to follow the table -- so after
-- this rebuild drops `receipts_v32` it would point at nothing and the next insert would fail with
-- `no such table: main.receipts_v32`. 0028 measured exactly this for `evaluations` and its
-- children, and recorded that `PRAGMA legacy_alter_table = ON` does not help because it is
-- ineffective inside a transaction, which every migration here runs in. I walked into it anyway;
-- the suite found it. SQLite says who the children are -- one table, asked of `sqlite_master`
-- rather than assumed from the migration texts.

DROP TRIGGER receipts_no_update;
DROP TRIGGER receipts_no_delete;
DROP TRIGGER evaluation_cited_runs_no_update;
DROP TRIGGER evaluation_cited_runs_no_delete;
ALTER TABLE evaluation_cited_runs RENAME TO evaluation_cited_runs_v32;
ALTER TABLE receipts RENAME TO receipts_v32;

CREATE TABLE receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version IN (1, 2, 3)),
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
        'incomplete',
        -- Dispatch v3. Every refusal names the gate that refused, so an Operator reads a
        -- correction rather than a category.
        'ruling_unreadable',
        'ruling_refused',
        'ruling_superseded',
        'ruling_drifted',
        'proposal_digest_mismatch',
        'proposal_not_ready',
        'candidate_absent',
        'candidate_mismatch',
        'content_digest_mismatch',
        'content_over_bounds',
        'source_not_local',
        'source_not_beacon_made',
        'repository_head_moved',
        'source_detached',
        'source_dirty',
        'autonomy_profile_refuses',
        'installation_paused',
        'already_dispatched',
        'pre_image_mismatch',
        -- R46: a pre-image the product could not READ. Not a mismatch: nothing was compared.
        'pre_image_unreadable',
        'wall_clock_exceeded',
        'admissible',
        'applied',
        -- After preparation. These may leave objects no ref names, which is why they are not
        -- `refused`: the repository is not byte-identical afterwards and the Receipt says so.
        'object_write_failed',
        'effect_does_not_match_declaration',
        'landing_refused',
        'landing_unobserved'
    )),
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    PRIMARY KEY (workspace_id, run_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES runs(workspace_id, id) ON DELETE RESTRICT
) STRICT;

INSERT INTO receipts SELECT * FROM receipts_v32;
DROP TABLE receipts_v32;

-- The child, from its own live definition, pointing at the rebuilt parent.
CREATE TABLE evaluation_cited_runs (
    workspace_id TEXT NOT NULL,
    evaluation_digest TEXT NOT NULL,
    run_id TEXT NOT NULL,
    PRIMARY KEY (workspace_id, evaluation_digest, run_id),
    FOREIGN KEY (workspace_id, evaluation_digest)
        REFERENCES evaluations(workspace_id, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, run_id)
        REFERENCES receipts(workspace_id, run_id) ON DELETE RESTRICT
) STRICT;

INSERT INTO evaluation_cited_runs SELECT * FROM evaluation_cited_runs_v32;
DROP TABLE evaluation_cited_runs_v32;

CREATE TRIGGER evaluation_cited_runs_no_update
BEFORE UPDATE ON evaluation_cited_runs
BEGIN
    SELECT RAISE(ABORT, 'Evaluation cited Runs are immutable');
END;
CREATE TRIGGER evaluation_cited_runs_no_delete
BEFORE DELETE ON evaluation_cited_runs
BEGIN
    SELECT RAISE(ABORT, 'Evaluation cited Runs are immutable');
END;

-- Recreated byte-exactly as 0022 wrote them: SQLite keeps the literal CREATE text, and startup
-- compares the schema before it reaches any verification pass (R39.3, learned on the R28 lanes).
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



UPDATE schema_meta SET version = 33 WHERE singleton = 1 AND version = 32;
