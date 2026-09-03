-- Candidate v1: the post-image digests of one proposed change, bound to one exact Proposal.
--
-- A Ruling names a Proposal; a Proposal names intended effects and carries NO content. Nothing in
-- either record's identity covers the bytes a dispatch would write. This table is that missing
-- link: one immutable, digest-identified record per Proposal, holding one entry per declared
-- mutation with the SHA-256 of the file bytes before and after.
--
-- A Candidate SELECTS bytes; it does not authorize them. Authority is `beacon.ruling.v2` below,
-- which binds this record's digest after its diff has been shown. Two rules make the selection
-- determinate:
--   * UNIQUE per Proposal, enforced here rather than by a Command that could be bypassed -- a
--     revised change is a new Proposal, which is what ADR 0034 already says about anything that
--     changes a Proposal's identity;
--   * ordered before the Ruling -- a dispatch-time comparison against `events.sequence`, the
--     store's own ordering primitive, so it needs no clock. This table does not enforce it.
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
        'proposal.candidate_recorded',
        'proposal.dispatched',
        'installation.paused',
        'installation.resumed'
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

-- At most ONE Candidate per Proposal. A second set of post-image digests would leave "which bytes"
-- ambiguous and force dispatch to choose. It does not choose: the store refuses the second record,
-- and a revised change is a new Proposal.
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

-- Ruling v2: the authority link. A Ruling that dispatch may honour binds the Candidate's digest
-- envelope, so the bytes the Operator was shown are named by the record that authorizes them.
-- Ruling v1 records stay valid and are simply not dispatchable, so their rows are copied untouched
-- and their canonicalizer column keeps its exact old value.
DROP TRIGGER rulings_no_update;
DROP TRIGGER rulings_no_delete;
DROP TRIGGER ruling_supersessions_no_update;
DROP TRIGGER ruling_supersessions_no_delete;
DROP INDEX rulings_scoped_identity;
DROP INDEX rulings_by_proposal;

-- The dependent relation moves first, so renaming `rulings` rewrites this preserved foreign key to
-- `rulings_v19`; both old tables stay until both copies exist. Same shape as migration 0016.
ALTER TABLE ruling_supersessions RENAME TO ruling_supersessions_v19;
ALTER TABLE rulings RENAME TO rulings_v19;

CREATE TABLE rulings (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    digest_algorithm TEXT NOT NULL CHECK (digest_algorithm = 'sha256'),
    -- Two grammars, both first-class. A v1 Ruling is a complete record of a decision; it simply
    -- names no Candidate, and dispatch refuses it for that reason rather than because it is old.
    digest_canonicalizer TEXT NOT NULL
        CHECK (digest_canonicalizer IN ('beacon.ruling.v1', 'beacon.ruling.v2')),
    digest_value TEXT NOT NULL PRIMARY KEY CHECK (
        length(digest_value) = 64
        AND digest_value NOT GLOB '*[^0-9a-f]*'
    ),
    canonical_bytes BLOB NOT NULL CHECK (length(canonical_bytes) BETWEEN 17 AND 65536),
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

INSERT INTO rulings (
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
    charter_revision, project_policy_revision, repository_revision, pinned_base_revision,
    decision, reason, decider_basis, ruled_at_ms
)
SELECT
    workspace_id, digest_algorithm, digest_canonicalizer, digest_value, canonical_bytes,
    proposal_digest, proposal_digest_algorithm, proposal_digest_canonicalizer,
    charter_revision, project_policy_revision, repository_revision, pinned_base_revision,
    decision, reason, decider_basis, ruled_at_ms
FROM rulings_v19;

CREATE UNIQUE INDEX rulings_scoped_identity
ON rulings (workspace_id, proposal_digest, digest_value);

CREATE INDEX rulings_by_proposal
ON rulings (workspace_id, proposal_digest, ruled_at_ms, digest_value);

CREATE TABLE ruling_supersessions (
    workspace_id TEXT NOT NULL,
    proposal_digest TEXT NOT NULL,
    ruling_digest TEXT NOT NULL,
    supersedes_ruling_digest TEXT NOT NULL,
    PRIMARY KEY (workspace_id, proposal_digest, ruling_digest),
    UNIQUE (workspace_id, proposal_digest, supersedes_ruling_digest),
    CHECK (ruling_digest <> supersedes_ruling_digest),
    FOREIGN KEY (workspace_id, proposal_digest, ruling_digest)
        REFERENCES rulings(workspace_id, proposal_digest, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, proposal_digest, supersedes_ruling_digest)
        REFERENCES rulings(workspace_id, proposal_digest, digest_value) ON DELETE RESTRICT
) STRICT;

INSERT INTO ruling_supersessions (
    workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
)
SELECT workspace_id, proposal_digest, ruling_digest, supersedes_ruling_digest
FROM ruling_supersessions_v19;

DROP TABLE ruling_supersessions_v19;
DROP TABLE rulings_v19;

-- The Candidate a v2 Ruling binds, as a RELATION rather than a nullable column: "this Ruling names
-- no Candidate" is the absence of a row, exactly as supersession models "no predecessor". A v2
-- Ruling has exactly one row here and a v1 Ruling has none, and reconciliation checks that the
-- relation and the canonicalizer in the bytes agree.
CREATE TABLE ruling_candidates (
    workspace_id TEXT NOT NULL,
    proposal_digest TEXT NOT NULL,
    ruling_digest TEXT NOT NULL,
    candidate_digest TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ruling_digest),
    FOREIGN KEY (workspace_id, proposal_digest, ruling_digest)
        REFERENCES rulings(workspace_id, proposal_digest, digest_value) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, candidate_digest)
        REFERENCES candidates(workspace_id, digest_value) ON DELETE RESTRICT
) STRICT;

