-- Outcome Document slice 3-A (ADR 0040): the immutable Outcome manifest store, and the candidate
-- Artifact each stored manifest was rendered into.
--
-- `prepare` stores one `OutcomeManifestV1`'s exact canonical bytes and, in the same transaction,
-- records the Markdown those bytes rendered to as an Artifact of kind `outcome_document`. This
-- table is the durable half: without it a completed Mission has a renderer and no Outcome, because
-- nothing persists what was rendered from.
--
-- NUMBERING: this migration holds 0029 PROVISIONALLY. `main` is at 0028 and other lanes in this
-- window (the publication relation, the evaluation arm) also want a number here; whichever of
-- 0029/0030 lands second renumbers at its final rebase, exactly as 0027 and 0028 each said the
-- second lane would.
--
-- NO `events` REBUILD, because this migration adds NO Event kind. A prepared Outcome enters the
-- ledger as its candidate Artifact's `artifact.recorded` Event -- the Artifact the manifest row's
-- foreign key requires -- so there is no record here the event log does not account for, and the
-- closed kind list is untouched. The FIRST later migration that rebuilds `events` must still carry
-- every kind `main` has, regenerated from its own parent.

-- One stored manifest: its exact canonical bytes, the identity those bytes hash to, and the
-- candidate Artifact they were rendered into.
--
-- The columns past `canonical_bytes` are PROJECTIONS FOR QUERYING ONLY -- the `artifacts`
-- discipline, applied here. Every claim is re-derived by decoding the stored bytes and recomputing
-- their digest on every verified read; a row that disagrees with its own bytes is invalid state,
-- not a different record. Nothing reads a claim off a column.
--
-- `manifest_id` is the R15 reference identity in the shape the seam expects, and it is DERIVED:
-- the first sixteen bytes of `sha256(canonical_bytes)`, re-spelled as a UUID. `OutcomeManifestV1`
-- carries no UUID inside its canonical bytes and its grammar is frozen (R12/R13), so a freshly
-- minted UUID here would be a second identity the bytes do not carry -- exactly the decoy R15
-- exists to refuse. A derived one cannot disagree with the bytes it is derived from. Whether the
-- manifest should instead carry a UUID inside its canonical bytes -- a canonicalizer version, and
-- a new frozen vector -- is a question for the Steward, recorded in the build report.
CREATE TABLE outcome_manifests (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer = 'beacon.outcome.manifest.v1'),
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

CREATE INDEX outcome_manifests_by_mission
    ON outcome_manifests (workspace_id, mission_id, boundary_event_seq);

-- Append-only, the `artifacts` rule: a stored manifest is the byte sequence a published Outcome's
-- appendix lets a reader recompute a digest from, so an in-place edit would silently rewrite the
-- proof. Corrections are a new manifest and a new version (R12 item 11: no deletion, no in-place
-- alteration, ever).
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

UPDATE schema_meta SET version = 29 WHERE singleton = 1 AND version = 28;
