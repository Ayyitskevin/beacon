---
status: accepted
---

# Artifact v1: a general content identity beside the authority ledger

Beacon has immutable, digest-identified records for intent (Proposal, ADR 0034), decision (Ruling,
0037), judgment (Evaluation, 0038), and byte selection (Candidate, 0039), and byte-identified
Receipts that are deliberately not digest-identified (0027, 0031, 0039). The Blueprint's remaining
unbuilt noun is the one those records are all *about*: the Artifact, "a versioned work product with
content identity and provenance" (`CONTEXT.md:194-197`). `git grep -n -i 'struct
Artifact\|artifact_id' -- crates` on the parent of this change returns nothing.

The first consumer is the Outcome Document (ADR 0040): its candidate Markdown is a durable,
content-addressed Artifact evaluated by a non-author before publication. The entity is designed
against every Blueprint artifact mention — the 26 are enumerated in the design
(`A-kimi-artifact-entity-design.md` §1) — not only that one.

This ADR records the grammar and identity exactly, at the level a second implementation needs. The
design rationale, the beside-not-generalizes analysis, and the Steward's rulings (FABLE-RULINGS R15
and R16, 2026-09-04) live in the design and its two non-author reads; this text states what is
built, not why every alternative lost.

## Decision

Beacon introduces `ArtifactV1`, canonicalizer `beacon.artifact.v1`. The identity is SHA-256 over the
record's canonical bytes, and those bytes cover **authorship, scope, kind, and the content digest
envelope** — not the bare content digest. Two Residents recording byte-identical content produce two
Artifacts with two identities, because criterion 26 (`docs/BLUEPRINT.md:898`, `:1163`) adjudicates
author versus evaluator and the author lives in the record. The content digest alone is stored as a
field; it is never the identity.

An Artifact is a statement of record, not an authority surface: recording one opens no Gate, admits
no dispatch, completes no Mission, publishes no Outcome, and grants no capability. Whether an
Artifact is *Material* (`CONTEXT.md:199-201`) is a fact about its consumers, never a flag on the
record, and no accessor answers a boolean.

### The canonical bytes

One canonical style in this crate, per ADR 0034/0037/0038/0039/0040: positional fixed-order fields;
integers unsigned big-endian at the stated width; strings valid UTF-8 preceded by a `u64` byte
length; UUIDs 16 raw bytes; SHA-256 values 32 raw bytes; closed one-byte tags, never renumbered. The
decoder consumes the entire input and refuses unknown tags, trailing bytes, over-limit lengths,
invalid UTF-8, and non-canonical forms; no declared length is used to reserve capacity — a length is
proved against its field's limit and the remaining input before any of it is read. `0x00` is
assigned only where absence is a value (`context`, `supersedes`) and is refused everywhere else, so
a zero-filled buffer cannot decode as any valid record.

| # | field | encoding | rule |
|---|---|---|---|
| 1 | magic | 19 literal bytes `beacon.artifact.v1\0` | fixed; not length-framed |
| 2 | `workspace_id` | uuid | scope root; every relation is composite over it |
| 3 | `kind` | tag | `0x01` = `outcome_document`; the only assigned value in v1 |
| 4 | `media_type` | string | must equal the kind's one permitted token (below) |
| 5 | `content_digest.algorithm` | string | `sha256` — the only assigned value |
| 6 | `content_digest.canonicalizer` | string | `beacon.raw.v1` — the only assigned value |
| 7 | `content_digest.value` | 32 raw bytes | SHA-256 over the exact stored content bytes |
| 8 | `content_bytes` | u64 | ≤ 1,048,576 (the v1 content ceiling, below) |
| 9 | `accountable.resident_id` | uuid | the Resident accountable for the content |
| 10 | `accountable.resident_configuration_id` | uuid | that Resident's configuration at the named Mission's admission |
| 11 | `context` | tag | `0x00` = `none`; `0x01` = `mission` + `project_id` + `mission_id` |
| 12 | `supersedes` | tag | `0x00` = `none`; `0x01` + 32 raw bytes of the predecessor's record digest |
| 13 | `recorded_at_ms` | u64 | service clock at record time, never caller input |

Field rules the table cannot carry:

