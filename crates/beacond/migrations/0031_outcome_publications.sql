-- Outcome Document slice 3-B (ADR 0040, gpt2 design section 1): the external publication relation.
--
-- `publish` records that one candidate Outcome Artifact was qualified by one independent
-- Evaluation. The relation is EXTERNAL because the alternative is impossible: embedding the
-- Evaluation inside the candidate would change the candidate's bytes, and therefore its digest, and
-- therefore the digest the Evaluation is about (gpt2 design section 1 question 3). The row is the
-- only place the pairing can live without one of the two records having to contain the other.
--
-- NUMBERING: this migration holds 0031 PROVISIONALLY and renumbers at its final rebase. `main` is
-- at 0030 (the manifest canonicalizer CHECK, #72); this lane is stacked on the assembler lane,
-- which adds none. 0030's own header reserved the phrase "as 0031" for the NEXT grammar move --
-- that move is not built, so the number is free, and a grammar bump landing first renumbers this.
--
-- NO `events` REBUILD, and no new Event kind: THE APPEND IS THE PUBLICATION EVENT (gpt2's defended
-- asymmetry, kimi's slice-3 read section 2). Adding a Ruling citation to an earlier record later
-- would be different -- that would alter a provenance claim about an event that already happened --
-- but this row IS the event being represented, so there is nothing for a separate Event kind to
-- account for that the row does not. The closed kind list is untouched. The FIRST later migration
-- that rebuilds `events` must still carry every kind `main` has, regenerated from its own parent.

-- One publication: which Mission, which candidate, which Evaluation qualified it, and which
-- version of the Mission's publication history this is.
--
-- Every column is an IDENTITY. There is deliberately nothing renderable here -- no title, no
-- verdict, no summary, no timestamp a surface could print as a claim (R4.3, carried forward from
-- the seam this row consumes). A surface that wants to say something about a published Outcome
-- reads the manifest and the candidate through their owning verified readers and cites those.
--
-- `mission_id` in particular is an identity column and never renderable (kimi's E4): it says which
-- Mission this publication belongs to so the version sequence can be per-Mission, and a document
-- that wants the Mission's title reads it through the Mission's owner at the boundary.
CREATE TABLE outcome_publications (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    mission_id TEXT NOT NULL,
    -- The R15 reference identity. Unlike `outcome_manifests.manifest_id` this one is MINTED rather
    -- than derived, and the difference is not an inconsistency: a manifest has canonical bytes to
    -- derive an identity from, and this row has none. It is a relation between two records that
    -- each carry their own identity, so its own identity cannot be a function of bytes it does not
    -- have. Deriving one from the pair would make the identity a restatement of the foreign keys.
    publication_id TEXT NOT NULL CHECK (length(publication_id) = 36),
    -- The candidate's RECORD digest (R16), in the envelope the Artifact module verified.
    candidate_artifact_digest TEXT NOT NULL CHECK (
        length(candidate_artifact_digest) = 64
        AND candidate_artifact_digest NOT GLOB '*[^0-9a-f]*'
    ),
    -- The Evaluation whose subject is EXACTLY that candidate digest. The seam proves that from the
    -- Evaluation's canonical bytes before this row is written; the column stores what it proved.
    evaluation_digest TEXT NOT NULL CHECK (
        length(evaluation_digest) = 64 AND evaluation_digest NOT GLOB '*[^0-9a-f]*'
    ),
    -- Contiguous from 1 per (workspace, Mission). A correction is version n+1 beside version n,
    -- never an edit of n (R12 item 11: no deletion, no in-place alteration). SQLite can enforce
    -- uniqueness and the floor; CONTIGUITY is enforced in `publish`, which reads the current
    -- maximum inside the same transaction and refuses a gap by name -- a constraint that can only
    -- be stated in code is stated in code, and named in a test rather than assumed.
    publication_version INTEGER NOT NULL CHECK (publication_version >= 1),
    PRIMARY KEY (workspace_id, mission_id, publication_version),
    UNIQUE (workspace_id, publication_id),
    -- One candidate is published at most once, and one Evaluation qualifies at most one
    -- publication. Both follow from the seam (an Evaluation's subject is one exact candidate), and
    -- both are stated here so a second row citing either is a database refusal rather than a
    -- second version that says nothing new.
    UNIQUE (workspace_id, candidate_artifact_digest),
    UNIQUE (workspace_id, evaluation_digest),
    FOREIGN KEY (workspace_id, mission_id)
        REFERENCES missions(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, candidate_artifact_digest)
        REFERENCES artifacts(workspace_id, digest_value) ON DELETE RESTRICT,
    -- `evaluations_workspace_digest` is the scoped unique index this pair references.
    FOREIGN KEY (workspace_id, evaluation_digest)
        REFERENCES evaluations(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

CREATE INDEX outcome_publications_by_mission
    ON outcome_publications (workspace_id, mission_id, publication_version);

-- Append-only, the `outcome_manifests` rule: a publication is a historical fact about what was
-- qualified when. An in-place edit would rewrite which document a past Evaluation approved, and a
-- delete would erase a version a correction is defined as superseding rather than replacing.
CREATE TRIGGER outcome_publications_no_update
BEFORE UPDATE ON outcome_publications
BEGIN
    SELECT RAISE(ABORT, 'Outcome publications are immutable');
END;

CREATE TRIGGER outcome_publications_no_delete
BEFORE DELETE ON outcome_publications
BEGIN
    SELECT RAISE(ABORT, 'Outcome publications are immutable');
END;

UPDATE schema_meta SET version = 31 WHERE singleton = 1 AND version = 30;
