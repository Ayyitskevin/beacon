-- R48: the Receipt outcome vocabulary belongs to each typed Rust owner, not to one shared SQL
-- membership CHECK.
--
-- R46.4 chose this as a separate follow-up to 0033. Removing only that CHECK means a future owner
-- vocabulary extension does not rebuild immutable Receipt history. The trade is explicit: SQL no
-- longer rejects an unknown token at insertion time; every owning read must deserialize and verify
-- the typed Receipt before returning a value.
--
-- SQLite cannot drop a CHECK in place, so this is 0033's tested parent-and-child rebuild shape.
-- Every other `receipts` constraint, both immutability triggers, and the dependent
-- `evaluation_cited_runs` table and triggers are carried unchanged. The child moves with the parent
-- because `ALTER TABLE ... RENAME` rewrites its foreign key to follow the old parent name.

DROP TRIGGER receipts_no_update;
DROP TRIGGER receipts_no_delete;
DROP TRIGGER evaluation_cited_runs_no_update;
DROP TRIGGER evaluation_cited_runs_no_delete;
ALTER TABLE evaluation_cited_runs RENAME TO evaluation_cited_runs_v33;
ALTER TABLE receipts RENAME TO receipts_v33;

CREATE TABLE receipts (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    -- R46.7.1: version 4 is the R46 era -- a mismatch Receipt carrying its observed pre-image as a
    -- closed value. Extended HERE rather than in a second rebuild, because this migration already
    -- rebuilds this table and a second one would be two rewrites of the same rows.
    version INTEGER NOT NULL CHECK (version IN (1, 2, 3, 4)),
    outcome TEXT NOT NULL,
    receipt_json TEXT NOT NULL CHECK (json_valid(receipt_json)),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    PRIMARY KEY (workspace_id, run_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES runs(workspace_id, id) ON DELETE RESTRICT
) STRICT;

INSERT INTO receipts SELECT * FROM receipts_v33;

-- New parent, copy, new child referencing the NEW parent, copy the child's rows, then drop the old
-- child and only then the old parent: the populated-child order 0033 proved.
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

INSERT INTO evaluation_cited_runs SELECT * FROM evaluation_cited_runs_v33;
DROP TABLE evaluation_cited_runs_v33;
DROP TABLE receipts_v33;

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

-- Refuse rather than merely report a dangling foreign key. `RAISE()` is trigger-only, so a
-- temporary CHECK turns any non-zero count into a transactional migration failure.
CREATE TEMP TABLE migration_0034_foreign_key_guard (
    dangling INTEGER NOT NULL CHECK (dangling = 0)
);
INSERT INTO migration_0034_foreign_key_guard
    SELECT count(*) FROM pragma_foreign_key_check;
DROP TABLE migration_0034_foreign_key_guard;

UPDATE schema_meta SET version = 34 WHERE singleton = 1 AND version = 33;
