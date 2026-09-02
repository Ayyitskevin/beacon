-- A Ruling records that a human decided about one exact Proposal. It creates no execution
-- authority: dispatch re-verifies the binding independently before acting on anything.
DROP TRIGGER team_formation_members_no_insert_after_formation;
DROP TRIGGER mission_admission_members_no_insert_after_admission;

ALTER TABLE events RENAME TO events_v18;

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
        'ruling.recorded'
    )),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at_ms INTEGER NOT NULL CHECK (occurred_at_ms >= 0)
) STRICT;

INSERT INTO events (sequence, workspace_id, kind, payload_json, occurred_at_ms)
SELECT sequence, workspace_id, kind, payload_json, occurred_at_ms
FROM events_v18
ORDER BY sequence;

DROP TABLE events_v18;

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

-- The parent key a Ruling binds to. `proposals.digest_value` is already unique on its own, but the
-- composite index lets the foreign key carry Workspace scope, as every other binding in this store
-- does rather than trusting a global identifier to stay in its lane.
CREATE UNIQUE INDEX proposals_workspace_digest
ON proposals (workspace_id, digest_value);

CREATE TABLE rulings (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    -- The Ruling's own identity: sha256 over its canonical bytes under `beacon.ruling.v1`.
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    digest_canonicalizer TEXT NOT NULL CHECK (digest_canonicalizer = 'beacon.ruling.v1'),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 17 AND 65536),
    -- The bound Proposal's identity, COPIED whole. A Ruling names the exact bytes it ruled on, and
    -- every digest carries its own algorithm and canonicalizer rather than inheriting either by
    -- assumption -- two envelopes, both explicit.
    proposal_digest TEXT NOT NULL CHECK (
        length(proposal_digest) = 64
        AND proposal_digest NOT GLOB '*[^0-9a-f]*'
    ),
    proposal_digest_algorithm TEXT NOT NULL CHECK (proposal_digest_algorithm = 'sha256'),
    proposal_digest_canonicalizer TEXT NOT NULL
        CHECK (proposal_digest_canonicalizer = 'beacon.proposal.v1'),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    project_policy_revision INTEGER NOT NULL CHECK (project_policy_revision >= 1),
    repository_revision INTEGER NOT NULL CHECK (repository_revision >= 1),
    pinned_base_revision TEXT NOT NULL CHECK (
        length(pinned_base_revision) = 40
        AND pinned_base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    decision TEXT NOT NULL CHECK (decision IN ('approve', 'refuse')),
    reason TEXT NOT NULL CHECK (length(trim(reason)) BETWEEN 1 AND 2000),
    decider_basis TEXT NOT NULL CHECK (decider_basis = 'owner'),
    ruled_at_ms INTEGER NOT NULL CHECK (ruled_at_ms >= 0),
    FOREIGN KEY (workspace_id, proposal_digest)
        REFERENCES proposals(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

-- Parent key for the supersession relation: a Ruling is identified within its Workspace AND its
-- Proposal, so a link cannot reach across either.
CREATE UNIQUE INDEX rulings_scoped_identity
ON rulings (workspace_id, proposal_digest, digest_value);

CREATE INDEX rulings_by_proposal
ON rulings (workspace_id, proposal_digest, ruled_at_ms, digest_value);

-- Supersession is a relation, not a nullable column. "This Ruling has no predecessor" is the
-- ABSENCE OF A ROW, which carries no meaning that a NULL would have to be interpreted to supply:
-- the canonical bytes say `supersedes = none` and reconciliation checks the two agree. Both foreign
-- keys are scoped by (workspace_id, proposal_digest), so a link across Workspaces or across
-- Proposals is structurally impossible rather than merely refused by a Command that could be
-- bypassed.
CREATE TABLE ruling_supersessions (
    workspace_id TEXT NOT NULL,
    proposal_digest TEXT NOT NULL,
    ruling_digest TEXT NOT NULL,
    supersedes_ruling_digest TEXT NOT NULL,
    -- Each Ruling cites at most one predecessor...
    PRIMARY KEY (workspace_id, proposal_digest, ruling_digest),
    -- ...and each Ruling is superseded at most once. Together these make the graph a set of
    -- disjoint paths: no forks, so "the effective Ruling is the last link" always has an answer.
    UNIQUE (workspace_id, proposal_digest, supersedes_ruling_digest),
    CHECK (ruling_digest <> supersedes_ruling_digest),
    FOREIGN KEY (workspace_id, proposal_digest, ruling_digest)
        REFERENCES rulings(workspace_id, proposal_digest, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, proposal_digest, supersedes_ruling_digest)
        REFERENCES rulings(workspace_id, proposal_digest, digest_value) ON DELETE RESTRICT
) STRICT;

CREATE TRIGGER rulings_no_update
BEFORE UPDATE ON rulings
BEGIN
    SELECT RAISE(ABORT, 'Rulings are immutable');
END;

CREATE TRIGGER rulings_no_delete
BEFORE DELETE ON rulings
BEGIN
    SELECT RAISE(ABORT, 'Rulings are immutable');
END;

CREATE TRIGGER ruling_supersessions_no_update
BEFORE UPDATE ON ruling_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Ruling supersessions are immutable');
END;

CREATE TRIGGER ruling_supersessions_no_delete
BEFORE DELETE ON ruling_supersessions
BEGIN
    SELECT RAISE(ABORT, 'Ruling supersessions are immutable');
END;

UPDATE schema_meta SET version = 19 WHERE singleton = 1 AND version = 18;
