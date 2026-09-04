-- The Evaluation subject becomes a tagged union in the STORE too: the `0x03` Artifact arm's
-- columns, the generalized `subject_digest` identity, and the rekey of everything that pointed at
-- `proposal_digest` (ADR 0038's 2026-09-04 amendment, K-2 slice E2).
--
-- ONE MIGRATION, and that is a correctness requirement rather than a preference. SQLite's foreign
-- keys are MATCH SIMPLE: a composite key with any NULL component is SATISFIED, not checked. So the
-- moment `proposal_digest` becomes nullable, `(workspace_id, proposal_digest) -> proposals` stops
-- constraining Artifact-subject rows -- and if the subject columns and the rekeyed relations
-- landed in a later migration, every row written in between would reference nothing at all. The
-- nullable column, the subject columns, the CHECK that makes exactly one arm populated, and the
-- rekeyed `evaluation_supersessions` / `evaluations_scoped_identity` are inseparable.
--
-- PROVISIONAL NUMBER. `main` is at 0027 and the publication relation (gpt2's slice 3) also wants a
-- number in this window; whichever lands second renumbers at its final rebase. This migration does
-- NOT rebuild the closed `events` table -- `evaluation.recorded` already covers the arm -- so the
-- kind list is untouched. The FIRST later migration that rebuilds it must still carry every kind
-- `main` has, regenerated from its own parent.

-- `evaluation_findings` and `evaluation_cited_runs` carry foreign keys INTO `evaluations`, and a
-- modern `ALTER TABLE ... RENAME` REWRITES those references to follow the table -- so after this
-- rebuild drops `evaluations_v23` they would point at nothing, and the next insert into either
-- child fails with `no such table: main.evaluations_v23`. (Measured: the suite found it, and
-- `PRAGMA legacy_alter_table = ON` does not help because it is ineffective inside a transaction,
-- which every migration here runs in.) So both children are rebuilt too, from 0023's own text,
-- with their rows carried across. Everything that pointed at the old table moves with it: that is
-- what "one migration" means here.
ALTER TABLE evaluations RENAME TO evaluations_v23;
ALTER TABLE evaluation_findings RENAME TO evaluation_findings_v23;
ALTER TABLE evaluation_cited_runs RENAME TO evaluation_cited_runs_v23;
DROP TRIGGER evaluation_findings_no_update;
DROP TRIGGER evaluation_findings_no_delete;
DROP TRIGGER evaluation_cited_runs_no_update;
DROP TRIGGER evaluation_cited_runs_no_delete;
DROP INDEX evaluations_scoped_identity;
DROP INDEX evaluations_by_proposal;
DROP INDEX evaluations_workspace_digest;
ALTER TABLE evaluation_supersessions RENAME TO evaluation_supersessions_v23;
-- Every trigger 0023 defined ON `evaluations` comes off with the table and must be recreated
-- below: a rebuild that recreated only the ones it happened to name would silently drop a rule.
-- 0023's list on this table is exactly three -- no_self_evaluation, no_update, no_delete.
DROP TRIGGER evaluations_no_self_evaluation;
DROP TRIGGER evaluations_no_update;
DROP TRIGGER evaluations_no_delete;
DROP TRIGGER evaluation_supersessions_no_update;
DROP TRIGGER evaluation_supersessions_no_delete;

CREATE TABLE evaluations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.evaluation.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 21 AND 65536),

    -- The subject, generalized. `subject_kind` is the tag the bytes carry, and `subject_digest` is
    -- the identity every relation now keys on -- so a later arm adds columns here and rekeys
    -- nothing. It is a PROJECTION of the canonical bytes like every other column: the verified
    -- reader re-derives it and refuses a row that disagrees.
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('proposal', 'artifact')),
    subject_digest TEXT NOT NULL CHECK (
        length(subject_digest) = 64
        AND subject_digest NOT GLOB '*[^0-9a-f]*'
    ),

    -- Arm 0x01. Nullable now, and the CHECK below is what keeps it honest, because a composite FK
    -- with a NULL component is satisfied rather than checked.
    proposal_digest TEXT CHECK (
        proposal_digest IS NULL
        OR (length(proposal_digest) = 64 AND proposal_digest NOT GLOB '*[^0-9a-f]*')
    ),
    proposal_digest_algorithm TEXT CHECK (
        proposal_digest_algorithm IS NULL OR proposal_digest_algorithm = 'sha256'),
    proposal_digest_canonicalizer TEXT CHECK (
        proposal_digest_canonicalizer IS NULL
        OR proposal_digest_canonicalizer = 'beacon.proposal.v1'),

    -- Arm 0x03: the Artifact RECORD digest, its envelope, and the project half of the scope the
    -- record declares. The mission half is `mission_id` below -- one column, both arms, derived.
    artifact_digest TEXT CHECK (
        artifact_digest IS NULL
        OR (length(artifact_digest) = 64 AND artifact_digest NOT GLOB '*[^0-9a-f]*')
    ),
    artifact_digest_algorithm TEXT CHECK (
        artifact_digest_algorithm IS NULL OR artifact_digest_algorithm = 'sha256'),
    artifact_digest_canonicalizer TEXT CHECK (
        artifact_digest_canonicalizer IS NULL
        OR artifact_digest_canonicalizer = 'beacon.artifact.v1'),
    artifact_project_id TEXT,

    -- Derived, never in the Evaluation's own bytes for arm 0x01 (from the bound Proposal) and
    -- copied from the subject Artifact's own bytes for arm 0x03. One hop through a different
    -- record, the same column discipline: re-derived at every read and at startup.
    mission_id TEXT NOT NULL,
    evaluator_resident_id TEXT NOT NULL,
    evaluator_configuration_id TEXT NOT NULL,
    rubric_id TEXT NOT NULL CHECK (length(rubric_id) BETWEEN 1 AND 64),
    rubric_version INTEGER NOT NULL CHECK (rubric_version >= 1),
    rubric_canonicalizer TEXT NOT NULL CHECK (length(rubric_canonicalizer) BETWEEN 1 AND 64),
    rubric_sha256 TEXT NOT NULL CHECK (
        length(rubric_sha256) = 64
        AND rubric_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    verdict TEXT NOT NULL CHECK (verdict IN (
        'meets',
        'revision_requested',
        'does_not_meet',
        'uncertain'
    )),
    conflicts_declared INTEGER NOT NULL CHECK (conflicts_declared IN (0, 1)),
    conflicts_text TEXT CHECK (
        (conflicts_declared = 0 AND conflicts_text IS NULL)
        OR (conflicts_declared = 1 AND length(trim(conflicts_text)) BETWEEN 1 AND 2000)
    ),
    rationale TEXT NOT NULL CHECK (length(trim(rationale)) BETWEEN 1 AND 2000),
    evaluated_at_ms INTEGER NOT NULL CHECK (evaluated_at_ms >= 0),
    -- EXACTLY ONE ARM, and `subject_digest` is that arm's digest. Both halves matter: without the
    -- second, `subject_digest` could name a record neither column points at, and every relation
    -- keyed on it would be pointing at nothing.
    CHECK (
        (subject_kind = 'proposal'
            AND proposal_digest IS NOT NULL
            AND proposal_digest_algorithm IS NOT NULL
            AND proposal_digest_canonicalizer IS NOT NULL
            AND artifact_digest IS NULL
            AND artifact_digest_algorithm IS NULL
            AND artifact_digest_canonicalizer IS NULL
            AND artifact_project_id IS NULL
            AND subject_digest = proposal_digest)
        OR (subject_kind = 'artifact'
            AND artifact_digest IS NOT NULL
            AND artifact_digest_algorithm IS NOT NULL
            AND artifact_digest_canonicalizer IS NOT NULL
            AND artifact_project_id IS NOT NULL
            AND proposal_digest IS NULL
            AND proposal_digest_algorithm IS NULL
            AND proposal_digest_canonicalizer IS NULL
            AND subject_digest = artifact_digest)
    ),
    FOREIGN KEY (workspace_id, proposal_digest)
        REFERENCES proposals(workspace_id, digest_value) ON DELETE RESTRICT,
    -- The subject Artifact must exist in THIS Workspace. MATCH SIMPLE means this is satisfied for
    -- a Proposal-subject row, where `artifact_digest` is NULL -- which is why the CHECK above does
    -- the work of saying which arm is populated.
    FOREIGN KEY (workspace_id, artifact_digest)
        REFERENCES artifacts(workspace_id, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, mission_id, evaluator_resident_id, evaluator_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

INSERT INTO evaluations (
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    subject_kind, subject_digest,
    proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
    mission_id, evaluator_resident_id, evaluator_configuration_id,
    rubric_id, rubric_version, rubric_canonicalizer, rubric_sha256,
    verdict, conflicts_declared, conflicts_text, rationale, evaluated_at_ms
)
SELECT
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    'proposal', proposal_digest,
    proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
    mission_id, evaluator_resident_id, evaluator_configuration_id,
    rubric_id, rubric_version, rubric_canonicalizer, rubric_sha256,
    verdict, conflicts_declared, conflicts_text, rationale, evaluated_at_ms
FROM evaluations_v23
ORDER BY digest_value;

-- Rekeyed to the SUBJECT. This is the parent key the supersession relation needs, and it is now
-- kind-agnostic: a chain is per subject digest, and an Artifact version is a different subject.
CREATE UNIQUE INDEX evaluations_scoped_identity
ON evaluations (workspace_id, subject_digest, digest_value);

CREATE INDEX evaluations_by_subject
ON evaluations (workspace_id, subject_digest, evaluated_at_ms, digest_value);

CREATE UNIQUE INDEX evaluations_workspace_digest
ON evaluations (workspace_id, digest_value);

-- The two children, rebuilt from 0023's text so their foreign keys name the table that exists.
CREATE TABLE evaluation_findings (
    workspace_id TEXT NOT NULL,
    evaluation_digest TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    severity TEXT NOT NULL CHECK (severity IN ('blocking', 'concern', 'note')),
    summary TEXT NOT NULL CHECK (length(trim(summary)) BETWEEN 1 AND 500),
    PRIMARY KEY (workspace_id, evaluation_digest, ordinal),
    FOREIGN KEY (workspace_id, evaluation_digest)
        REFERENCES evaluations(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

INSERT INTO evaluation_findings (workspace_id, evaluation_digest, ordinal, severity, summary)
SELECT workspace_id, evaluation_digest, ordinal, severity, summary
FROM evaluation_findings_v23 ORDER BY evaluation_digest, ordinal;

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

INSERT INTO evaluation_cited_runs (workspace_id, evaluation_digest, run_id)
SELECT workspace_id, evaluation_digest, run_id
FROM evaluation_cited_runs_v23 ORDER BY evaluation_digest, run_id;

DROP TABLE evaluation_findings_v23;
DROP TABLE evaluation_cited_runs_v23;

CREATE TRIGGER evaluation_findings_no_update
BEFORE UPDATE ON evaluation_findings
BEGIN
    SELECT RAISE(ABORT, 'Evaluation findings are immutable');
END;

CREATE TRIGGER evaluation_findings_no_delete
BEFORE DELETE ON evaluation_findings
BEGIN
    SELECT RAISE(ABORT, 'Evaluation findings are immutable');
END;

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

CREATE TABLE evaluation_supersessions (
    workspace_id TEXT NOT NULL,
    -- Was `proposal_digest`. Chains are per SUBJECT: a new Artifact version is a new subject, and
    -- the old digest's Evaluations stay true of the old bytes rather than carrying over.
    subject_digest TEXT NOT NULL,
    evaluation_digest TEXT NOT NULL,
    supersedes_evaluation_digest TEXT NOT NULL,
    PRIMARY KEY (workspace_id, subject_digest, evaluation_digest),
    UNIQUE (workspace_id, subject_digest, supersedes_evaluation_digest),
    CHECK (evaluation_digest <> supersedes_evaluation_digest),
    FOREIGN KEY (workspace_id, subject_digest, evaluation_digest)
        REFERENCES evaluations(workspace_id, subject_digest, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, subject_digest, supersedes_evaluation_digest)
        REFERENCES evaluations(workspace_id, subject_digest, digest_value) ON DELETE RESTRICT
) STRICT;

INSERT INTO evaluation_supersessions (
    workspace_id, subject_digest, evaluation_digest, supersedes_evaluation_digest
)
SELECT workspace_id, proposal_digest, evaluation_digest, supersedes_evaluation_digest
FROM evaluation_supersessions_v23
ORDER BY evaluation_digest;

DROP TABLE evaluation_supersessions_v23;
DROP TABLE evaluations_v23;

CREATE TRIGGER evaluations_no_update
BEFORE UPDATE ON evaluations
BEGIN
    SELECT RAISE(ABORT, 'Evaluations are immutable');
END;

CREATE TRIGGER evaluations_no_delete
BEFORE DELETE ON evaluations
BEGIN
    SELECT RAISE(ABORT, 'Evaluations are immutable');
END;

CREATE TRIGGER evaluation_supersessions_no_update
BEFORE UPDATE ON evaluation_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Evaluation supersessions are immutable');
END;

CREATE TRIGGER evaluation_supersessions_no_delete
BEFORE DELETE ON evaluation_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Evaluation supersessions are immutable');
END;

-- NON-AUTHORSHIP, as a store rule rather than a comparison a caller must remember.
--
-- Keyed on the RESIDENT and never on the (Resident, Configuration) pair: an author who changes
-- configuration is still the author, and a pair comparison would let them evaluate their own work
-- by reconfiguring. One exclusion, not two -- the accountable Residents of OTHER versions in the
-- Artifact's supersession chain are not excluded, because the Resident who took a document over at
-- v3 is not the author of v1.
-- Carried forward from 0023 unchanged in meaning, and narrowed to its arm: a Proposal-subject row
-- may not name the Proposal's author. `proposal_digest` is NULL on an Artifact row, so the EXISTS
-- cannot match there -- but the arm is named explicitly rather than left to NULL semantics.
CREATE TRIGGER evaluations_no_self_evaluation
BEFORE INSERT ON evaluations
WHEN NEW.subject_kind = 'proposal'
  AND EXISTS (
    SELECT 1 FROM proposals
    WHERE workspace_id = NEW.workspace_id
      AND digest_value = NEW.proposal_digest
      AND requester_resident_id = NEW.evaluator_resident_id
)
BEGIN
    SELECT RAISE(ABORT, 'An Evaluation may not name the author of the Proposal it evaluates');
END;

CREATE TRIGGER evaluations_evaluator_is_not_the_artifact_author
BEFORE INSERT ON evaluations
WHEN NEW.subject_kind = 'artifact'
  AND EXISTS (
      SELECT 1 FROM artifacts
      WHERE artifacts.workspace_id = NEW.workspace_id
        AND artifacts.digest_value = NEW.artifact_digest
        AND artifacts.accountable_resident_id = NEW.evaluator_resident_id
  )
BEGIN
    SELECT RAISE(ABORT, 'The evaluator is the accountable Resident of the subject Artifact');
END;

-- The subject Artifact must carry the Mission scope this row derives, and it must be THE scope --
-- an Artifact with `context = none` has no roster to check an evaluator against, so it is not an
-- Evaluation subject in v1. The Command refuses it first; this is the store saying the same thing,
-- because a rule only a Command enforces is a rule a splice can walk past.
CREATE TRIGGER evaluations_artifact_subject_carries_its_mission
BEFORE INSERT ON evaluations
WHEN NEW.subject_kind = 'artifact'
  AND NOT EXISTS (
      SELECT 1 FROM artifacts
      WHERE artifacts.workspace_id = NEW.workspace_id
        AND artifacts.digest_value = NEW.artifact_digest
        AND artifacts.context_kind = 'mission'
        AND artifacts.context_mission_id = NEW.mission_id
        AND artifacts.context_project_id = NEW.artifact_project_id
  )
BEGIN
    SELECT RAISE(ABORT, 'The subject Artifact does not declare this Mission scope');
END;

UPDATE schema_meta SET version = 28 WHERE singleton = 1 AND version = 27;
