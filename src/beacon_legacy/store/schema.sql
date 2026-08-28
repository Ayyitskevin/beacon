-- Beacon legacy desk unified store — envelope + authority core.
--
-- OWNERSHIP. Lane A (claude-09) owns the tables and columns below: the shape.
-- Lane B (claude-0d) attaches capability sub-models and typed bodies.
-- Lane C (claude-2f) owns INTEGRITY: additional CHECKs, triggers, the
-- exactly-one-binding-variant constraint, and the migration readers.
-- Each invariant is enforced in exactly ONE layer; the other layer's tests
-- assert it is enforced there rather than re-enforcing it. Two layers both
-- enforcing is how they drift.
--
-- STACK. SQLite (Tauri's store), but deliberately plain: no ORM, no
-- SQLite-only types, no Tauri assumptions. A stack pivot should cost the
-- dialect and nothing above it.
--
-- THE RULE THAT SHAPES ALL OF THIS. An absence with no place to be recorded
-- makes distinct absences indistinguishable. So:
--   * every meaningful column is NOT NULL
--   * there are NO DEFAULTS on meaningful columns -- a default is a value
--     nobody chose, and a defaulted boolean is a two-state nullable with
--     better manners (it is why five superseded gates read as live)
--   * "missing" is an explicit enumerated value, never NULL
--   * '' is used as an explicit non-value where a column is structurally
--     present but has no content, and its legality is constrained per column
--
-- NULL appears in exactly one place in this file, and it is load-bearing:
-- `capability_registry.exists_since` -- see the comment there.

PRAGMA foreign_keys = ON;


-- ──────────────────────────────────────────────────────── enum lookups ──
-- Every enum's legal values live in ONE canonical place: the Python
-- definition in `beacon_legacy.store.enums.CANONICAL_ENUMS`, materialized here by
-- `seed_enums()`. These tables are deliberately EMPTY in the DDL.
--
-- Inline `CHECK (col IN ('a','b'))` was the alternative and it is a second
-- hand-maintained copy of the same list. It drifts in the direction nothing
-- detects: the schema goes on accepting a value the code has dropped, and a
-- stale row validates forever. Lane C's reconciliation test compares both
-- directions against the canonical definition.

CREATE TABLE enum_capability        (value TEXT PRIMARY KEY);
CREATE TABLE enum_origin            (value TEXT PRIMARY KEY);
CREATE TABLE enum_resolution        (value TEXT PRIMARY KEY);
CREATE TABLE enum_relation_kind     (value TEXT PRIMARY KEY);
CREATE TABLE enum_decider           (value TEXT PRIMARY KEY);
CREATE TABLE enum_check_result      (value TEXT PRIMARY KEY);
CREATE TABLE enum_enforcement       (value TEXT PRIMARY KEY);
CREATE TABLE enum_lifecycle         (value TEXT PRIMARY KEY);
CREATE TABLE enum_force             (value TEXT PRIMARY KEY);
CREATE TABLE enum_binding_kind      (value TEXT PRIMARY KEY);
CREATE TABLE enum_capability_state  (value TEXT PRIMARY KEY);
CREATE TABLE enum_authority_state   (value TEXT PRIMARY KEY);
CREATE TABLE enum_type_nature       (value TEXT PRIMARY KEY);
CREATE TABLE enum_status_polarity   (value TEXT PRIMARY KEY);

-- ─────────────────────────────────────────────────────────── vocabularies ──
-- Per-(capability, kind) closed vocabularies. Stored rather than code-only so
-- a foreign key can enforce membership: SQLite cannot express a subquery
-- CHECK, but it can express an FK, which is the same guarantee.

CREATE TABLE vocabulary (
    capability        TEXT NOT NULL,
    kind              TEXT NOT NULL,
    -- 'lifecycle' | 'event'. An event type declares NO status members at all,
    -- rather than a one-member vocabulary: a column whose value is always the
    -- same carries no information while implying there is something to check,
    -- and invites a lifecycle being smuggled in later.
    nature            TEXT NOT NULL REFERENCES enum_type_nature (value),
    PRIMARY KEY (capability, kind)
);

CREATE TABLE vocabulary_member (
    capability        TEXT NOT NULL,
    kind              TEXT NOT NULL,
    member            TEXT NOT NULL,
    -- What this member MEANS. Was `is_terminal_negative INTEGER`, which was
    -- the `spent: bool` defect inside the column built to prevent collapses:
    -- `= 0` returned open, in_progress, done AND verified, grouping "has not
    -- finished" with "finished successfully". A boolean cannot hold those
    -- apart, and they are the two a caller most needs separated.
    --
    -- Four values, and the fourth earns its place by operator action:
    -- terminal_negative (someone decided against it) vs terminal_unruled
    -- (expired/withdrawn/superseded -- nobody ruled at all). Janus's own
    -- supersede prints "NOBODY RULED". A refusal is settled; something that
    -- lapsed may still matter and can be re-raised.
    polarity          TEXT NOT NULL REFERENCES enum_status_polarity (value),
    PRIMARY KEY (capability, kind, member),
    FOREIGN KEY (capability, kind) REFERENCES vocabulary (capability, kind)
);

-- ────────────────────────────────────────────────── capability registry ──
-- Level 4 of the absence rule: a registry derived from what happens to be
-- implemented cannot report what is missing. ReplayDeck was ranked first in
-- the portfolio, named in the product trace, never built -- and every handoff
-- said "six capabilities" for a day, because a capability that does not exist
-- has nowhere to be absent from.

CREATE TABLE capability_registry (
    capability        TEXT PRIMARY KEY,
    -- 'built' | 'not_built' | 'deferred' | 'retired'. Never inferred from
    -- whether rows happen to exist.
    state             TEXT NOT NULL REFERENCES enum_capability_state (value),
    -- REQUIRED, and the reason this table earns its place. 'never built' and
    -- 'deferred because there is no run state on this host' are both "no
    -- implementation" and are different facts; reporting the first as the
    -- second sends someone to go restart a service that was never written.
    reason            TEXT NOT NULL CHECK (reason <> ''),
    -- The ONE nullable in this schema, and it is the honest shape: a
    -- capability that does not exist has no date on which it started to.
    -- NULL here means "not applicable", which is the column's only possible
    -- reading -- it is not standing in for four different absences.
    exists_since      TEXT
);

-- ──────────────────────────────────────────────────────────── references ──
-- A reference is (capability, ref, origin), never a bare string: Athena keys
-- (MWS-2), Buzz event ids (64 hex) and Janus gate ids (g3410c2cff0f) are all
-- "an id" and none is unique across the set.
--
-- `origin` is a third part rather than folded into `capability` because the
-- same capability holds records from more than one origin, indefinitely:
-- MWS-2 migrated from old Athena and MWS-2 created natively afterwards are
-- different records with the same ref.

CREATE TABLE reference (
    id                TEXT PRIMARY KEY,
    capability        TEXT NOT NULL REFERENCES enum_capability (value),
    ref               TEXT NOT NULL CHECK (ref <> ''),
    origin            TEXT NOT NULL REFERENCES enum_origin (value),
    -- Four-valued. 'absent' is the COMMON, HEALTHY state: we know what it
    -- names and do not hold it. A foreign key that refused an un-imported
    -- reference would delete the "2 of 7 trace hops joinable" measurement;
    -- a nullable that swallowed it would hide it.
    resolution        TEXT NOT NULL REFERENCES enum_resolution (value),
    -- Recorded ALONGSIDE the triple, never in place of it. Resolution is not
    -- destructive: replacing the triple with a store id feels like
    -- normalization and destroys the ability to re-check AND any way to
    -- detect the resolution was wrong. A wrong resolution that has eaten its
    -- own input is undetectable forever. Same rule as: a re-evaluated verdict
    -- is never persisted as a stored fact.
    resolved_to       TEXT NOT NULL,
    CHECK ((resolution = 'resolved') = (resolved_to <> '')),
    UNIQUE (capability, ref, origin)
);

-- ──────────────────────────────────────────────────────────────── records ──

CREATE TABLE record (
    -- Store-assigned and opaque. Natural keys live in provenance_source_ref:
    -- a migration importing the same gate twice, or two capabilities whose
    -- ref formats overlap, must not collide -- and a record's identity must
    -- not change if its source system is later corrected.
    id                TEXT PRIMARY KEY,
    capability        TEXT NOT NULL,
    -- The capability's own word, scoped by capability. Never a shared
    -- taxonomy: an enum every capability must squeeze into is the most
    -- expensive mistake available at this layer.
    kind              TEXT NOT NULL CHECK (kind <> ''),
    title             TEXT NOT NULL CHECK (title <> ''),
    -- A member of this (capability, kind)'s closed vocabulary, or
    -- 'unmapped_legacy'. '' ONLY for event-natured types, which declare no
    -- status at all. Lane C enforces the event/'' correspondence.
    status            TEXT NOT NULL,

    -- Two clocks, never one. A migration gives every imported record the same
    -- recorded_at and wildly different occurred_at; collapsing them makes
    -- imported and native records indistinguishable while ordering silently
    -- means different things for different rows.
    occurred_at       TEXT NOT NULL CHECK (occurred_at <> ''),
    recorded_at       TEXT NOT NULL CHECK (recorded_at <> ''),

    -- Provenance: how this fact came to exist. Inline because it is 1:1 and
    -- always present. Under federation this answered "which endpoint did we
    -- read"; that question is gone and this one matters more.
    provenance_origin      TEXT NOT NULL,
    provenance_source_ref  TEXT NOT NULL,  -- '' when there is none
    provenance_source_rev  TEXT NOT NULL,  -- '' when there is none
    provenance_actor       TEXT NOT NULL CHECK (provenance_actor <> ''),
    -- Set for migrated origins, '' otherwise. Imported and native records
    -- coexist INDEFINITELY -- capabilities retire on their own schedules --
    -- so this is permanent, not transitional, and must never be inferable
    -- only from a timestamp.
    provenance_imported_at TEXT NOT NULL,

    -- The typed structure for this (capability, kind), serialized. Typed
    -- rather than free-form because an untyped column is the nullable problem
    -- at larger scale: every key is a nullable field and none can be
    -- constrained. Lane B owns the per-type schemas.
    body              TEXT NOT NULL,
    -- Content we did not model, verbatim, with its reason. An explicit state,
    -- never a silent omission and never a coercion to the nearest thing we
    -- did model. A migration that quietly drops an unmodelled field is
    -- permanent data loss with no second look coming. '' when nothing was
    -- unmapped.
    body_unmapped     TEXT NOT NULL,

    -- 'required' | 'not_required'. Whether this record carries a decision at
    -- all. Explicit because "no approval was ever sought" and "an approval is
    -- missing" are different facts, and a nullable FK would make them one.
    authority_state   TEXT NOT NULL REFERENCES enum_authority_state (value),
    authority_id      TEXT NOT NULL,  -- '' when not_required or not yet recorded
    CHECK (authority_state = 'required' OR authority_id = ''),

    FOREIGN KEY (capability) REFERENCES capability_registry (capability),
    FOREIGN KEY (capability, kind) REFERENCES vocabulary (capability, kind)
);

-- ────────────────────────────────────────────────────────────── relations ──
-- Three meanings that were one field. Structural belonging, access scope and
-- reply-threading were all converging on a single parent reference, and any
-- two of them conflated is correct-looking until someone's access follows a
-- structural move.

CREATE TABLE relation (
    from_record       TEXT NOT NULL REFERENCES record (id),
    to_reference      TEXT NOT NULL REFERENCES reference (id),
    kind              TEXT NOT NULL REFERENCES enum_relation_kind (value),
    PRIMARY KEY (from_record, to_reference, kind)
);

-- Structural belonging is singular; the other kinds are not. Scope is
-- deliberately NOT constrained to one, and is deliberately not derived from
-- containment: a structural move must never silently change who can see a
-- record.
CREATE UNIQUE INDEX relation_one_container
    ON relation (from_record) WHERE kind = 'contains';

-- ────────────────────────────────────────────────────────────── authority ──
-- The seven mechanisms as one type with required axes -- not one shape
-- replacing four. They were correct answers to different questions.

CREATE TABLE authority (
    id                TEXT PRIMARY KEY,
    -- Who decided. Not how important the mechanism is.
    decider           TEXT NOT NULL REFERENCES enum_decider (value),
    -- What re-deriving produced THIS RUN. Split from the binding because they
    -- have different lifetimes: a binding is fixed at decision time; a check
    -- result changes every time you look.
    check_result      TEXT NOT NULL REFERENCES enum_check_result (value),
    enforcement       TEXT NOT NULL REFERENCES enum_enforcement (value),
    -- Where a decision GOT TO.
    lifecycle         TEXT NOT NULL REFERENCES enum_lifecycle (value),
    -- Whether it STILL APPLIES. A different question from lifecycle, and
    -- neither substitutes for the other: a refused decision reached a terminal
    -- lifecycle and is not in force; an authorized-then-superseded one reached
    -- a favourable lifecycle and is also not in force. Collapsing them
    -- reproduces "a decision record exists" read as "a decision was
    -- favourable". NO DEFAULT -- its predecessor was `spent BOOLEAN DEFAULT 0`.
    force             TEXT NOT NULL REFERENCES enum_force (value),
    -- Discriminator for the binding variant. Exactly one row must exist in
    -- exactly one binding_* table. Lane C enforces the exactly-one.
    binding_kind      TEXT NOT NULL REFERENCES enum_binding_kind (value),
    -- The mechanism's own name: janus.ruling, icarus.plan_approval,
    -- buzz.kind.46030. Buzz kind numbers are recorded literally because Buzz
    -- is upstream OSS and can renumber; a change must surface as a change.
    mechanism         TEXT NOT NULL CHECK (mechanism <> ''),
    -- What this decision means, in the deciding capability's own words.
    meaning           TEXT NOT NULL CHECK (meaning <> '')
);

-- Binding variants as separate tables rather than one table with nullable
-- columns. An intent binding has no digest, and "digest is absent" must not be
-- representable as a NULL that could equally mean "not recorded" or "lost in
-- transport". This is the tagged union the no-meaningful-NULL rule requires.

CREATE TABLE binding_digest (
    authority_id      TEXT PRIMARY KEY REFERENCES authority (id),
    digest            TEXT NOT NULL CHECK (digest <> ''),
    -- REQUIRED beside every digest. Icarus's stableJson is JS-specific: an
    -- integral float serializes 5 in Node and 5.0 in Python, and non-BMP key
    -- ordering differs. Re-deriving a migrated digest with a different
    -- canonicaliser does not look like a tooling change -- it looks like
    -- DRIFT, the one state that demands a human act now, fired across every
    -- migrated approval at once. Recording the algorithm keeps the
    -- vendored-frozen canonicaliser option open.
    algorithm         TEXT NOT NULL CHECK (algorithm <> ''),
    -- What the digest is over, named. Icarus digests plan + provider + checks
    -- + sandbox + ceiling + base commit + manifest; Janus digests one
    -- artifact. Both are "a digest" and they promise different things.
    covers            TEXT NOT NULL CHECK (covers <> '')
);

CREATE TABLE binding_intent (
    authority_id      TEXT PRIMARY KEY REFERENCES authority (id),
    action            TEXT NOT NULL CHECK (action <> ''),
    target_reference  TEXT NOT NULL REFERENCES reference (id)
);

CREATE TABLE binding_handle (
    authority_id      TEXT PRIMARY KEY REFERENCES authority (id),
    handle            TEXT NOT NULL CHECK (handle <> ''),
    expires_at        TEXT NOT NULL CHECK (expires_at <> ''),
    approver_spec     TEXT NOT NULL CHECK (approver_spec <> '')
);

CREATE TABLE binding_step (
    authority_id      TEXT PRIMARY KEY REFERENCES authority (id),
    run_reference     TEXT NOT NULL REFERENCES reference (id),
    step_id           TEXT NOT NULL CHECK (step_id <> ''),
    step_index        INTEGER NOT NULL
);

CREATE TABLE binding_none (
    authority_id      TEXT PRIMARY KEY REFERENCES authority (id),
    -- Why nothing was bound. An unexplained absence is indistinguishable from
    -- an oversight. Measured 2026-08-25: 4 of 15 live Janus gates are bound to
    -- nothing and 3 of those are approved, in the same list as 9 git-bound
    -- rulings.
    reason            TEXT NOT NULL CHECK (reason <> '')
);
