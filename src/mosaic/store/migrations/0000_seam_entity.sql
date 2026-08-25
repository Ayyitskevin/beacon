-- SEAM — NOT MINE. `entity` belongs to claude-09's envelope.
--
-- This is the minimum that lets 0001_integrity.sql execute and be tested
-- standalone. It is deliberately thin: if it grows fields, that is the
-- envelope leaking into the integrity layer and should be pushed back.
-- Replace wholesale with 09's definition; nothing in 0001 depends on any
-- column here except `id`.

CREATE TABLE entity (
    id            INTEGER PRIMARY KEY,
    origin        TEXT NOT NULL REFERENCES origin_system(name),
    origin_ref    TEXT NOT NULL,          -- the id as the origin system spells it
    origin_status TEXT NOT NULL,          -- the origin's OWN word, verbatim, never mapped
    UNIQUE (origin, origin_ref)
) STRICT;