-- The Candidate's parent key, scoped by Workspace like every other binding in this store.
CREATE UNIQUE INDEX candidates_workspace_digest
ON candidates (workspace_id, digest_value);

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

CREATE TRIGGER ruling_candidates_no_update
BEFORE UPDATE ON ruling_candidates
BEGIN
    SELECT RAISE(ABORT, 'Ruling candidate bindings are immutable');
END;

CREATE TRIGGER ruling_candidates_no_delete
BEFORE DELETE ON ruling_candidates
BEGIN
    SELECT RAISE(ABORT, 'Ruling candidate bindings are immutable');
END;

-- Dispatch: the Run kind, its states, and the Receipt vocabulary. This build has no landing path,
-- so `applied` and `landing_unobserved` are declared here and emitted by no code yet -- the
-- vocabulary lands once, with the act it belongs to, rather than in pieces a store could be caught
-- between.
DROP TRIGGER runs_no_delete;
DROP TRIGGER runs_single_terminal_transition;
DROP TRIGGER receipts_no_update;
DROP TRIGGER receipts_no_delete;
DROP INDEX runs_project_started;
DROP INDEX runs_open_lease;
-- Migration 0020's ordering index too: a rename carries indexes with the table, so one left
-- behind collides when the new table recreates it.
DROP INDEX runs_workspace_started;

ALTER TABLE receipts RENAME TO receipts_v21;
ALTER TABLE runs RENAME TO runs_v21;

CREATE TABLE runs (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('check_preflight', 'declared_checks', 'proposal_dispatch')),
    repository_revision INTEGER NOT NULL CHECK (repository_revision >= 1),
    charter_revision INTEGER NOT NULL CHECK (charter_revision >= 1),
    -- `dispatching` is a dispatch Run's open state, beside preflight's `preflighting`. A Run
    -- advances once, from its own open state to a terminal one.
    state TEXT NOT NULL CHECK (state IN (
        'preflighting',
        'dispatching',
        'refused',
        'verified',
        'incomplete',
        'applied',
        'unconfirmed'
    )),
    lease_expires_at_ms INTEGER NOT NULL CHECK (lease_expires_at_ms >= 0),
    started_at_ms INTEGER NOT NULL CHECK (started_at_ms >= 0),
    ended_at_ms INTEGER CHECK (ended_at_ms IS NULL OR ended_at_ms >= started_at_ms),
    PRIMARY KEY (workspace_id, id),
    FOREIGN KEY (workspace_id, project_id)
        REFERENCES projects(workspace_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (workspace_id, project_id, repository_revision)
        REFERENCES project_repository_revisions(workspace_id, project_id, revision) ON DELETE RESTRICT
) STRICT;

INSERT INTO runs (
    workspace_id, id, project_id, kind, repository_revision, charter_revision,
    state, lease_expires_at_ms, started_at_ms, ended_at_ms
)
SELECT
    workspace_id, id, project_id, kind, repository_revision, charter_revision,
    state, lease_expires_at_ms, started_at_ms, ended_at_ms
FROM runs_v21;

CREATE INDEX runs_project_started
ON runs (workspace_id, project_id, started_at_ms DESC);

CREATE INDEX runs_open_lease
ON runs (state, lease_expires_at_ms)
WHERE state IN ('preflighting', 'dispatching');

-- Migration 0020's ordering index, recreated with the table it belongs to: Briefing's recent-Receipt
-- Query is bounded only while this exists, and rebuilding `runs` takes its indexes with it.
CREATE INDEX runs_workspace_started
ON runs (workspace_id, started_at_ms DESC, id DESC);

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

INSERT INTO receipts (workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms)
SELECT workspace_id, run_id, version, outcome, receipt_json, recorded_at_ms
FROM receipts_v21;

DROP TABLE receipts_v21;
DROP TABLE runs_v21;

CREATE TRIGGER runs_no_delete
BEFORE DELETE ON runs
BEGIN
    SELECT RAISE(ABORT, 'Runs are immutable once opened');
END;

