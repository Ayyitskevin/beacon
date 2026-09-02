-- Candidate v1: the post-image digests of one proposed change, bound to one exact Proposal.
--
-- A Ruling names a Proposal; a Proposal names intended effects and carries NO content. Nothing in
-- either record's identity covers the bytes a dispatch would write. This table is that missing
-- link: one immutable, digest-identified record per Proposal, holding one entry per declared
-- mutation with the SHA-256 of the file bytes before and after.
--
-- Two structural rules make it authority-bearing rather than decorative, and both are here rather
-- than in a Command that could be bypassed:
--   * UNIQUE per Proposal -- a revised change is a new Proposal, which is what ADR 0034 already
--     says about anything that changes a Proposal's identity;
--   * ordered before the Ruling -- enforced at dispatch time against `events.sequence`, the
--     store's own ordering primitive, so it needs no clock.
--
-- This file lands the vocabulary the dispatch slices write as they write it. It stays a single
-- migration for the whole act (ADR 0039) rather than one per slice, and is extended in place while
-- the branch is unmerged, so no released store ever sees a partial dispatch vocabulary.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v20;

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
        'proposal.candidate_recorded'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v20
ORDER BY sequence;

DROP TABLE events_v20;

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

CREATE TABLE candidates (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    -- The Candidate's own identity: sha256 over its canonical bytes under `beacon.candidate.v1`.
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.candidate.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 20 AND 65536),
    -- The bound Proposal's identity, COPIED whole, exactly as a Ruling copies it: a digest carries
    -- its algorithm and canonicalizer everywhere in this store rather than inheriting either by
    -- assumption.
    proposal_digest TEXT NOT NULL CHECK (
        length(proposal_digest) = 64
        AND proposal_digest NOT GLOB '*[^0-9a-f]*'
    ),
    proposal_digest_algorithm TEXT NOT NULL CHECK (proposal_digest_algorithm = 'sha256'),
    proposal_digest_canonicalizer TEXT NOT NULL
        CHECK (proposal_digest_canonicalizer = 'beacon.proposal.v1'),
    -- Copied from the same verified Proposal. The pre-image digests are facts about the tree at
    -- this revision and mean nothing at another.
    pinned_base_revision TEXT NOT NULL CHECK (
        length(pinned_base_revision) = 40
        AND pinned_base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    entry_count INTEGER NOT NULL CHECK (entry_count >= 1),
    recorded_at_ms INTEGER NOT NULL CHECK (recorded_at_ms >= 0),
    FOREIGN KEY (workspace_id, proposal_digest)
        REFERENCES proposals(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

-- At most ONE Candidate per Proposal. A second set of post-image digests for one decision would
-- make "the bytes the Operator ruled on" ambiguous, and dispatch would have to choose. It does not
-- choose: the store refuses the second record, and a revised change is a new Proposal.
CREATE UNIQUE INDEX candidates_one_per_proposal
ON candidates (workspace_id, proposal_digest);

CREATE TRIGGER candidates_no_update
BEFORE UPDATE ON candidates
BEGIN
    SELECT RAISE(ABORT, 'Candidates are immutable');
END;

CREATE TRIGGER candidates_no_delete
BEFORE DELETE ON candidates
BEGIN
    SELECT RAISE(ABORT, 'Candidates are immutable');
END;

UPDATE schema_meta SET version = 22 WHERE singleton = 1 AND version = 20;
