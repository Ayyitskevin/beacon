-- Mosaic unified store — the integrity layer.
--
-- This file exists to make four distinctions impossible to lose, because each
-- was found collapsed in live code during the design phase:
--
--   absent      nothing here
--   dead        this is over
--   broken      this failed
--   transient   we did not look this run
--
-- The rule that generates most of what follows: **the interpretation lives in
-- the row, not the reader's head.** A value whose meaning depends on an
-- unstated convention must carry the convention -- the zone beside the
-- instant, the algorithm beside the digest, the kind beside the edge, an
-- explicit state instead of a NULL.
--
-- What this schema enforces, and what it does NOT:
--   ENFORCED HERE   an unknown enum value cannot be stored at all (CHECK with
--                   an explicit value list -- the database is the reader that
--                   refuses), and NULL is never the signal.
--   NOT ENFORCED    the other two totality checks. "A field was added to a
--                   type the reader ignores" and "a column no model claims"
--                   are application-level round-trip and reconciliation tests
--                   (tests/integrity/). Claiming the schema covers them would
--                   be the overclaim the read-only AST tests already made once.

PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------------
-- 1. Instants: the zone travels with the value
-- ---------------------------------------------------------------------------
-- Measured during design: Athena stores naive strings that ARE UTC, but only
-- because SQLite's datetime('now') happens to be. Nothing asserted it -- not
-- the column type, not a constraint, not a test -- so one INSERT of a naive
-- local value would have put a 4h error in the same column undetectably.
-- Three source encodings exist across the seven: ISO-8601Z, naive-UTC, epoch
-- int. Normalised on the way in; the source encoding is kept so that "was this
-- really UTC?" stays answerable after the source system retires.

CREATE TABLE instant_encoding (
    name TEXT PRIMARY KEY
) STRICT;
INSERT INTO instant_encoding(name) VALUES
    ('iso8601z'),      -- explicit offset, unambiguous
    ('naive_utc'),     -- no offset; UTC established by reading what WROTE it
    ('epoch_seconds'),
    ('epoch_millis');

-- ---------------------------------------------------------------------------
-- 2. Provenance: an entry that cannot name its source is a bug
-- ---------------------------------------------------------------------------
CREATE TABLE origin_system (
    name TEXT PRIMARY KEY
) STRICT;
INSERT INTO origin_system(name) VALUES
    ('buzz'), ('athena'), ('minerva'), ('vulcan'), ('janus'), ('icarus'),
    ('replaydeck'),   -- greenfield: no migration reader, nothing to read from
    ('mosaic');       -- native, not migrated

CREATE TABLE value_origin (
    name TEXT PRIMARY KEY
) STRICT;
INSERT INTO value_origin(name) VALUES
    ('observed'),   -- read from the source as a stored fact
    ('derived'),    -- the source computed it at read time; see §5
    ('native');     -- produced by Mosaic itself

-- ---------------------------------------------------------------------------
-- 3. Digests: the algorithm travels with the value
-- ---------------------------------------------------------------------------
-- Measured: Icarus's stableJson is JS-specific. An integral float digests as
-- `5` under JSON.stringify and `5.0` under Python's json.dumps, so a
-- re-implementation silently produces a different hash -- which does not read
-- as a bug, it reads as DRIFT, the one state that demands a human act now.
-- Re-deriving migrated digests would fire a false ACT alarm across every
-- migrated approval at once. Kevin ruled: vendor the original canonicaliser
-- frozen. This table is what makes that decision durable rather than a note.

CREATE TABLE digest_algorithm (
    name        TEXT PRIMARY KEY,
    frozen_at   TEXT NOT NULL,
    description TEXT NOT NULL
) STRICT;
INSERT INTO digest_algorithm(name, frozen_at, description) VALUES
    ('icarus.stableJson.v1', '2026-08-25',
     'Vendored frozen from Icarus packages/core/src/digest.ts. JS-specific: JSON.stringify number formatting and UTF-16 code-unit key ordering. MUST NOT be reimplemented -- vendor it or mark the digest unverifiable.'),
    ('mosaic.canonical.v1', '2026-08-25',
     'Native canonicalisation for digests Mosaic computes itself.');

-- ---------------------------------------------------------------------------
-- 4. References: unresolved is the steady state, not an edge case
-- ---------------------------------------------------------------------------
-- A reference is a triple (capability, ref, origin), never a bare string:
-- `MWS-2`, a 64-hex Buzz event id and `g3410c2cff0f` are all "an id" and none
-- are unique across the merged set.
--
-- Four states, not two. During a multi-month migration MOST references are
-- unresolved -- that is the steady state. Collapsing unresolved into broken
-- would fire "broken reference" continuously across a half-migrated store,
-- which is the alarm-fatigue failure Janus's own resolve_binding warns about:
-- drift that fires constantly teaches the reader to ignore drift.

CREATE TABLE reference_state (
    name TEXT PRIMARY KEY
) STRICT;
INSERT INTO reference_state(name) VALUES
    ('resolved'),   -- target exists in this store, and target_id names it
    ('pending'),    -- looked; the target's system has not been migrated yet
    ('dangling'),   -- looked; the target's system IS migrated and it is absent
    ('unchecked');  -- transient: we did not look this run

