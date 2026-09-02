# Phase 2 Proposal v1

This checkpoint gives Beacon a stable Proposal identity: an immutable description of intended
effects, inputs, bounds, checks, and budget, identified by the SHA-256 of the exact bytes an Owner
submitted. It implements [ADR 0034](../adr/0034-immutable-proposal-v1-authority-bytes.md) within
that decision's fence.

It creates no authority. Submission records an Operator-attributed, Resident-requested description
for review. It does not create a Gate or Ruling, consult an execution capability, prepare a
worktree, start a process, invoke a model, make a network request, populate a Receipt, or dispatch
anything. No Proposal command crosses the production transport, and no desktop surface exists. The
Proposal is inert.

It is numbered Phase 2 because it is the first slice past the Phase 1 Repository Binding that a
future Ruling, Evaluation, or Run can bind to.

## One interface, and the bytes are the record

```text
submit(canonical_bytes) -> Proposal
get(digest) -> Proposal
list_for_mission(mission_id) -> [Proposal]
```

`submit` authenticates the Owner before it looks at the bytes, so an unauthenticated caller receives
the account refusal and never a parser verdict. It then hashes the supplied slice, strictly decodes
that same slice, re-encodes the decoded value, and requires byte-for-byte equality. There is no
public function that accepts a parsed value and a digest as independent arguments; permitting those
two to disagree is the alternative ADR 0034 rejects.

`Proposal` keeps private fields and derives no `Deserialize`, unlike every comparable view in this
crate. The departure is deliberate and its reason is the ADR's own rejected alternative: "**Caller-
supplied parsed object plus digest:** permits the values to disagree." A public-field struct could
be assembled by hand with bytes and payload that do not match; this one can only come from the
constructor that proved they do.

## The canonical bytes

The canonicalizer identity is `beacon.proposal.v1`. The grammar is binary, fixed-order, and
self-delimiting: a 19-byte ASCII magic `beacon.proposal.v1\0`, UUIDs as 16 raw bytes, SHA-256 values
as 32 raw bytes, unsigned big-endian integers at each field's fixed width, one-byte enum tags that
are never renumbered, strings as UTF-8 behind a 64-bit big-endian byte length, and lists as a 32-bit
big-endian count followed by entries in their specified order.

No declared length or count is ever used to reserve capacity. The decoder proves each one fits
within the remaining input and the field's own limit, then appends entries as it decodes them. It
consumes the entire input and refuses unknown tags, trailing bytes, over-limit lengths, invalid
UTF-8, and non-canonical ordering. `submit` refuses a slice larger than 64 KiB before parsing.

Optionality is always a closed one-byte variant. `readable_context` is `not_declared` or
`declared(entries, manifest_sha256)`, and those two states produce different bytes and different
identities — an empty declaration is the positive assertion that no Repository file is readable, not
the absence of a declaration. Each undeveloped constraint family is `not_declared` or
`bound(kind, version, canonicalizer, sha256)`; the `bound` tag is defined so a later reviewed
decision needs no new tag, and submission refuses it because no owning module can resolve one yet.

Paths are neither Unicode-normalized nor case-folded, and human text is encoded exactly as
submitted. Unsorted or duplicate input is refused rather than silently reordered: changing the
submitted meaning before binding it is the thing the digest exists to prevent.

This follows the repository's existing length-framed binary digest precedent in
`crates/beacond/src/sample_project.rs` and adds no dependency — `data-encoding` and `sha2` were
already `beacond` dependencies, so RFC 4648 padded base64 and SHA-256 cost nothing new.

### Where ADR 0034 was silent

Seven interpretations were made and ruled on before implementation, rather than resolved silently:

1. **Check-run coverage and empty lists.** Every `declared_check_runs` name must resolve in
   `declared_checks`; the converse is not required, and all four intent lists may be empty. The
   accepted text states one direction only, and a bijection would be an invented constraint.
2. **Mutation order.** `declared_mutations` is the caller's intended sequence, identity-bearing and
   duplicate-free on the exact `(kind, target)` pair. Every other list in the ADR is given an
   explicit canonical order; this one is called an intended sequence instead.
3. **`pinned_base_revision`** is encoded as its 40-character lowercase hexadecimal string, not
   twenty raw bytes. Raw bytes would re-encode `A1B2…` and `a1b2…` to one identity, and case-folding
   a Binding is exactly the silent normalization this grammar refuses.
4. **Bounds** are six `u64` values, matching the existing bounds vocabulary in
   `crates/beacond/src/sandbox/bounded.rs`, and zero is a legal bound. No positivity rule was
   invented.
5. **Budget and rollback** have one legal arm each in v1, so no two accepted Proposals can differ in
   them. Their bytes are proved load-bearing instead: changing the tag changes the identity and the
   record is refused as an unknown tag rather than read as the value it used to hold.
6. **Referent checks** use one narrow crate-private reader per owning module — `mission.rs`,
   `project_policy.rs`, `operator_account.rs` — rather than SQL from `proposal.rs` into another
   module's tables.
