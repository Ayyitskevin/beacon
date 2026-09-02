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

### The readable-context comparison

ADR 0034 (`docs/adr/0034-immutable-proposal-v1-authority-bytes.md:166-170`) requires a readiness
diagnostic that "independently compares every entry with that revision when the bound Repository is
locally available; otherwise it reports that comparison as not verified", and forbids any surface
from labelling the context "verified from the manifest's self-consistency alone".

This slice accepts `declared(...)` contexts past submission, so that diagnostic is due here. It
ships as `UnmetPrecondition::ReadableContextNotVerified { reason }`, whose closed `ContextComparison`
vocabulary records why the comparison did not happen. Each reason is decided by inspecting the bound
Repository revision the Proposal names — not the current one — using the same object store Check
Preflight reads, under the same inspection budget:

| reason | what was observed |
|---|---|
| `RepositorySourceNotLocal` | the binding names a Git URL; nothing local exists to compare, and this slice performs no fetch |
| `RepositoryUnreadable` | the local path did not open as a Git object store: missing, unreadable, or not a repository |
| `InspectionFailed` | the store opened and an inspection step errored — in practice a pack index that cannot be parsed as version 2 (a `.git` indirection chain is *not* an example: that fails `open`, and is reported as `RepositoryUnreadable`) |
| `InspectionTimedOut` | the inspection did not finish inside its budget: not evaluated, distinct from both a negative answer and an error |
| `PinnedRevisionAbsent` | the store opened, but the pinned base revision is not in it: the basis for the comparison is what is missing |

**The inspection budget is best-effort, not a hard bound.** The comparison consults a 5-second
deadline on entry and between every object-store step, and passes that same deadline into
`ObjectStore::contains`, so an elapsed budget is reported as `InspectionTimedOut` — not evaluated,
never a negative answer. Four things it does not bound, all of them inside
`crates/beacond/src/git_object_store.rs` or the startup loop rather than in this module:

Cited by function rather than by line: these coordinates shift with every edit to the file, and a
residual list that has to be re-measured to be read is not evidence.

- `ObjectStore::open` takes no deadline argument at all, so nothing it does — including reading a
  `.git` file whole with `read_to_string` — is under the budget;
- `index_contains` consults the deadline immediately after its `fs::read` of a pack index, but that
  read cannot be interrupted once begun, so a single very large or slow index still overruns;
- `ObjectStore::contains` likewise checks the deadline after enumerating and collecting the pack
  directory, not during it;
- startup grants each declared-context Proposal its own fresh budget, so N such Proposals cost up
  to N × 5s, not 5s in total.

The shape of the first three is the same: the budget bounds *how much work is started*, never how
long one filesystem call already in flight may take.

Bounding the first three means changing that module's I/O, which this slice does not own. The
fourth would need one budget threaded through startup reconciliation rather than per read. Both are
follow-ups; what is claimed here is only what is enforced.

**The comparison runs, for loose objects.** Resolving `path` to its bytes at a revision means
decompressing the commit object, walking its tree, and reading the blob. When this note was first
written that was a capability limit and `ContentUnreadableAtRevision` was the terminal state:
`beacond` declared no decompression dependency and `git_object_store.rs` was presence-only. Both
changed. The Git-hardening work added `flate2`, and the readable-context slices added
`ObjectStore::read_loose_object`, `ObjectStore::resolve_path_at`, and the comparison that calls them.

A declared entry that holds at the pinned base revision now adds **no readiness item at all**, which
is the outcome the whole area exists for. One that does not is `ReadableContextMismatch`, naming the
entry and whether its digest differs or its path is absent. Reading the entries out of the working
tree at `source` is still refused, for the reason it always was: that compares against whatever is
checked out now rather than against the pinned revision, manufacturing both false agreement and
false disagreement from the wrong evidence.

**Loose objects only, and that is the honest scope.** Every object on the path must be a loose file.
A repository that has been freshly cloned or `git gc`-ed keeps everything in packs, and reports
`PackedObjectUnreadable` for every entry — which may well be the common case. What the slices buy is
that the comparison is true for repositories an Operator has been *working in*, and that
`ReadableContextMismatch` has a real producer. Nobody should read it as "the comparison now runs"
without that qualifier. A pack reader is its own slice: varint entry headers, `OFS_DELTA` and
`REF_DELTA` chains, a recursive resolver with depth and reconstructed-size caps, and an abuse
surface of its own.