CREATE TABLE entity_reference (
    id                INTEGER PRIMARY KEY,
    from_entity_id    INTEGER NOT NULL REFERENCES entity(id),
    capability        TEXT    NOT NULL REFERENCES origin_system(name),
    ref               TEXT    NOT NULL,   -- the id as the ORIGIN system spells it
    origin            TEXT    NOT NULL REFERENCES origin_system(name),
    state             TEXT    NOT NULL REFERENCES reference_state(name),
    target_entity_id  INTEGER          REFERENCES entity(id),
    checked_at        TEXT,
    checked_encoding  TEXT             REFERENCES instant_encoding(name),

    -- NULL is never the signal. `target_entity_id IS NULL` does not mean
    -- anything on its own; `state` does, and these tie the two together so a
    -- half-written row cannot exist.
    CHECK (state <> 'resolved' OR target_entity_id IS NOT NULL),
    CHECK (state =  'resolved' OR target_entity_id IS NULL),
    -- Every state except 'unchecked' is a claim about a moment; a claim with
    -- no moment cannot be aged out or re-verified.
    CHECK (state =  'unchecked' OR checked_at IS NOT NULL),
    CHECK (checked_at IS NULL   OR checked_encoding IS NOT NULL)
) STRICT;

-- ---------------------------------------------------------------------------
-- 5. Derived values: three kinds, and only one is safe to store
-- ---------------------------------------------------------------------------
-- Measured in Athena's aegis/fleet_work.py::_item(), which computes three
-- values at read time that are NOT the same kind of thing:
--
--   clock_derived        claim_state       stale the instant it is written
--   cross_table_derived  reporting_state   stale, AND silently wrong if the
--                                          other table moves -- fails
--                                          non-locally, which is why it
--                                          survives review
--   row_derived          attention_reasons recomputable from the same row;
--                                          materialising is a perf choice
--
-- The flat rule ("never store a derived value") is wrong in both directions:
-- it forbids the harmless case and gives no reason to fear the dangerous one.

CREATE TABLE derivation_kind (
    name           TEXT PRIMARY KEY,
    storable       INTEGER NOT NULL CHECK (storable IN (0, 1)),
    rationale      TEXT NOT NULL
) STRICT;
INSERT INTO derivation_kind(name, storable, rationale) VALUES
    ('clock_derived', 0,
     'A pure function of the clock. Storing it freezes a verdict that is stale the instant it is written. Compute at read from its inputs.'),
    ('cross_table_derived', 0,
     'Depends on another table plus the clock. Stale AND silently wrong if that table moves. Fails non-locally; the most dangerous of the three.'),
    ('row_derived', 1,
     'Computable from the same row. Materialising duplicates rather than lies. A performance choice, not a correctness one.');

-- Where a derived value IS stored, the inputs must be stored too: a derived
-- value whose inputs were not migrated cannot be recomputed, only trusted --
-- which is bytes-unverifiable wearing a different hat.
CREATE TABLE derived_value (
    id              INTEGER PRIMARY KEY,
    entity_id       INTEGER NOT NULL REFERENCES entity(id),
    field           TEXT    NOT NULL,
    kind            TEXT    NOT NULL REFERENCES derivation_kind(name),
    inputs_present  INTEGER NOT NULL CHECK (inputs_present IN (0, 1)),
    input_fields    TEXT,          -- JSON array naming them
    CHECK (kind = 'row_derived'),  -- the other two are not storable at all
    CHECK (inputs_present = 1),
    CHECK (input_fields IS NOT NULL)
) STRICT;

-- ---------------------------------------------------------------------------
-- 6. Evaluation (ReplayDeck) — GREENFIELD
-- ---------------------------------------------------------------------------
-- Measured during design: ReplayDeck has never been built. The roadmap calls
-- it "the working codename for the existing fleet-eval repo"; that repo is
-- absent from both nodes and from GitHub. So there is no migration reader for
-- it and no prior art to lift -- this schema is DESIGNED, not derived from a
-- running system, and is marked so it never reads with the authority of the
-- measured sections.
--
-- Four-valued outcome, because a `passed BOOLEAN` would collapse all four on
-- day one -- the absent/dead/broken/transient quartet applied to replay.

CREATE TABLE replay_outcome (
    name TEXT PRIMARY KEY
) STRICT;
INSERT INTO replay_outcome(name) VALUES
    ('reproduced'),     -- ran; output matched
    ('diverged'),       -- ran; output did not match
    ('not_replayable'), -- the inputs no longer exist: permanent, not a failure
    ('not_attempted');  -- transient: we did not run it this time

CREATE TABLE replay_run (
    id                INTEGER PRIMARY KEY,
    subject_entity_id INTEGER NOT NULL REFERENCES entity(id),
    outcome           TEXT    NOT NULL REFERENCES replay_outcome(name),

    -- The environment is IN the digest, not beside it. A replay that
    -- reproduces the output but not the environment has not reproduced the
    -- run. Shape lifted from Icarus's planApprovalDigest, which already
    -- digests provider + checks + sandbox + ceiling + baseCommit.
    environment_digest      TEXT,
    environment_digest_algo TEXT REFERENCES digest_algorithm(name),

    observed_at          TEXT,
    observed_encoding    TEXT REFERENCES instant_encoding(name),

    -- Inputs are pinned by digest, never by name. A replay bound to a moving
    -- name fires on every unrelated change and teaches the reader to ignore
    -- it -- Janus's own reasoning about drift, applied to replay.
    inputs_digest        TEXT,
    inputs_digest_algo   TEXT REFERENCES digest_algorithm(name),

    CHECK (outcome <> 'not_attempted' OR observed_at IS NULL),
    CHECK (outcome =  'not_attempted' OR observed_at IS NOT NULL),
    CHECK (observed_at IS NULL OR observed_encoding IS NOT NULL),
    -- A run that actually executed must say what it ran against.
    CHECK (outcome NOT IN ('reproduced','diverged')
           OR (environment_digest IS NOT NULL AND inputs_digest IS NOT NULL)),
    CHECK (environment_digest IS NULL OR environment_digest_algo IS NOT NULL),
    CHECK (inputs_digest      IS NULL OR inputs_digest_algo      IS NOT NULL)
) STRICT;
