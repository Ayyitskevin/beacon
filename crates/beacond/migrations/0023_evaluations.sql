-- An Evaluation records one non-author Resident's verdict about one exact Proposal digest
-- (ADR 0038). It confers no authority: nothing here opens a Gate, admits a dispatch, or answers a
-- question with a boolean. A Ruling does not cite an Evaluation in v1 -- that relation is deferred
-- until `beacon.ruling.v2` can cover the cited set with the Ruling's own digest.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v22;

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
        'proposal.submitted',
        'ruling.recorded',
        'proposal.candidate_recorded',
        'proposal.dispatched',
        'installation.paused',
        'installation.resumed',
        'evaluation.recorded'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v22
ORDER BY sequence;

DROP TABLE events_v22;

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


-- One Evaluation, its canonical bytes, and the identity those bytes hash to.
--
-- `mission_id` is the one column with NO counterpart in the canonical bytes: the Evaluation's
-- grammar carries no Mission. It is copied from the bound Proposal's row at insert so the
-- evaluator-membership foreign key has a parent to point at, which is why startup re-derives it
-- against the PROPOSAL rather than against the bytes -- there are no bytes to compare it to.
CREATE TABLE evaluations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    -- The Evaluation's own identity: sha256 over its canonical bytes under `beacon.evaluation.v1`.
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.evaluation.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    -- The upper bound repeats `MAX_CANONICAL_BYTES`, which the encoder enforces first. The
    -- per-field limits alone admit 145,941 bytes, so without an owned total bound a record every
    -- field table accepts would be refused here instead -- a database error surfacing through a
    -- Command rather than a fact about the bytes.
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 21 AND 65536),
    -- The bound Proposal's identity, COPIED whole: value, algorithm and canonicalizer, so a reader
    -- never has to assume which function produced the digest it is looking at.
    proposal_digest TEXT NOT NULL CHECK (
        length(proposal_digest) = 64
        AND proposal_digest NOT GLOB '*[^0-9a-f]*'
    ),
    proposal_digest_algorithm TEXT NOT NULL CHECK (proposal_digest_algorithm = 'sha256'),
    proposal_digest_canonicalizer TEXT NOT NULL
        CHECK (proposal_digest_canonicalizer = 'beacon.proposal.v1'),
    -- Derived from the bound Proposal, not from the bytes. See the note above.
    mission_id TEXT NOT NULL,
    evaluator_resident_id TEXT NOT NULL,
    evaluator_configuration_id TEXT NOT NULL,
    -- A SELF-REPORT. Nothing in Beacon resolves a rubric: there is no rubric table, no owning
    -- module, and no foreign key. These four columns say what the evaluator DECLARED they applied,
    -- and no surface may present a rubric as applied or verified on their strength alone.
    rubric_id TEXT NOT NULL CHECK (length(rubric_id) BETWEEN 1 AND 64),
    rubric_version INTEGER NOT NULL CHECK (rubric_version >= 1),
    rubric_canonicalizer TEXT NOT NULL CHECK (length(rubric_canonicalizer) BETWEEN 1 AND 64),
    rubric_sha256 TEXT NOT NULL CHECK (
        length(rubric_sha256) = 64
        AND rubric_sha256 NOT GLOB '*[^0-9a-f]*'
    ),
    -- `uncertain` is a first-class outcome, not a soft rejection, and none of the four is
    -- `approved`: approval is a Ruling word and this record must not lend it.
    verdict TEXT NOT NULL CHECK (verdict IN (
        'meets',
        'revision_requested',
        'does_not_meet',
        'uncertain'
    )),
    -- Absence is a VALUE here: `:630` requires conflicts to be recorded, so "none declared" is a
    -- statement rather than a missing field. The text is present exactly when the tag says so.
    conflicts_declared INTEGER NOT NULL CHECK (conflicts_declared IN (0, 1)),
    conflicts_text TEXT CHECK (
        (conflicts_declared = 0 AND conflicts_text IS NULL)
        OR (conflicts_declared = 1 AND length(trim(conflicts_text)) BETWEEN 1 AND 2000)
    ),
    rationale TEXT NOT NULL CHECK (length(trim(rationale)) BETWEEN 1 AND 2000),
    -- Service clock, never caller input: a caller-supplied time would let an Evaluation recorded
    -- after a Ruling be backdated through the ordinary Command.
    evaluated_at_ms INTEGER NOT NULL CHECK (evaluated_at_ms >= 0),
    FOREIGN KEY (workspace_id, proposal_digest)
        REFERENCES proposals(workspace_id, digest_value) ON DELETE RESTRICT,
    -- The evaluator must be an admitted member of the bound Proposal's Mission under the exact
    -- configuration named. Membership is structural; NON-AUTHORSHIP is slice 2's trigger.
    FOREIGN KEY (workspace_id, mission_id, evaluator_resident_id, evaluator_configuration_id)
        REFERENCES mission_admission_members(
            workspace_id, mission_id, resident_id, configuration_id
        ) ON DELETE RESTRICT
) STRICT;

-- An Evaluation is identified within its Workspace AND its Proposal, so a later relation cannot
-- reach across either. This is the parent key slice 3's supersession relation needs.
CREATE UNIQUE INDEX evaluations_scoped_identity
ON evaluations (workspace_id, proposal_digest, digest_value);

CREATE INDEX evaluations_by_proposal
ON evaluations (workspace_id, proposal_digest, evaluated_at_ms, digest_value);

-- Findings are a SEQUENCE and their order is semantic: the evaluator wrote them in an order, and
-- re-sorting by severity would be the interface deciding what mattered. A relation rather than a
-- JSON blob so the count is verifiable at startup and the rows are queryable.
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

-- Cited Runs are a SET: ascending raw-UUID order in the bytes, duplicates refused, so a permutation
-- of the same inputs cannot produce a different Evaluation identity.
--
-- The foreign key targets `receipts`, not `runs`. A cited Run must have recorded a Receipt, and
-- pointing at `runs` would leave that true in prose and structurally permitted.
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

CREATE UNIQUE INDEX evaluations_workspace_digest
ON evaluations (workspace_id, digest_value);

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

UPDATE schema_meta SET version = 23 WHERE singleton = 1 AND version = 22;