**What each outcome is a fact about.** `Agreed` and `Mismatched` are facts about the repository;
every not-verified reason is a fact about what Beacon could reach. The two are never collapsed, and
no failure path produces silence, because silence is the only outcome that reads as agreement.
`PackedObjectUnreadable` in particular is claimed only after an authenticated pack lookup finds the
object — one that an authenticated tree names and that lives in no storage at all is
`InspectionFailed`, because calling that "packed" would excuse a broken store as a Beacon
limitation.

**Mismatches are buffered.** They are published only after every declared entry has completed from a
fully read, self-authenticated object. A mismatch is an accusation about the Operator's declaration
and an unfinished walk has not earned one, so a later packed, oversized, too-deep or malformed entry
discards the buffer and the whole comparison is not-verified.

### The amendment's required evidence, bullet by bullet

The proposed ADR 0034 amendment says this mapping lives here, so here it is. **[u]** =
`crates/beacond/src/git_object_store.rs` or `src/proposal.rs` unit tests, **[i]** =
`crates/beacond/tests/proposal.rs`.

| bullet | test |
|---|---|
| a declared entry that holds adds no readiness item | `a_declared_entry_that_holds_at_the_pinned_revision_adds_no_readiness_item` [i] |
| an entry whose digest differs is a mismatch naming it | `a_declared_entry_that_does_not_hold_at_the_pinned_revision_is_a_mismatch_naming_it` [i] |
| an absent path is a mismatch, not an inspection failure | `a_declared_path_absent_at_the_pinned_revision_is_a_mismatch_not_an_inspection_failure` [i] |
| a packed repository is not-verified, and "packed" is earned by the lookup | `a_packed_repository_is_not_verified_rather_than_mismatched` [i] · `an_object_in_no_storage_at_all_is_an_inspection_failure_not_a_packed_object` [i] |
| a mismatch found before an unreadable entry is discarded | `a_mismatch_found_before_an_unreadable_entry_is_discarded_rather_than_published` [i] · `a_buffered_mismatch_is_discarded_by_every_way_the_walk_can_stop` [u] |
| a symlink is not-verified rather than mismatched | `a_symlink_is_not_followed_and_is_not_reported_as_a_file` [u] · the buffering test above [i] |
| an object that does not hash to its own name is refused | `an_object_that_does_not_hash_to_its_own_name_is_refused` [u] · `a_tampered_tree_on_the_path_is_refused_by_its_own_name` [u] |
| a commit that does not parse as a commit is refused | `a_commit_is_validated_as_a_commit_and_not_merely_authenticated` [u] · `a_commits_headers_must_end_at_a_blank_line_and_not_merely_run_out` [u] |
| a tree naming one path twice is refused | `a_tree_naming_one_path_twice_is_refused_whatever_the_modes` [u], including the nonadjacent case |
| the comparison stops at its deadline rather than answering | `a_deadline_that_expires_mid_read_is_not_published_as_an_outcome` [u] · `a_walk_whose_deadline_expires_mid_traversal_publishes_nothing` [u] · `an_io_failure_after_the_budget_expires_is_elapsed_not_a_broken_repository` [u] |
| every bound the comparison owns is its own | `the_aggregate_budget_is_charged_by_every_object_and_is_its_own_refusal` [u] · `an_exhausted_aggregate_budget_is_the_reason_an_operator_reads` [u] · the per-object rows below |

### The comparison's bounds, and which are proved

The comparison owns its bounds rather than inheriting them, because `read_loose_object` deliberately
invents no limit of its own — a limit chosen inside the reader would be a policy about how large a
legitimate file may be, applied where nothing knows what the file is for.