CREATE TRIGGER runs_single_terminal_transition
BEFORE UPDATE ON runs
WHEN OLD.state NOT IN ('preflighting', 'dispatching')
  OR NEW.state NOT IN ('refused', 'verified', 'incomplete', 'applied', 'unconfirmed')
  OR OLD.ended_at_ms IS NOT NULL
  OR NEW.ended_at_ms IS NULL
  OR NEW.id <> OLD.id
  OR NEW.workspace_id <> OLD.workspace_id
  OR NEW.project_id <> OLD.project_id
  OR NEW.kind <> OLD.kind
  OR NEW.repository_revision <> OLD.repository_revision
  OR NEW.charter_revision <> OLD.charter_revision
  OR NEW.started_at_ms <> OLD.started_at_ms
  OR NEW.lease_expires_at_ms <> OLD.lease_expires_at_ms
BEGIN
    SELECT RAISE(ABORT, 'A Run advances once from its open state to a terminal state');
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

-- What a dispatch prepared, written BEFORE the irreversible step.
--
-- Two jobs, and only the second is about races. It records the exact commit an attempt was about to
-- publish, so an attempt whose result was never established can name the commit to inspect -- a
-- worktree and an in-memory Candidate are not recovery state (gpt1). And its unique index spends
-- the authority: one preparation per Ruling, so a second dispatch of one decision cannot even
-- prepare, and an uncertain attempt stays spent until the Operator resolves it. The compare-and-swap
-- is what makes application exactly once (R9); this is what makes a repeat attempt refuse rather
-- than race.
CREATE TABLE dispatch_preparations (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    run_id TEXT NOT NULL,
    ruling_digest TEXT NOT NULL CHECK (
        length(ruling_digest) = 64
        AND ruling_digest NOT GLOB '*[^0-9a-f]*'
    ),
    source TEXT NOT NULL CHECK (length(source) BETWEEN 1 AND 4096),
    -- The device and inode of the directory the gates measured. A pathname is what a rename moves;
    -- this is what "the same repository" means to the kernel (gpt1's path-swap probe).
    source_device INTEGER NOT NULL,
    source_inode INTEGER NOT NULL,
    branch TEXT NOT NULL CHECK (length(branch) BETWEEN 1 AND 512),
    pinned_base_revision TEXT NOT NULL CHECK (
        length(pinned_base_revision) = 40
        AND pinned_base_revision NOT GLOB '*[^0-9a-f]*'
    ),
    prepared_commit TEXT NOT NULL CHECK (
        length(prepared_commit) = 40
        AND prepared_commit NOT GLOB '*[^0-9a-f]*'
    ),
    prepared_at_ms INTEGER NOT NULL CHECK (prepared_at_ms >= 0),
    PRIMARY KEY (workspace_id, run_id),
    FOREIGN KEY (workspace_id, run_id) REFERENCES runs(workspace_id, id) ON DELETE RESTRICT
) STRICT;

-- One preparation per Ruling, ever.
CREATE UNIQUE INDEX dispatch_preparations_one_per_ruling
ON dispatch_preparations (workspace_id, ruling_digest);

CREATE TRIGGER dispatch_preparations_no_update
BEFORE UPDATE ON dispatch_preparations
BEGIN
    SELECT RAISE(ABORT, 'Dispatch preparations are immutable');
END;

CREATE TRIGGER dispatch_preparations_no_delete
BEFORE DELETE ON dispatch_preparations
BEGIN
    SELECT RAISE(ABORT, 'Dispatch preparations are immutable');
END;

-- Repositories Beacon made, and may therefore dispatch into.
--
-- The provenance gate used to ask only whether a path was under a directory the caller chose, so
-- any repository placed there qualified and the caller chose the root (gpt1). Origin is a fact
-- about who created a directory, and only Beacon can record that it did. The device and inode are
-- stored beside the path because a pathname is what a rename moves.
CREATE TABLE beacon_repositories (
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE RESTRICT,
    path TEXT NOT NULL CHECK (length(path) BETWEEN 1 AND 4096),
    device INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    origin TEXT NOT NULL CHECK (origin = 'sample_project_copy'),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    PRIMARY KEY (workspace_id, path)
) STRICT;

CREATE TRIGGER beacon_repositories_no_update
BEFORE UPDATE ON beacon_repositories
BEGIN
    SELECT RAISE(ABORT, 'Beacon repository provenance is immutable');
END;

-- Pause Work, as the store's own fact. BLUEPRINT:377-379 and AGENTS.md:128 make a paused
-- Installation refuse new dispatches, and a Receipt line saying "pause not verified" does not make
-- a forbidden dispatch permissible (gpt1). This is the minimum that makes the gate real: one
-- singleton row an Owner Command sets. The rest of Pause -- interruption requests, Worker lease
-- renewals, the tray and shortcut surfaces -- is not built and is not claimed.
CREATE TABLE installation_pause (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    paused INTEGER NOT NULL CHECK (paused IN (0, 1)),
    changed_at_ms INTEGER NOT NULL CHECK (changed_at_ms >= 0)
) STRICT;

INSERT INTO installation_pause (singleton, paused, changed_at_ms) VALUES (1, 0, 0);

UPDATE schema_meta SET version = 22 WHERE singleton = 1 AND version = 20;