- **Kind and media type are one relation, keyed on the kind.** v1 assigns exactly one pair:
  `outcome_document` → `text/markdown; charset=utf-8`. The codec refuses any other pairing; a caller
  never supplies a media type the kind does not own. New kinds are assigned by amendment, never
  renumbered, and only when a consumer exists (R16/A4 — assigning shapes ahead of their owners is
  ADR 0034's undeveloped-constraint failure mode).
- **`beacon.raw.v1`** means "SHA-256 over the exact stored bytes, no transformation" (R16/A6). A
  future canonicalized content form is a new token, never a reinterpretation. The digest value is
  not embedded inside the content it names.
- **The accountable pair** is validated at record time against the Workspace: the Resident exists,
  and — when `context` is `mission` — the Resident is an admitted member of that Mission under
  exactly the named Configuration. Without that check the roster a future Evaluation arm's
  non-authorship proof rests on would be self-chosen (D-opus2-review-eval-arm-design.md, E3). A
  Resident's lifecycle later moving does not corrupt the record: accountability is a record-time
  fact, and `BLUEPRINT:343` requires a retired Specialist's Artifacts to stay attributable.
- **`context`** exists because accountability without a Mission has no admission to name:
  `mission_admission_members` pins the Resident's Configuration at *a Mission's* admission, so
  field 10 has no parent without one (R16/E1). `none` is a real value: a Workspace-scoped Artifact
  is representable, and it is not evaluable in v1 — there is no roster to check an evaluator
  against, which is a consequence, not a policy choice.
- **`supersedes`** names the predecessor's record digest. Corrections are new Artifacts, never
  edits: store triggers will refuse update and delete, and the chain rules (cite the current tail,
  one predecessor each, superseded at most once, chain order never timestamp order) are ADR
  0037/0038's, arriving with the supersession relation in slice 4.
- **The record's own total bound is 4,096 bytes** (R16/A7's surviving number). The field table
  admits at most ~260, so the envelope is defensive; it is stated anyway, because a grammar whose
  per-field limits admit a size the store refuses turns a fact about the bytes into a database
  error surfacing through a Command (ADR 0038's reason for its own bound).
- **The same-width caveat, as ADR 0037/0038 state it.** This grammar is positional and untagged. A
  reordering the decoder can see is refused; a transposition of two same-width values is not
  detectable by construction — what holds is that every byte is covered by the digest, so a
  transposed record is a different Artifact, never a mistaken reading of this one. Field order is
  fixed by the table and pinned by the published vector below; it is not runtime-enforced.

### Published identity vector

The fixture: Workspace `00000000-0000-4000-8000-000000000001`, Project `…0002`, Mission `…0003`,
accountable Resident `…0004` at Configuration `…0005`; kind `outcome_document`; media type
`text/markdown; charset=utf-8`; content digest value `66`×32; `content_bytes` 11,638; `context =
mission`; `supersedes = none`; `recorded_at_ms` 1,788,000,000,000.

**221 bytes**, SHA-256 `83e3056b4cc665a458d279fa8602f9b137f87cc8032b5a5037afab614ed4eff0`:

```text
626561636f6e2e61727469666163742e76310000000000000040008000000000000001010000
00000000001c746578742f6d61726b646f776e3b20636861727365743d7574662d3800000000
00000006736861323536000000000000000d626561636f6e2e7261772e763166666666666666
666666666666666666666666666666666666666666666666660000000000002d760000000000
0040008000000000000004000000000000400080000000000000050100000000000040008000
0000000000020000000000004000800000000000000300000001a04d1ad800
```

The bytes were produced independently, in Python, from the field table alone, importing no Beacon
code; the Rust test assembles the same bytes a second time by hand and asserts the encoder agrees
with both. A reader reproduces the identity with
`printf '%s' '<hex, joined>' | xxd -r -p | sha256sum`.

### v1 is inline-only

Content up to the 1 MiB ceiling is stored beside the record (slice 2). The maximum v1 Artifact is
exactly the Outcome's emitted-Markdown bound (ADR 0040), so the content-addressed blob directory —
descriptor-pinned writes, staging and rename, adopt-or-fail, verified single-descriptor reads,
orphan diagnostics — is **designed and deliberately not built** (R16/A7, A9): it arrives with the
first kind that needs it, and its fail-closed startup blast radius is the Steward's question then,
not now. A consequence stated rather than discovered: there is no dedup in v1; two identical
documents are two rows, each carrying a full copy.

### The v1 fence

Slice 1 (this ADR's evidence below) is the codec and the identity: types, encoder, decoder,
refusals, the published vector. The store, the `artifact.recorded` Event, the derived creation event
cursor (exposed by the verified reader), the supersession relation, and the verified reader itself
are slices 2-4. There is no transport arm, no desktop surface, and no agent path: the first caller
is the Outcome module's `prepare`, crate-internal (R16/A10). Migrations and the supersession
relation take their numbers after rebasing on main.

Seams, named and not built: the Evaluation Artifact subject arm (`0x03`, reserved in ADR 0038's
field-2 row by its pending amendment) binds this record's digest; MissionCompletionV1's
delivered-Artifact reference is this record's digest envelope, copied whole; the Outcome manifest
cites Artifacts by `SourceRef`.

## Required evidence before this grammar lands

- the published vector is reproduced by a hand-built assembly in the test file and by the
  independent Python construction above, and the digest matches;
- every identity-bearing field changes the digest;
- one changed byte, a trailing byte, an unknown tag, a zero-filled buffer, and an over-envelope
  input are each refused, distinctly;
- a media type the kind does not own is refused by the codec, in both directions;
- an unassigned kind tag is refused;
- a declared length that cannot fit the remaining input is refused by arithmetic, before any
  allocation;
- decode and re-encode are byte-identical, and the decoder consumes the entire input;
- `context = none` and `supersedes = none` encode as values, and the decoder refuses a missing
  field in their place.

Each bullet maps to one named test in `crates/beacond/tests/artifact.rs`. The mutation gate is
`beacon-team/K3-mutants.sh`: accept a second media token for `outcome_document`; skip the
re-encode comparison; trust a declared length for capacity; accept an unassigned kind. Each must
turn a named test red.

## Consequences

Beacon gains the general Artifact identity the Blueprint's kernel names (`docs/BLUEPRINT.md:658`),
beside the authority ledger's records and not instead of them (R16/A1: generalizing would
retroactively digest Receipts against two accepted rulings and fork every pinned identity vector).
Nothing can be recorded, read, evaluated, completed against, or published from an Artifact until the
later slices land; slice 1 proves only that the identity is reproducible and the grammar is closed.
A stated gap: the record carries no human-readable title — the Outcome's `VerifiedArtifact.title`
has no source until one is assigned, which is a canonicalizer change by the rule above and is the
Steward's to schedule.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.
