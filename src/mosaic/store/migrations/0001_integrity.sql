-- Mosaic unified store — the INTEGRITY layer, applied on top of schema.sql.
--
-- OWNERSHIP, per schema.sql's own header: Lane A owns the tables and columns
-- (the shape); Lane C owns integrity — additional CHECKs, closed vocabularies
-- over columns Lane A left open, and the concepts Lane A's envelope does not
-- model. Each invariant is enforced in exactly ONE layer.
--
-- An earlier version of this file created `entity`, `entity_reference`,
-- `reference_state` and `origin_system`. Every one of those duplicated a table
-- Lane A had already built (`record`, `reference`, `enum_resolution`,
-- `enum_origin`). They are deleted rather than reconciled: two tables for one
-- concept is the second-copy defect at the schema layer, and reconciling them
-- would have preserved it. Nothing here re-enforces an invariant schema.sql
-- already holds; the tests assert it is enforced THERE.
--
-- Naming follows Lane A's `enum_` convention. Conforming to the existing
-- convention beat imposing a better one.

PRAGMA foreign_keys = ON;

-- ---------------------------------------------------------------------------
-- 1. Instants: the zone travels with the value
-- ---------------------------------------------------------------------------
-- Not modelled by the envelope. Measured: Athena stores naive strings that ARE
-- UTC, but only because SQLite's datetime('now') happens to be. Nothing
-- asserted it, so one naive-local INSERT was a silent 4h error. Three source
-- encodings exist across the six migrating systems.
-- Declared here, SEEDED FROM PYTHON (ownership.seed). Lane A's pattern,
-- adopted: a value list in SQL beside an enum in Python is two copies that
-- drift, and the drift direction nobody notices is the schema accepting a
-- value the code stopped handling.
CREATE TABLE enum_instant_encoding (value TEXT PRIMARY KEY) STRICT;

-- ---------------------------------------------------------------------------
-- 2. Digest algorithms: closing a column Lane A left open
-- ---------------------------------------------------------------------------
-- schema.sql requires `binding_digest.algorithm` to be non-empty but accepts
-- any string. This closes it to a registered set, which is the difference
-- between recording an algorithm and recording a typo. Kevin ruled: vendor
-- Icarus's canonicaliser frozen rather than re-derive migrated digests.
CREATE TABLE enum_digest_algorithm (value TEXT PRIMARY KEY) STRICT;

-- ---------------------------------------------------------------------------
-- 3. Derived values: three kinds, and only one is safe to store
-- ---------------------------------------------------------------------------
-- Measured in Athena's aegis/fleet_work.py::_item(), which computes three
-- values at read time that are not the same kind of thing.
-- `storable` deliberately does NOT live here as a column: clock-derived and
-- cross-table-derived are not storable at all, so derived_value's CHECK admits
-- only row_derived and a per-kind flag would be a second statement of the same
-- rule, free to disagree with it.
CREATE TABLE enum_derivation_kind (value TEXT PRIMARY KEY) STRICT;

CREATE TABLE derived_value (
    id              TEXT    PRIMARY KEY,
    record_id       TEXT    NOT NULL REFERENCES record (id),
    field           TEXT    NOT NULL CHECK (field <> ''),
    kind            TEXT    NOT NULL REFERENCES enum_derivation_kind (value),
    input_fields    TEXT    NOT NULL CHECK (input_fields <> ''),
    -- The other two kinds are not storable at all, so this table can only ever
    -- hold row-derived values. And a derived value whose inputs were not
    -- migrated can be trusted but never recomputed, which is
    -- bytes-unverifiable wearing a different hat.
    CHECK (kind = 'row_derived')
) STRICT;

-- ---------------------------------------------------------------------------
-- 4. Evaluation (ReplayDeck) — GREENFIELD, designed not measured
-- ---------------------------------------------------------------------------
-- ReplayDeck has never been built: the roadmap calls it "the working codename
-- for the existing fleet-eval repo" and that repo is absent from both nodes
-- and GitHub. So there is no migration reader for it and no prior art, and
-- this section is marked so it never reads with the authority of the measured
-- ones.
CREATE TABLE enum_replay_outcome (value TEXT PRIMARY KEY) STRICT;

CREATE TABLE replay_run (
    id                     TEXT PRIMARY KEY,
    subject_record_id      TEXT NOT NULL REFERENCES record (id),
    outcome                TEXT NOT NULL REFERENCES enum_replay_outcome (value),
    -- The environment is IN the digest, not beside it: a replay that reproduces
    -- the output but not the environment has not reproduced the run. Shape
    -- lifted from Icarus's planApprovalDigest.
    environment_digest     TEXT NOT NULL,
    environment_algorithm  TEXT NOT NULL,
    -- Inputs pinned by digest, never by name. A replay bound to a moving name
    -- fires on every unrelated change and teaches the reader to ignore it.
    inputs_digest          TEXT NOT NULL,
    inputs_algorithm       TEXT NOT NULL,
    observed_at            TEXT NOT NULL,
    observed_encoding      TEXT NOT NULL REFERENCES enum_instant_encoding (value),
    CHECK (environment_algorithm <> '' AND inputs_algorithm <> ''),
    -- '' is the explicit non-value, per schema.sql's convention. A run that
    -- actually executed must say what it ran against and when; one that did
    -- not must claim neither.
    CHECK (
        (outcome = 'not_attempted'
            AND environment_digest = '' AND inputs_digest = '' AND observed_at = '')
        OR
        (outcome <> 'not_attempted'
            AND environment_digest <> '' AND inputs_digest <> '' AND observed_at <> '')
    )
) STRICT;
