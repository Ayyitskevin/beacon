-- Outcome manifest store, second cut (R22): the `digest_canonicalizer` CHECK follows the codec.
--
-- Migration 0029 shipped with `digest_canonicalizer` pinned to `beacon.outcome.manifest.v1`, and
-- then the R18.3-5 grammar revision retired v1 and made `beacon.outcome.manifest.v2` the one
-- canonicalizer the codec emits or reads. 0029 merged while that revision was being built, so it is
-- applied history and cannot be edited (R22); this migration is the edit, as a migration.
--
-- THE CHECK PINS THE CONSTANT, v2 -- it does not admit {v1, v2}. The codec refuses v1 bytes BY
-- NAME (`RetiredVersion`), so a store that admitted a v1 row would hold a row its own reader
-- refuses; and there is no v1 row anywhere to keep: `outcome_manifests` merged empty and nothing
-- outside tests has called `prepare`. The CHECK's job is to equal the codec's truth, and the
-- codec's truth is one value. When the grammar moves again, so does this line, as 0031.
--
-- SQLite cannot alter a CHECK in place, so the table is rebuilt: the DDL below is 0029's, copied
-- verbatim with the one CHECK changed (the migration test diffs the two definitions so nothing
-- else can drift), rows are carried across, and the index and both immutability triggers are
-- recreated under their 0029 names. Nothing references `outcome_manifests` by foreign key, so the
-- rename rewrites no other table. Carrying a v1 row across would fail this CHECK loudly -- which is
-- the right outcome for a store holding a row the codec cannot read.
--
-- NUMBERING: this migration holds 0030 PROVISIONALLY. Other lanes want a number in this window;
-- whichever lands second renumbers at its final rebase, per the convention 0027-0029 followed.
--
-- NO `events` REBUILD: no Event kind is added (R18.6 still holds -- a prepared manifest is
-- positioned by its candidate Artifact's `artifact.recorded` Event). The FIRST later migration that
-- rebuilds `events` must still carry every kind `main` has, regenerated from its own parent.

DROP TRIGGER outcome_manifests_no_update;
DROP TRIGGER outcome_manifests_no_delete;
DROP INDEX outcome_manifests_by_mission;
ALTER TABLE outcome_manifests RENAME TO outcome_manifests_v29;

CREATE TABLE outcome_manifests_v30 (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer = 'beacon.outcome.manifest.v2'),
    manifest_digest TEXT NOT NULL PRIMARY KEY CHECK (
        length(manifest_digest) = 64 AND manifest_digest NOT GLOB '*[^0-9a-f]*'
    ),
    -- The lower bound is the codec's magic (27 bytes); the upper repeats the store's own
    -- `MAX_STORED_MANIFEST_BYTES`, which `record` enforces first, so a manifest past the ceiling is
    -- a named refusal about the bytes rather than a database error surfacing through a caller.
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 27 AND 16777216),
    manifest_id TEXT NOT NULL CHECK (length(manifest_id) = 36),
    mission_id TEXT NOT NULL,
    -- The `MissionCompletionV1` this manifest is frozen on, by its content identity.
    completion_digest TEXT NOT NULL CHECK (
        length(completion_digest) = 64 AND completion_digest NOT GLOB '*[^0-9a-f]*'
    ),
    canonicalizer_version TEXT NOT NULL,
    renderer_version TEXT NOT NULL,
    -- The completion's cut. Everything the manifest cites was recorded at or before it.
    boundary_event_seq INTEGER NOT NULL CHECK (boundary_event_seq >= 0),
    -- The Artifact holding the exact Markdown these bytes rendered to. One manifest renders to one
    -- candidate under one renderer version, and one candidate belongs to one manifest: a second
    -- manifest claiming a recorded candidate would make the document's provenance ambiguous.
    --
    -- This column is also this row's POSITION IN THE LOG. There is no `outcome.prepared` Event
    -- (R18.6), so the candidate's own `artifact.recorded` Event is what accounts for the manifest,
    -- and `NOT NULL` plus the foreign key below is why a manifest cannot exist without one.
    candidate_artifact_digest TEXT NOT NULL CHECK (
        length(candidate_artifact_digest) = 64
        AND candidate_artifact_digest NOT GLOB '*[^0-9a-f]*'
    ),
    UNIQUE (workspace_id, manifest_digest),
    UNIQUE (workspace_id, manifest_id),
    UNIQUE (workspace_id, candidate_artifact_digest),
    FOREIGN KEY (workspace_id, mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    -- `mission_completions` is keyed by digest alone, so this is the single-column reference that
    -- key admits. The Workspace agreement between the two records is the verified reader's check,
    -- which decodes both and refuses a cross-Workspace pair rather than filtering it away.
    FOREIGN KEY (completion_digest)
        REFERENCES mission_completions(digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, candidate_artifact_digest)
        REFERENCES artifacts(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

INSERT INTO outcome_manifests_v30 SELECT * FROM outcome_manifests_v29;
DROP TABLE outcome_manifests_v29;
ALTER TABLE outcome_manifests_v30 RENAME TO outcome_manifests;

CREATE INDEX outcome_manifests_by_mission
    ON outcome_manifests (workspace_id, mission_id, boundary_event_seq);

CREATE TRIGGER outcome_manifests_no_update
BEFORE UPDATE ON outcome_manifests
BEGIN
    SELECT RAISE(ABORT, 'Outcome manifests are immutable');
END;

CREATE TRIGGER outcome_manifests_no_delete
BEFORE DELETE ON outcome_manifests
BEGIN
    SELECT RAISE(ABORT, 'Outcome manifests are immutable');
END;

UPDATE schema_meta SET version = 30 WHERE singleton = 1 AND version = 29;