| bound | owned by | proved by |
|---|---|---|
| one object's compressed and inflated size | the caller, per read | `an_object_larger_than_the_callers_bound_is_a_refusal_not_a_broken_object`, `an_object_that_inflates_past_the_bound_is_a_refusal_not_a_broken_object` |
| path depth | the walk (`MAX_PATH_COMPONENTS`) | `a_malformed_or_over_deep_path_is_refused_before_the_walk_trusts_it`, both sides of the boundary |
| `CONTEXT_OBJECT_BYTES` | the comparison | `a_bound_that_excludes_an_object_on_the_path_is_a_refusal_not_an_absence` |
| `CONTEXT_TOTAL_BYTES` | the comparison, charged by **every** object the walk reads | `the_aggregate_budget_is_charged_by_every_object_and_is_its_own_refusal`, both sides of the boundary; `an_exhausted_aggregate_budget_is_the_reason_an_operator_reads`, for the reason the exhausted state becomes |

**The aggregate bound was neither total nor guarded, and both are fixed.** It charged only the final
blob body, so the commit, every tree and every absent-path traversal were free; and a mutant
replacing the subtraction with a no-op left every test green. Two non-author reviews found the two
halves independently. The budget is now threaded as a parameter — for the reason the deadline is
one, that it is the seam making the outcome reachable without a 64 MiB fixture — every object the
walk reads is charged against it, and exhausting it is `ContextBudgetExhausted` rather than
`ObjectTooLargeToInspect`. Reporting the second as the first told an Operator their six-byte file
was too large, which sends them to shrink the wrong thing.

**The reason an Operator reads is guarded separately from the state the walk produces**, because
those are two facts and only the first is public. A third non-author review remapped the exhausted
state to `ObjectTooLargeToInspect` at the comparison and every proposal test stayed green: the
lower-layer charge was guarded, and the translation above it was not. The integration test that
carried the rule's name never exhausted anything — submission runs the comparison with the constant,
so nothing reachable from there can spend 64 MiB — and it has been renamed for what it does prove.
`an_exhausted_aggregate_budget_is_the_reason_an_operator_reads` drives the comparison itself at a
one-byte allowance, with a full-allowance positive control over the same declaration, so the remap
is RED.

### Object validation, and why authentication is not enough

A Git object's name is the SHA-1 of its uncompressed `<type> <size>\0<body>` form, so an object that
hashes to its own filename cannot have been substituted for another. That is necessary and not
sufficient: it proves the bytes match the name, not that they are the object that name is supposed
to hold. Bytes that authenticate and are not a well-formed object are refused before any result is
drawn from them.

- A **commit** must begin `tree <40 lowercase hex>\n`, and its headers must parse: each
  `<name> SP <value> LF` until the blank line, `parent` values are object names, exactly one
  `author` and one `committer`. Searching the body for a `tree` line instead would let a commit
  *message* choose which tree Beacon walks. The blank line is **required**, and the end of the
  body is not a second terminator: headers that run out instead of ending never reached the
  terminator this grammar names, and `git` writes the blank line even when the message is empty,
  so requiring it refuses nothing a real repository holds.
- A **tree** is parsed whole before any entry is used: a known mode, a non-empty name free of `/`,
  `.` and `..`, a full 20-byte id with no truncated trailing entry, strictly increasing order, and
  no repeated raw name. Ordering uses Git's real rule, where a directory sorts as though its name
  ended in `/` — a naive byte sort refuses ordinary repositories, `src` after `src.rs`. The raw
  directory mode is `40000`; `git cat-file -p` prints `040000`, and a parser written from the
  display refuses every directory that exists.
- The **raw-name** check is separate from the ordering check on purpose: a file `a` has sort key `a`
  and a directory `a` has key `a/`, so a tree naming one path twice passes an ordering check while
  leaving the walk to take whichever entry came first.

### Not evaluated is not an answer

Every definite outcome of a read or a walk — value **and** error — leaves through one gate that
consults the deadline first. Work that finished after the budget expired describes the store as it
was before the answer was due, so publishing it turns Beacon's slowness into a claim about the
Operator's repository. Routing every arm through one gate is what makes that true of all of them
rather than of most of them.

The deadline tests are staged rather than immediate, and the count of checks a successful read
performs is a **pinned constant**. An earlier version measured the count and then expired at it,
which meant that against a mutated implementation it measured the mutant and recalibrated around the
missing guard — an assertion that could not fail. Changing that constant is a decision someone
writes down.

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
