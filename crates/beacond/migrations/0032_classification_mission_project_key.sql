-- A classification whose Project does not own its Mission becomes UNREPRESENTABLE (R20.1, sol1's
-- cold read #2).
--
-- `MissionImpactClassificationV1` identifies its Policy as `(project_id, policy_revision,
-- charter_revision)` and its subject as `mission_id`, and 0025 declared those two facts as
-- SEPARATE foreign keys: `(workspace_id, mission_id) -> missions` and `(workspace_id, project_id,
-- policy_revision) -> project_policy_revisions`. Both hold for a real Mission of Project A
-- classified under a real Policy of unrelated Project B, because neither says the two belong
-- together. The Command refuses it (`PolicyProjectIsNotTheMissions`) and so does the verified
-- reader, on every read -- and both are code, which a restore, an older binary or a hand-run
-- INSERT walks straight past. This is the same rule as a key.
--
-- THE PARENT KEY ALREADY EXISTS. 0018 created `missions_workspace_project_id` UNIQUE over
-- `(workspace_id, project_id, id)` for the Proposal's own composite key, and SQLite resolves a
-- parent key by the SET of columns rather than the order they are written in -- measured on
-- 3.45.1 against this store: a child declaring `REFERENCES missions(workspace_id, id, project_id)`
-- binds to that index and refuses a mismatched row with `FOREIGN KEY constraint failed`, not with
-- `foreign key mismatch`. So no second UNIQUE is added here: one index, one statement of the
-- Mission's identity within its Project, and nothing new to drift.
--
-- The composite key REPLACES the plain `(workspace_id, mission_id)` one rather than joining it:
-- `(workspace_id, mission_id, project_id)` implies the Mission exists, so keeping both would be
-- two spellings of one rule with the weaker one able to pass alone.
--
-- SQLite cannot add a foreign key in place, so the table is rebuilt: the DDL below is 0025's,
-- copied with that one key changed, rows carried across in digest order, and both immutability
-- triggers recreated under their 0025 names. NOTHING references
-- `mission_impact_classifications` by foreign key, so the rename rewrites no other table -- the
-- trap 0028 hit and documented. Any row already stored that the new key refuses fails this
-- migration loudly, which is the right outcome for a store holding a classification its own
-- Command and reader both refuse.
--
-- NUMBERING: 0032 is final. It was written while `main` was at 0030 with 0031 riding an unmerged
-- lane; that lane (the publication relation, #80) landed first, so this rebased onto it and the
-- ladder is contiguous: 30 -> 31 -> 32. The stamp below names 31 as the version a store arrives
-- at, because a guard that matches nothing is a silent no-op -- the failure the 0028 rollback test
-- records, where an UPDATE stopped firing and surfaced downstream as a SchemaMismatch.
--
-- R21.5 IS NOT IN THIS MIGRATION, and the reason is measurable rather than a preference. The
-- analogous key for supersession -- a projected `supersedes_digest` column with
-- `(workspace_id, supersedes_digest) -> mission_impact_classifications` -- needs that column filled
-- for the rows already stored, and its value lives at a VARIABLE offset inside `canonical_bytes`:
-- `encode` writes `rule_id` (optional, variable) and `reason` (variable-length text) before
-- `supersedes`, so no `substr()` can reach it and only the codec can compute it. The ladder applies
-- pure SQL, so a migration that added the column would leave every existing row NULL -- and under
-- MATCH SIMPLE a NULL component SATISFIES a composite foreign key, so those rows would be silently
-- unconstrained while the schema looked fastened. That is worse than not shipping it yet.
--
-- It is also not schema-only: the writer must populate the column and the verified reader must
-- re-derive it in `classification_columns_match`, where every projection is re-derived so that
-- adding a column without adding a line there is a compile-time hole rather than a silent one.
-- This migration needed no code change at all. So R21.5 is its own slice: a codec-side backfill,
-- then the column and the key. Until then the belt is real and merged -- the reader judges a
-- predecessor by its own bytes and refuses one that is absent, unreadable, of another Workspace or
-- of another Mission (sol1's #9, `PredecessorFault`).
--
-- NO `events` REBUILD: no Event kind is added. The FIRST later migration that rebuilds `events`
-- must still carry every kind `main` has, regenerated from its own parent.

DROP TRIGGER mission_impact_classifications_no_update;
DROP TRIGGER mission_impact_classifications_no_delete;

ALTER TABLE mission_impact_classifications RENAME TO mission_impact_classifications_v25;

CREATE TABLE mission_impact_classifications (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer = 'beacon.mission-impact.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 25 AND 65536),
    classification_id TEXT NOT NULL CHECK (length(classification_id) = 36),
    mission_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    version INTEGER NOT NULL CHECK (version >= 1),
    impact TEXT NOT NULL CHECK (impact IN ('high', 'ordinary')),
    basis TEXT NOT NULL CHECK (basis IN ('operator_declared', 'policy_rule')),
    policy_revision INTEGER NOT NULL CHECK (policy_revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    classified_at_ms INTEGER NOT NULL CHECK (classified_at_ms >= 0),
    UNIQUE (workspace_id, classification_id),
    UNIQUE (workspace_id, mission_id, version),
    -- The key this migration exists for: the Mission is this Workspace's, and the Project the
    -- record names is the Project that Mission belongs to. Subsumes 0025's
    -- `(workspace_id, mission_id) -> missions`.
    FOREIGN KEY (workspace_id, mission_id, project_id)
        REFERENCES missions(workspace_id, id, project_id) ON DELETE RESTRICT,
    -- The composite key that makes it impossible to classify a Mission under a Policy revision that
    -- does not exist. The check that would otherwise live in prose.
    FOREIGN KEY (workspace_id, project_id, policy_revision)
        REFERENCES project_policy_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

INSERT INTO mission_impact_classifications (
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    classification_id, mission_id, project_id, version, impact, basis,
    policy_revision, charter_revision, classified_at_ms
)
SELECT
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    classification_id, mission_id, project_id, version, impact, basis,
    policy_revision, charter_revision, classified_at_ms
FROM mission_impact_classifications_v25
ORDER BY digest_value;

DROP TABLE mission_impact_classifications_v25;

CREATE TRIGGER mission_impact_classifications_no_update
BEFORE UPDATE ON mission_impact_classifications
BEGIN
    SELECT RAISE(ABORT, 'Mission impact classifications are immutable');
END;

CREATE TRIGGER mission_impact_classifications_no_delete
BEFORE DELETE ON mission_impact_classifications
BEGIN
    SELECT RAISE(ABORT, 'Mission impact classifications are immutable');
END;

UPDATE schema_meta SET version = 32 WHERE singleton = 1 AND version = 31;