7. **Readiness** ships as a list of unmet preconditions with no boolean accessor. In v1 all seven
   constraint families can only be `not_declared`, so every v1 Proposal is unready by construction;
   that fact is stated rather than implied.

### The recorded fixture

`crates/beacond/src/proposal.rs` pins one fabricated Proposal by its bytes and by its digest, as two
separate claims. The 690-byte record is:

```text
626561636f6e2e70726f706f73616c2e7631001111111111111111111111111111111122222222222222222222222222
222222333333333333333333333333333333334444444444444444444444444444444455555555555555555555555555
55555501000000000000003e546865204f776e65722073656c65637465642074686973205265736964656e7420746f20
726571756573742074686973206f6e652050726f706f73616c2e00000001000000010000000100000000000000286131
623263336434653566366131623263336434653566366131623263336434653566366131623200000000000000304164
6420612072656772657373696f6e207465737420666f722074686520696e76617269616e7420636865636b65722e0200
000002000000000000000a7372632f6c69622e7273666666666666666666666666666666666666666666666666666666
6666666666000000000000001374657374732f696e76617269616e74732e727377777777777777777777777777777777
7777777777777777777777777777777772e7780cda9baa6a2c6ab87af538f7b629937ba6f4c31cb94c3f2aad8cd67a10
00000002000000000000000a7372632f6c69622e7273000000000000001374657374732f696e76617269616e74732e72
730000000202000000000000000a7372632f6c69622e727301000000000000001374657374732f696e76617269616e74
732e727300000001000000000000000a636172676f2d7465737400000001000000000000000a636172676f2d74657374
000000030000000000000005636172676f000000000000000474657374000000000000000b2d2d776f726b7370616365
00000000000000012e0101010101010100000000000493e000000000000000f000000000800000000000000010000000
000000000010000000000000000000000101
```

Its identity is:

```text
a40cb728defbb93cdad8c9c81286f31a8bca9287b92ec1baa5f260016f7ae404
```

Reproduce it with no Beacon code at all:

```sh
printf '%s' '<the hex above, on one line>' | xxd -r -p | sha256sum
```

`sha256sum` and `openssl dgst -sha256` were both run against those bytes and agree. The test asserts
that the encoder produces exactly this record, and separately that this digest is the digest of this
record, so an encoder that drifted fails the first claim and a digest taken over the wrong preimage
fails the second.

## Persistence and reconciliation

Schema version 18 adds the immutable `proposals` table. One row stores the canonical bytes, the
digest algorithm, canonicalizer, and value, the scope and binding columns, the requesting Resident
and Configuration, the bootstrap basis and reason, and the original submission time. Every state
discriminator is `NOT NULL`; meaningful absence is an explicit parsed variant inside the bytes,
never SQL null. Update and delete triggers refuse to change a recorded Proposal.

Migration 0018 also adds four unique indexes so composite foreign keys can state in the schema what
the ADR states in prose: the Mission belongs to the named Project, the requesting Resident is an
admitted member at that exact admission Configuration, and the Policy and Repository revisions bind
the named Charter revision and pinned base revision. The migration follows the existing closed-Event
rebuild pattern, preserves every v17 Event sequence, payload byte, and timestamp, adds only
`proposal.submitted`, creates empty Proposal state, and advances the version only from 17.

An authenticated submission is one immediate transaction: hash, decode, re-encode, and compare;
validate referent existence and Workspace/Project scope consistency without comparing any binding to
the current revision; insert the row; insert exactly one `proposal.submitted` Event carrying the
digest envelope, the exact bytes as canonical RFC 4648 padded base64, actor `owner`, and the
submission time. A resubmission of identical bytes returns the original record and writes no second
Event. The same digest with different bytes is a store-integrity failure, not an idempotent match.

Startup recomputes each digest from the stored bytes, strictly decodes and re-encodes them, compares
every indexed column to the parsed value, re-verifies every structural reference, requires exactly
one Event per Proposal whose base64-decoded byte field equals the stored bytes with no orphan Event,
and verifies that the Owner account still exists for every `owner` bootstrap record. Refusal means
`Supervisor::bootstrap` returns an error instead of a `StartupReport`, so no service is constructed
against that store.

Readiness and staleness are derived at read time and never stored. A historical Proposal does not
become corrupt because a current revision moved; it reports the moved binding as an unmet
precondition. Readiness is a diagnostic list, never a boolean, and no interface may present it as
permission to proceed.

## Verification

From the repository root:

```sh
cargo test -p beacond --lib proposal
cargo test -p beacond --test proposal
cargo test -p beacond --lib version_seventeen_store_preserves_event_bytes_and_adds_empty_proposal_state
cargo test -p beacond --lib migration_eighteen_rolls_back_exactly_before_forward_startup
cargo test -p beacond --lib migration_eighteen_advances_the_version_only_from_seventeen
scripts/gate all
```
