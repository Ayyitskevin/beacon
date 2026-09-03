---
status: proposed
---

# Dispatch a change only on an in-force Ruling, an exact Proposal, and one Candidate

Proposal v1 (ADR 0034) records an immutable description of intended effects and
creates no authority. Ruling v1 (ADR 0037) records that the Owner decided about
one exact Proposal, and creates no execution authority either. Preflight
(ADR 0027) establishes whether execution would be admissible and then refuses to
execute; a disposable worktree (ADR 0030) provides a tree; a Landlock ruleset
(ADR 0029) and a descriptor-bound working directory (ADR 0032) bound what runs
inside it. Nothing in that chain mutates a Git repository at all, and the
Blueprint's Alpha flow needs exactly one act that does — in v1, as the Decision below
scopes it, reaching only a Beacon-made disposable copy and never the Operator's own
repository: "Allow dispatches the
prepared Proposal only if its digest and base revision still match"
(`docs/BLUEPRINT.md:826`), followed by "Execution applies the bounded local change
and emits a complete Receipt" (`:827`).

Two Blueprint criteria bound that act from opposite sides. "Every mutation is
attributable to an actor, Proposal, policy version, and Receipt" (`:905`) says
what a dispatch must record. "A denied or stale Proposal causes zero repository
mutation" (`:907`) says what it must not do, and a negative criterion is only
closed by a mechanism that can be measured. The gates were therefore built and proved before
the landing existed, and those measurements are still the row-34 tests.

**A Ruling names a Proposal; a Proposal carries no content.** Its declared
mutations name a kind and a path and nothing else. Nothing in either record's
identity covers the bytes a dispatch would write, so a dispatch that accepted
content at call time would apply bytes under the authority of a decision that
never saw them. That gap is the reason this ADR introduces a third record rather
than only an act.

## Decision

Dispatch is **one Command with sixteen gates, one irreversible step, and a closed outcome
vocabulary**, and it is the only act in Beacon that mutates a Git repository on the strength of
a record. It runs as the authenticated Operator and no transport exposes it; the named desktop
Command that would make it Operator-*triggerable* is a condition on exposing it, stated under
"Supervised only, and human-only by construction" below and not claimed here. **In v1 that repository is a Beacon-made
disposable copy and nothing else**, enforced by a hard provenance gate: landing into
a repository the Operator owns stays disabled, for the reasons in "Conditions on the
landing slices" below. Everything this section says is scoped by that sentence.

```text
dispatch(ruling_digest, content) -> DispatchReceipt
```

The gates run in this order, and the order is part of the decision. Everything the store can answer
runs before anything the filesystem must, so a refusal costs no I/O and cannot be attributed to a
repository it never opened; the content check sits with the record gates for the same reason. A
request with several faults is therefore refused by the earliest gate in this list, and multi-fault
precedence is pinned by test rather than left to reading order.

Every refusal at any of these gates: **a refusal at any gate G1-G16 happens before
anything is written** — before one object enters the repository's object database, and
before the transaction that could publish anything opens. A refusal after the commit step
W3 is a different claim, made precisely below.

```text
  G1  Operator authenticated
  G2  Ruling re-verified from its own bytes (hash, decode, re-encode, columns)
  G3  decision == approve
  G4  force.superseded_by() is none
  G5  force.drift() is empty
  G6  the Proposal's stored bytes hash to the digest the Ruling names
  G7  the Proposal's live unmet preconditions are empty (see the gap below)
  G8  exactly one Candidate for that Proposal, recorded at a LOWER event
      sequence than the Ruling
  G9  the Candidate binds the same base revision and exactly the Proposal's
      declared mutations, in the Proposal's order
  G10 the Project's autonomy profile is Supervised
  G11 the Installation is not paused
  G12 the supplied content is within the Proposal's own byte and file-size
      bounds, and hashes to the Candidate's post-image digests
  G13 the Repository Binding names a local path Beacon itself recorded creating,
      by device and inode -- not by pathname
  G14 the Candidate's declared pre-images are the bytes the pinned base tree
      actually holds
  G15 LIVE: the repository's HEAD is a branch, equals the pinned base revision,
      and its tree is clean
  G16 this Ruling has not already been dispatched
  ------------------------------------------------------------- no writes above
  W1  the pinned base tree read into a scratch index outside the repository
  W2  one blob per mutation from `hash-object --stdin` with no path, then one
      `update-index` entry each -- no checkout, no index of the repository's,
      and no pathspec
  W3  write-tree, then commit-tree
  W4  post-condition: base..commit touches exactly the declared set, and every
      blob in it holds the declared bytes
  ------------------------------------------------------------- still discardable
  L1  git update-ref <branch> <candidate> <pinned base>   << THE ONLY
                                                             IRREVERSIBLE STEP
```

`Refused` is a fact about the request. A refusal at any gate G1-G16 leaves the
repository byte-identical, which is the strong claim and the one row 34 needs; a
refusal after W3 leaves the source refs and working tree unchanged and **may leave
unreachable objects**, which the Receipt names. `Applied` names the resulting
revision, verified after the fact. `LandingUnobserved` means the swap produced no
establishable result, and the Receipt names the commit to check. The three never collapse, for the reason ADR 0027
gives about preflight's outcomes: a negative answer, an error, and a step that did
not finish are different facts.

## Candidate v1: the record that carries what a dispatch would write

`beacon.candidate.v1` is an immutable, digest-identified record bound to one
exact Proposal, carrying one entry per declared mutation:

```text
record(proposal_digest, images) -> Candidate
get(candidate_digest)           -> Candidate
for_proposal(proposal_digest)   -> Candidate | none
```

Each entry holds a `MutationKind`, a repository-relative path, and the **SHA-256
of the file's exact bytes** before and after the change — the identity ADR 0034
already defines for readable context, deliberately not the Git object name, which
is a SHA-1 over a header and the same body. A Candidate is the output-side twin
of that input-side manifest.

**Digests only. A Candidate never carries file content.** Content is supplied to
the dispatch Command and checked against these digests, so this record stays
inside the same 64 KiB envelope a Proposal does, and no blob store is invented
for a slice that does not need one.

**The kind and the path of every entry are copied from the verified Proposal**,
never accepted from the caller, for the reason a Ruling copies its bindings: a
record that asserted a target it never read would be inferring exactly what
copying exists to stop. A caller supplies images, one per declared mutation, in
the Proposal's own order.

**A Candidate selects bytes; it does not authorize them** — `beacon.ruling.v2`
below does. Two rules make the selection determinate: one the Candidate store
enforces, one dispatch applies later.

- **One Candidate per Proposal**, enforced structurally by a unique index rather
  than by a Command that could be bypassed. Two sets of post-image digests would
  leave "which bytes" with no answer and force dispatch to choose. It does not
  choose: the second record is refused, and a revised change is a new Proposal —
  which is what ADR 0034 already says about anything that changes a Proposal's
  identity.
- **Recorded before the Ruling**, compared against `events.sequence` — the store's
  own ordering primitive, so the rule needs no clock and cannot be defeated by two
  records in one millisecond. This one is a **dispatch-time gate**, not a Candidate
  store invariant: the `candidates` table does not refuse a late record, and the
  Ruling that binds it is what makes the order matter.

Together they mean that at the moment the Owner ruled, exactly one set of
post-image digests existed for that Proposal. That is byte selection, and it is all
it is.

### What that does not prove, and what must land before a dispatch may apply anything

**Prior existence is not authorization.** A non-author review produced the smallest
counterexample: record Candidate C for Proposal P at sequence 11, present only P's
intent to the Operator, record approving Ruling R at sequence 12, then supply bytes
matching C. Every gate above passes, and the Operator authorized "modify README" —
not those bytes. `RulingV1` has no Candidate field, so nothing in the authority
record binds the change that was shown.

The Candidate therefore closes **byte selection**: dispatch applies one determined
set of digests rather than whatever a caller hands it, and a second set makes the
Proposal undispatchable. It does not close **human authority**, and this ADR does
not claim it does.

The authority link is **`beacon.ruling.v2`**: v1's payload plus the Candidate's full
digest envelope, recorded by a decision surface that presents that Candidate's exact
diff and identity before the Operator rules. A separate dispatch-authorization record
was considered and refused: one human act should carry the authority rather than two
objects that can disagree.

**Ruling v1 records stay valid and are simply not dispatchable.** Nothing ruled so
far is, so nothing already recorded is invalidated, and ADR 0037 stays accepted
exactly as written — v2 is a new canonicalizer beside it, never a reinterpretation of
v1 bytes. Recording a v2 Ruling requires the Candidate to exist and to bind the same
Proposal, so the two records cannot name different changes.

Required evidence: the authorized Candidate is the one applied, and a mutant that
swaps the Candidate after the displayed Proposal while preserving kind, path, and
base goes RED.

Two further rules are the codec's, because a record that cannot be dispatched
should not be expressible. **One entry per path**: v1 dispatch applies one effect
per path, and two entries would leave the resulting diff with one row for two
declarations, so W4's post-condition could not be stated. **No empty effect**: a
modification whose before and after are the same bytes declares no change, and
the mutation it mirrors declared one.

## The exact canonical bytes of `beacon.candidate.v1`

A grammar that is described but not published has no identity: a non-author reviewer wrote an
encoder from the design's field list alone and produced a record whose bytes were self-consistent
and not this one's. The table below is the whole grammar, and every field is fixed-width or exactly
length-prefixed, so no value has a second spelling.

| # | field | encoding |
|---|---|---|
| 1 | magic | `beacon.candidate.v1\0`, 20 bytes |
| 2 | Proposal digest value | 32 raw bytes |
| 3 | Proposal digest algorithm | `u64` big-endian length, then UTF-8; must be `sha256` |
| 4 | Proposal digest canonicalizer | `u64` length, then UTF-8; must be `beacon.proposal.v1` |
| 5 | pinned base revision | `u64` length, then 40 lowercase hex characters |
| 6 | entry count | `u32` big-endian, at least 1 |
| 7 | entry: kind | one byte: `01` create, `02` modify, `03` delete |
| 8 | entry: path | `u64` length, then UTF-8 |
| 9 | entry: pre-image | `00` absent, or `01` and 32 raw bytes |
| 10 | entry: post-image | `00` absent, or `01` and 32 raw bytes |
| 11 | recorded at | `u64` big-endian milliseconds |

Fields 7-10 repeat exactly `entry count` times, in the Proposal's declared-mutation order. The
record ends after field 11: trailing bytes are refused, and so is input that ends inside a field.

Refusals, all applied by the encoder and the decoder through the same predicates, so a record that
can be written can always be read back:

- an uppercase digest or base revision is **refused, never folded** — two spellings of one value
  would be two identities for one record;
- a Proposal digest envelope naming any other algorithm or canonicalizer;
- a path that is not repository-relative: empty, absolute, containing a NUL, a backslash, an empty
  component, `.`, or `..`. No Unicode normalization and no case folding: both can name a different
  file;
- images that do not match their kind — a create with a pre-image, a delete with a post-image, a
  modify missing either;
- two entries naming one path;
- a modification whose pre-image and post-image are equal;
- an entry count that exceeds the 64 KiB envelope, or that could not fit in the remaining input.
  The count is multiplied by the smallest an entry can be (11 bytes) and compared with what is left
  **before the loop reserves anything**, so a hostile count is refused by arithmetic rather than by
  allocation.

Two independent vectors are pinned, and both are checked by `crates/beacond/src/candidate.rs`:

**This project's fixture.** Proposal digest `aa`×32, base
`a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2`, entries
`(modify src/lib.rs 11→22)`, `(delete src/old.rs 33)`, `(create docs/new.md →44)`, recorded at
`1756000000000`: **344 bytes**, SHA-256
`fb7c33826e705f1d91b1a607f089561d77d997e502385393ff4b44bbca1349d4`. The hex was written out by
hand from this table and pinned; the encoder is checked against it, not the other way round.

**The cross-lab vector.** A non-author reviewer's encoder, given Proposal digest bytes `00..1f`,
base `0123456789abcdef0123456789abcdef01234567`, and entries
`(create docs/new.txt →20..3f)`, `(modify src/lib.rs 40..5f→60..7f)`, `(delete old.txt 80..9f)`,
produced **334 bytes**, SHA-256
`05d7e247e3285b18cf2579b3486e5f12244fa6ebdb532ea1f20d6c7417b01ceb`. That record stops at the last
entry, because field 11 is the one field the design's summary did not name. With
`recorded_at_ms = 1756000000000` appended, this grammar produces **342 bytes**, SHA-256
`532c2311082d3eb98d0bde53a9f276fd02ad52f12985096d7c6f64dce9d36ff1`, whose first 334 bytes are the
reviewer's record byte for byte. Both facts are pinned in
`the_cross_lab_vector_reproduces_byte_for_byte`.

`recorded_at_ms` is kept rather than dropped: every other record in this store carries its own time
in its bytes, the audit Event's `occurred_at_ms` is checked against it on every replay, and a
Candidate whose time lived only in a column could not be verified against the bytes at all.

## What "match" means, byte for byte

Three independent equalities, all required:

1. **The decision.** `sha256(proposal.canonical_bytes)` equals the value the
   Ruling names (G6), and the Candidate names that same digest in its own bytes
   (G9).
2. **The content.** Every pre-image digest gates a modification or a deletion;
   every post-image digest gates a creation or a modification; the supplied
   content is checked before the worktree exists (G12) and each written file is
   re-read and checked after (W2).
3. **The effect.** After the commit, `git diff --name-status -z base commit` must
   produce exactly the declared mutation set — same paths, same kinds, no extra
   row. This is what makes "applied exactly what was declared" a measurement
   rather than an assertion, and it is what catches a write that a hook, a
   filter, or a defect added on the side.

A kernel review measured two ways this was wrong as first written, and both
corrections are part of this decision:

- **Content never passes through a checkout or an index.** Git's built-in
  `text eol=lf` conversion — no external filter, so the filter-driver hardening never
  sees it — turned CRLF Candidate bytes into an LF blob with a different SHA-256
  while `diff --name-status` still reported the declared `M`. Blobs are therefore
  written with `hash-object --no-filters` and the tree and commit constructed from
  them, so the declared bytes *are* the object bytes. Post-image digests are verified
  against the blobs in the prepared commit's tree, which is the artifact that gets
  published.
- **No pathspec is passed at all**, which is also how `git add -- ':(glob)*.txt'`
  staging a second, undeclared file stops being reachable. Where a path list is read
  back — `diff --name-status` — it is consumed NUL-delimited with
  `--literal-pathspecs`, because a Proposal path may contain a newline.

## What a kernel review measured, and what it changed

Every mechanism below was probed directly against `main` on this host (Git 2.43.0) by a non-author
reviewer. Two held: the hardened Git shape executed **zero** planted `post-checkout`, `post-merge`,
`pre-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `core.fsmonitor` and clean/smudge
programs, and `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS` refused `..`, an outward
symlink and `/proc/self/fd/0` with `EXDEV`. Four did not, and this decision is what they leave:

**The landing primitive is a compare-and-swap, not a fast-forward merge.**
`git merge --ff-only` requires HEAD to be *an ancestor* of the candidate commit, not equal to its
first parent. With the branch moved **backward** to an ancestor after the head check, the merge
exited 0 and published the commit. The landing step is therefore
`git update-ref <branch> <candidate> <pinned base>`, which fails unless the ref still holds exactly
the value the gate observed. Where a checkout is materialized afterwards it is a separately
observable step and is not described as atomic with the ref.

**The source repository is pinned by identity, not by pathname.** `Worktree::create` canonicalizes
to a path and every Git child re-enters it by name, so renaming the directory and installing an
equivalent decoy at the same pathname redirected the mutation: the decoy advanced, the real
repository stayed at base. Dispatch pins the repository once, retains its descriptor and identity
for the whole Run, enters it by descriptor, and fails closed if the name stops resolving to the
pinned identity. Deterministic swaps between the head check and the worktree, and between the head
check and the landing, are required tests.

**Gate reads may not write.** `git status --porcelain` refreshes and rewrites `.git/index`, so the
head-and-cleanliness gate mutated the repository it was measuring — and the design's own
five-part fingerprint could not see it. Git hardening is split: a **read-only** variant carrying
`GIT_OPTIONAL_LOCKS=0` for every gate read, and the mutating variant for the worktree and the
landing.

**Zero mutation is measured over raw bytes.** The fingerprint is replaced by
`raw_repository_snapshot(root)`: a recursive walk of the whole repository root that runs no Git,
follows no symlink, sorts entries by raw `OsStr` bytes, and hashes each entry's relative name, file
type, mode, symlink target, length and full contents — `.git` included, so the index, reflogs,
pseudorefs, config, worktree metadata, and untracked and ignored files are all in scope. Its own
control proves that one changed byte of `.git/index`, of a loose object, or of worktree metadata
changes the snapshot. It establishes **final byte identity** and is never described as proof that no
transient file existed.

**Exactly once is the compare-and-swap, not a row written afterwards.** Two dispatchers each
running a fast-forward merge both exit 0 — the second reports the branch already current. The CAS
closes that by itself: the second attempt finds the ref no longer at the pinned base and refuses.
The `Applied` row is written in the same serialized step as the swap, not as the thing that makes
application unique. What is still persisted **before** the act is `{run, source identity, branch,
pinned base, prepared commit}`, for one narrower purpose: an attempt whose result was never observed
must be able to name the exact commit to inspect, and neither a worktree nor an in-memory Candidate
is recovery state.

**Objects are written as declared, without attribute processing.** Git's built-in `text eol=lf`
conversion has no path into the object store if content never passes through the index or a
checkout: blobs are written with `hash-object --no-filters`, and the tree and commit are constructed
from them. The post-condition "`base..commit` touches exactly the declared set, byte for byte" is
then a fact about **objects**, which is what gets published, and pathspec magic never arises because
no pathspec is passed.

**The checkout is Beacon's, in v1 only.** Inside the disposable-copy scope Beacon owns the working
tree and updates it after the CAS. A compare-and-swap into a repository the Operator also has
checked out would leave their working tree showing the inverse of the applied change; that is a real
problem, it belongs to the first slice that touches an Operator's own repository, and this ADR
records it rather than claiming v1 solved it.

## The exact canonical bytes of `beacon.ruling.v2`

Two non-author reviewers built encoders from the previous draft's prose alone and both reported the
same gap: "v1's payload plus the Candidate's full digest envelope" does not determine an identity.
It does not say the magic, whether the envelope precedes or follows the Proposal's, whether a
presence tag separates it, or which tokens it may carry — and each reading produces a different
digest for the same facts. The table is therefore published, and a specimen is pinned.

| # | field | encoding |
|---|---|---|
| 1 | magic | `beacon.ruling.v2\0`, 17 bytes — v1's magic with one digit changed |
| 2 | Proposal digest value | 32 raw bytes |
| 3 | Proposal digest algorithm | `u64` big-endian length, then UTF-8; must be `sha256` |
| 4 | Proposal digest canonicalizer | `u64` length, then UTF-8; must be `beacon.proposal.v1` |
| 5 | Candidate digest value | 32 raw bytes, **immediately after field 4**, with no presence tag |
| 6 | Candidate digest algorithm | `u64` length, then UTF-8; must be `sha256` |
| 7 | Candidate digest canonicalizer | `u64` length, then UTF-8; must be `beacon.candidate.v1` |
| 8-11 | the four copied revisions | as ADR 0037: three `u32`, then the 40-character base revision |
| 12-15 | decision, reason, decider, supersedes | exactly as ADR 0037 |
| 16 | ruled at | `u64` big-endian milliseconds |

Fields 5-7 are the whole difference. There is no presence tag: the grammar in the magic already says
whether a Candidate is named, so a tag would be a second place to say one thing. Everything from
field 8 on is v1's tail byte for byte, which is what makes "v2 is v1 plus one envelope" checkable
rather than asserted — `the_two_grammars_differ_only_by_their_magic_and_the_candidate_envelope`
checks it.

**The pinned vector.** ADR 0037's published v1 specimen — Proposal digest
`3b1f8c0e…9c1e`, revisions 1/2/3, base `a1b2…a1b2`, `approve`, the same reason string, `owner`, no
predecessor, `ruled_at_ms = 1756800000000` — with ADR 0039's own Candidate vector
`fb7c33826e705f1d91b1a607f089561d77d997e502385393ff4b44bbca1349d4` as fields 5-7, encodes to **307
bytes**, SHA-256 `18133ea4f334f0282a08e1af4495354849c8725bf2c9d71d6187ee9899ec8264`. It was produced
independently, in Python, importing no Beacon code, and is pinned as literal bytes in
`crates/beacond/src/ruling.rs` — because a runtime-built expectation is not an identity pin: an
encoder and a hand-built expected buffer can drift together and nothing outside the file would
notice.

**A known limitation, stated rather than discovered.** Two same-width fields transposed produce a
different digest but the same length, so a byte count alone does not detect a transposition. The
literal vector does.

## Conditions on the landing slices

The record slices (Candidate v1) stand alone. Every condition below binds the slices
that write to a repository, and each came from a non-author review of this design.

**Authority is read through transaction-bound, owning-module readers.** A service
method that opens its own store closure returns a snapshot, and a `RulingForce`
derived a moment ago is not a fact about the moment of landing. Each owning module
exposes a crate-internal reader that takes the caller's `&Connection` and performs
its own full proof — digest over the stored bytes, strict decode, re-encode,
indexed-column equality, and the Event pairing — before any field of its record is
used. Dispatch never assembles authority by joining tables.

**Authority linearizes at one point.** Nothing in the gate list stops a Ruling from
being superseded, or a policy or binding from moving, between G4/G5 and L1. A final
re-read without serialization narrows that window rather than closing it. So one
`BEGIN IMMEDIATE` transaction holds the store's write lock — which serializes more than
per Proposal, and is therefore stronger than the rule requires — across the Proposal's
wall-clock ceiling, the full authority re-verification (Ruling proof, approve, not
superseded, no drift, Supervised, not paused), the uniqueness claim, the ref
compare-and-swap, and the Receipt, Event and terminal Run state. A dispatch that is
superseded, drifted, paused or demoted in that window is refused at the last moment it
could be, and a deterministic two-connection test drives both races.

**Supervised only, and human-only by construction.** ADR 0004 gives Observe,
Supervised, and Bounded; `BLUEPRINT.md:405-406` says Observe permits **no external
mutation**. v1 dispatch requires Supervised, refuses under Observe, and defers
Bounded. Authentication proves an account, not a human, so the Command boundary is where
"human-triggered" becomes executable. **What is built is the negative half**: `actor: owner`
is derived inside the service and never accepted from a caller, and an inventory test with
a positive control proves the transport has no dispatch, pause or resume arm — dispatch is
reachable only in-process. **The positive half is a condition, not a claim**: one named
desktop Command with its own capability must exist before dispatch is exposed to anything
outside the crate, and this ADR does not assert that it does.

**Pause Work gates dispatch.** `BLUEPRINT.md:377-379` and `:1193-1194` and
`AGENTS.md:128` say a paused Installation rejects new dispatches, and recording
"pause not verified" in a Receipt would not make a forbidden dispatch permissible. So
this decision adds the minimum that makes the gate real: one Owner-set singleton, with
`pause()` and `resume()` as **separate** Commands because they are not symmetric --
Resume refuses while any Run is open or `unconfirmed`, naming them, or records what was
explicitly accepted (`AGENTS.md:137`) -- each writing its own audit Event in the same
transaction as the state change. The Blueprint's fuller Pause (bounded interruption
requests to active Runs, Worker lease renewals, the tray and the global shortcut) is
**not built and not claimed**. Pause is **not** one of the seven undeclared families
below.

**A Candidate is used to support a Ruling, so it is a Material Artifact.**
`CONTEXT.md:199-201` defines one as an Artifact used to support a Ruling or authorize
effects — a Candidate does the first, and a v2 Ruling turns it into the second — and
`BLUEPRINT.md:627-631` requires a non-author Evaluation before it may do either. v1
has no Evaluation citation. The scope this ADR asks the Steward to accept, in
writing rather than by implication: **the whole v1 landing path reaches only
Beacon-made disposable copies**, enforced as a hard provenance gate in code rather
than as a fixture convention. Landing into an Operator's own repository stays
disabled until Evaluation grows its Candidate arm — Evaluation v1 reserved an Artifact
arm for exactly this — and until the Safety Snapshot requirement
(`BLUEPRINT.md:450-452`) is met. Accepting this ADR accepts that scope and no more.

**What a refusal preserves, stated precisely.** "Byte-identical" is true of a refusal
before W3 and false after it: a commit written into a worktree leaves objects in the
shared object database that no ref names. The claim this ADR makes, and the one its
tests measure, is therefore in two parts: **every refusal leaves the source refs and
the working tree unchanged**, and a refusal after W3 **may leave unreachable objects,
which the Receipt names**. The zero-mutation tests measure refs, the working tree,
`.git/worktrees`, and the object listing separately, so the weaker claim cannot pass
for the stronger one.

**Every material Run terminates with a Receipt, and the vocabulary keeps ADR 0031's
meaning.** `AGENTS.md:154` admits no exception. ADR 0031 already reserves the Run
state `unconfirmed` for a lease that expired with **no terminal Receipt**, so this
ADR does not reuse that word for a Receipt outcome. Three distinct states:
`Refused` for a pre-mutation gate, with the repository unchanged; a
post-preparation refusal that names its residue; and `LandingUnobserved` for a
Receipt that exists and whose Git result could not be established — which names the
prepared commit, because the reservation record above persisted it before the act.
A Run whose lease expires without any Receipt stays `unconfirmed` only until startup,
which writes the Receipt that terminates it from that same durable record — asking the
ref what became of the prepared commit, and recording `LandingUnobserved` naming that
commit whenever the repository cannot be read or no longer has the identity that was
pinned.

**What startup does with a Receipt that already exists is narrower, deliberately.** It
re-verifies the record: the stored bytes against the Event's bytes, the outcome against
the Run state, the invariants that outcome arm owes, and the identity of the durable
preparation the landing transaction wrote. It does **not** re-read the repository, and an
`Applied` Receipt is not a claim that the branch still names that commit — `resulting_revision`
is a landing-at-time fact. Ordinary history continues after a dispatch, and a Receipt whose
truth expired at the next commit would be no evidence at all.

## The gap this decision proceeds past, in its own words

ADR 0034 says of the seven constraint families — execution policy, sandbox,
network, model/provider, command allowlist, tool allowlist, and evaluations —
that they are "the constraint families a future readiness check must resolve
**before dispatch is conceivable**". In v1 they cannot be resolved: submission
refuses any `ConstraintBinding` other than `NotDeclared`, so every v1 Proposal
carries all seven as unmet preconditions, permanently. Read literally, ADR 0034
says dispatch is not conceivable yet.

**This ADR proposes to proceed anyway, and accepting it is accepting that.**
The compensating conditions, each of which is a fact about the code rather than
an intention:

- Every dispatch runs as an authenticated Operator, derived at the boundary. No Agent,
  model, scheduler, or background task can reach it: no transport exposes it, proved by
  an inventory test with a positive control, and the actor vocabulary is a single closed
  arm for exactly that reason.
- Every dispatch Receipt names all seven families in its `not_verified` list, so
  no record of an applied change can be read as having checked them.
- The act has exactly one irreversible step: a compare-and-swap of one ref against the
  exact revision the gates observed, in a repository Beacon made and pinned by descriptor.
  (An earlier draft said "fast-forward"; a kernel review showed a fast-forward merge does
  not check that the ref still holds the observed value, and R9 withdrew it.) The prior
  state stays reachable by object name and is recorded as `pinned_base_revision`. That is
  not a Safety Snapshot (`BLUEPRINT:450-452`), and this ADR does not claim it is.
- Dispatch is gated on the Project's readable-context comparison having *agreed*.
  A context that could not be verified refuses, carrying its reason.

G7 therefore checks the **nine live preconditions** — the two readable-context
outcomes, the four binding-drift arms, `ResidentLifecycleChanged`,
`ConfigurationChanged`, and, as a positive requirement rather than an absence,
that a readable context was declared at all — and records the other seven as an
open gap rather than silently treating "unmet" as "unimportant".

## What Beacon writes into the history it creates

In v1 that history is a Beacon-made disposable copy's, not the Operator's. The identity is
nevertheless decided now rather than when the scope widens, because it is not retroactive:
every commit already minted keeps the identity it was minted with, and a dispatch commit is
permanent bytes that may later be read, cherry-picked or pushed by a person. This ADR
proposes: author and committer `Beacon <beacon@localhost>`;
committer time equal to the Run's start, so the resulting revision is a function
of its inputs rather than of the clock; the Proposal's summary as the subject;
and the trailers `Beacon-Proposal:`, `Beacon-Ruling:`, `Beacon-Candidate:`, which
put criterion `:905`'s attribution chain inside Git itself.

**The Product Steward confirms this identity before any dispatch into a
repository that is not a Beacon-made copy.** A rename afterwards is not
retroactive: every commit already minted keeps the old identity.

## How that commit message is encoded, unambiguously

A Proposal summary may contain newlines and tabs (ADR 0034 permits both), so using it
verbatim above the authority trailers would let a summary forge trailer-shaped lines.
The message is therefore built, not concatenated: the subject is the summary's first
line with every control character except space removed and the result truncated on a
character boundary; the body is a blank line, then the three trailers, and nothing
else. A summary that contains a line looking like `Beacon-Ruling: …` cannot produce a
second trailer, and a test drives exactly that.

## Receipt v3, encoded

A non-author review built a Receipt specimen from an earlier field list and showed that
reordering only its members produced different bytes for the same facts. A field list is
not an encoding. This section is the encoding.

Two facts bound the problem. Receipts in this store are **not digest-identified**:
`PreflightReceipt` and `ExecutionReceipt` are serde JSON, and reconciliation replays them
by deserializing. But it does not compare only values — `check_preflight::verify_run_event`
requires the stored `receipt_json` and its Event payload to be **byte-identical** and their
parsed values equal. So within an implementation the bytes are already pinned by the store;
what this section pins is a form two implementations agree on.

**Encoding.** `serde_json` compact form (no insignificant whitespace), UTF-8, object
members emitted in the declaration order below and never sorted, `camelCase` names,
`deny_unknown_fields` on the way back in. Absence is `null`, never omission: a reader sees
the same thirty members on every Receipt and distinguishes "measured zero" from "not
established" by type, not by which keys are present. Numbers are JSON integers.
Enumerations are `snake_case` strings drawn from closed vocabularies.

| # | Member | Type | Absent when |
|---|---|---|---|
| 1 | `version` | integer, always `3` | never |
| 2 | `runId` | UUID string | never |
| 3 | `workspaceId` | UUID string | never |
| 4 | `projectId` | UUID string | never |
| 5 | `actor` | `"owner"`, the only arm the vocabulary has | never — derived, never accepted from a caller |
| 6 | `rulingDigest` | digest envelope `{algorithm,canonicalizer,value}` | never |
| 7 | `proposalDigest` | digest envelope | the refusal was the Ruling itself |
| 8 | `candidateDigest` | digest envelope | no Candidate could be read |
| 9 | `outcome` | closed `DispatchOutcome` token | never |
| 10 | `autonomyProfile` | `"observe"` \| `"supervised"` — Bounded is deferred and has no token | the profile could not be proved |
| 11 | `charterRevision` | integer | never — `0` before it is read |
| 12 | `projectPolicyRevision` | integer | never — `0` before it is read |
| 13 | `repositoryRevision` | integer | never — `0` before it is read |
| 14 | `sourceKind` | closed `RepositorySourceKind` token | no binding was read |
| 15 | `source` | string, the bound path | no binding was read |
| 16 | `sourceIdentity` | `{device,inode}`, integers | the repository was never opened |
| 17 | `branch` | full ref name, e.g. `refs/heads/main` | no branch was resolved |
| 18 | `pinnedBaseRevision` | 40-hex string | the Proposal could not be read |
| 19 | `observedHead` | 40-hex string | the head gate was never reached |
| 20 | `preparedCommit` | 40-hex string | nothing was written to the object database |
| 21 | `resultingRevision` | 40-hex string | the swap did not succeed, or was not observed |
| 22 | `diffDigest` | 64-hex SHA-256 over the prepared commit's own name-status output | no commit was prepared, or the record was reconstructed rather than measured |
| 23 | `checkoutUpdated` | boolean | never — `false` is a measured fact |
| 24 | `gitSteps` | array of `{step,argv,result}` in attempt order | never — `[]` when none was attempted |
| 25 | `modelInvocations` | integer | never — always `0` by construction |
| 26 | `costUsdMicros` | integer | never — always `0` by construction |
| 27 | `perMutation` | array of `{kind,path,preImage,postImage,effect}` in the Proposal's order | never — `[]` before a Candidate is read |
| 28 | `startedAtMs` | integer | never |
| 29 | `endedAtMs` | integer | never |
| 30 | `notVerified` | array of closed `NotVerified` tokens, fixed order | never — the seven undeclarable families are on every Receipt |

Every digest envelope in this record — `rulingDigest`, `proposalDigest`, `candidateDigest` —
is the same three-member shape this store uses everywhere: `algorithm`, `canonicalizer`,
`value`. A digest without the grammar that produced it names bytes without saying what they
were bytes of.

`gitSteps` is one entry per invocation actually made, in the order they were made, including
the ones that failed: `step` is a closed `GitStepName`, `argv` is the exact argument vector
(fixed text this build chooses plus the revisions and declared paths of the request — never a
caller's free text), and `result` is a bounded `GitStepResult` category, never dynamic stderr
text, so two implementations that run the same steps write the same bytes.

The contract carries what the accepted Receipt contract requires (`BLUEPRINT.md:790-800`)
or names each exception. A dispatch runs no Declared Check, invokes no model, and consumes
no sandbox: `modelInvocations` and `costUsdMicros` are therefore measured zeroes, and the
commands, Evaluations, checks and sandbox version a Declared Check Receipt carries have no
counterpart here — the absence is structural, not an omission, and `notVerified` names the
Evaluation gap explicitly (`non_author_evaluation_of_candidate`). G12 enforces the
Proposal's own byte, file-size and wall-clock bounds before any caller-supplied content is
read; the wall-clock ceiling is re-checked inside the landing transaction.

**Specimens.** Two vectors, built entirely from fixed inputs — UUIDs that count up, constant
digests, times 1 and 2 — so a second implementation compares bytes rather than prose. Both
literals are in `dispatch::tests` (`the_applied_receipt_specimen_is_exactly_these_bytes`,
`the_refused_receipt_specimen_is_exactly_these_bytes`) together with their lengths and digests:

```text
applied         2048 bytes  sha256 6d13ab8b7a003b679985758f61bf190f4c0ed538772c9b2e5c21b77d1ece069a
ruling_refused  1301 bytes  sha256 76305395d931d8464ed38b6980ba6f8abb649014ca44899380c19e67546af8e3
```

The two arms are not one record with different values. A `RulingRefused` leaves the gate list
at G3, so it has no Candidate, no repository, no autonomy profile and no objects: its specimen
carries `candidateDigest`, `autonomyProfile`, `sourceKind`, `source`, `sourceIdentity`,
`branch`, `observedHead`, `preparedCommit`, `resultingRevision` and `diffDigest` as `null`, and
that exact absence list is asserted against a **real** refused Receipt as well as against the
specimen. Each specimen must also satisfy the invariants its arm owes. A vector for a shape the
dispatch path cannot emit would be a vector for nothing. Three further tests pin the member list of real
Receipts in stored-byte order for the Applied, Refused and reconstructed arms
(`crates/beacond/tests/dispatch.rs`) — read off the text, not out of a parsed map, which
would assert alphabetical order and pass however the record were written. Reordering any two
members of the struct makes all five red.

**What `resultingRevision` means.** It is a **landing-at-time** fact: the revision the
branch named when this dispatch observed it, immediately after its own compare-and-swap.
It is not a claim about the ref now. Ordinary history continues after a dispatch, and
startup deliberately does not require the branch to still name that commit — a Receipt
whose truth expired the moment someone made the next commit would be worthless as
evidence. Startup re-verifies the record: bytes, Event, digests, outcome invariants, and
the durable preparation. It does not re-read the repository.

## Persistence and reconciliation

`candidates` holds the canonical bytes, the copied Proposal digest envelope, the
copied pinned base revision, the entry count, and the recorded time. Every column
is also in the bytes, and every read re-proves that the two agree — the digest
names these bytes, the bytes decode, they re-encode identically, and every indexed
column repeats what they say. Immutability is enforced by triggers, as it is for
Proposals, Rulings, Runs and Receipts.

Startup reconciliation re-verifies every Candidate from its bytes, re-derives the
agreement between its copied entries and its Proposal's declared mutations, and
requires exactly one `proposal.candidate_recorded` Event per Candidate with no
orphan. It deliberately re-derives **nothing about the world**: a Candidate
records what a change would do to a tree at one revision, and that record does not
become invalid because the tree moved. Dispatch re-establishes every live fact
itself, at dispatch time, from bytes.

## What this decision does not decide

- **Packed and delta objects.** The readable-context comparison reads only loose
  objects, so a `git gc`-ed repository reports `PackedObjectUnreadable` and is not
  dispatchable in v1. Named in the refusal rather than discovered.
- **Safety Snapshots and Policy impact classification.** Neither exists;
  `BLUEPRINT:450-452` and `:723` stay open, and the paragraph above says what
  stands in for them and what it is not.
- **Pause Work.** `BLUEPRINT:378` and `:1193` require a paused Workspace to reject
  new dispatches, and the singleton this decision adds is what the gate reads. What
  remains unbuilt is the rest of Pause -- interruption requests, Worker lease renewals,
  and the tray and shortcut surfaces -- and it is named here rather than implied.
- **Evaluation citation.** A v1 Ruling does not cite an Evaluation, so dispatch
  cannot require one and must not pretend to. Named in every Receipt.
- **Two mutations on one path.** Expressible in a Proposal, refused by a
  Candidate. Deciding how a collapsed effect should be reported is a later slice.
- **Anything an Agent may do.** Nothing here widens agent authority; there is no
  agent path to dispatch at all.

## Required evidence before implementation can land

The Candidate record (this slice):

- a Candidate names the exact Proposal and pinned base revision it was built
  against, and its bytes equal an independently built record of the same facts;
- its entries follow the Proposal's declared-mutation order, not its sorted paths;
- a second Candidate for one Proposal is refused by the store;
- images that do not match the Proposal's mutations — wrong count, an image the
  kind forbids, a modification that changes nothing — write neither row nor Event;
- recording a Candidate creates no Run, Receipt, worktree, or external effect;
- an unauthenticated caller receives only the account refusal;
- every byte of the canonical record is load-bearing, every closed vocabulary
  refuses an unassigned tag, and an uppercase digest or revision is refused rather
  than folded;
- a row that disagrees with its own bytes, a Candidate whose entries no longer
  match its Proposal, a Candidate with no Event, and a duplicated Event each fail
  startup.

The dispatch act (later slices, each proved before the next opens):

- a refused, superseded, drifted, or stale Ruling dispatches nothing: HEAD, every
  ref, the working tree, `.git/worktrees`, and the object listing are each measured
  separately and each unchanged, so the strong claim is proved where it holds rather
  than assumed everywhere;
- a refusal after the commit step leaves the refs and working tree unchanged and
  names its unreachable-object residue in the Receipt;
- the authorized Candidate is the one applied, and swapping the Candidate after the
  displayed Proposal — same kind, same path, same base — is RED;
- a Ruling superseded between the force check and the landing does not land, driven
  by two connections rather than by luck;
- dispatch refuses under Observe, and refuses while the Installation is paused;
- the dispatch service is absent from every Agent, MCP, schedule, and reconnect
  surface, proved by an inventory test rather than by prose;
- a dispatch Run whose lease expired reaches its terminal state through a Receipt
  written from the durable pre-landing record, while `unconfirmed` keeps ADR 0031's
  meaning;
- the branch moved **backward** to an ancestor after the head check does not land —
  the case that kills both a deleted head gate and the belief that a fast-forward
  merge is an exact-base compare-and-swap;
- a decoy repository installed at the pinned pathname between the head check and the
  worktree, and again between the head check and the landing, does not receive the
  mutation;
- a Candidate path carrying pathspec magic stages only itself;
- a Candidate whose bytes would be normalized on the way into the tree refuses,
  because post-image digests are compared against the prepared commit's blobs;
- two dispatchers of one authority produce one application, by reservation rather
  than by a unique row written afterwards;
- the raw repository snapshot changes when one byte of `.git/index`, one loose
  object, or one worktree-metadata file changes — the control without which every
  zero-mutation assertion above is weaker than it reads;
- a moved repository head refuses **even though every stored record still
  agrees**, which is the only test that fails if G11 is deleted;
- content that does not hash to the Candidate refuses before any write;
- a target that escapes the worktree refuses before anything is published;
- an effect wider than the declaration refuses rather than publishing;
- an in-force approval applies exactly once, and the second attempt refuses;
- the applied commit contains exactly the declared mutations.

## Consequences

The first end-to-end loop becomes provable: Proposal → Ruling → Candidate →
dispatch → Receipt, with a canned change and no Agent. Criterion `:907` gains a
mechanism it can be measured against, and criterion `:905` gains a Receipt that
carries the whole attribution chain. The cost is a third immutable record and a
new Run kind, and a standing obligation: the seven undeclared constraint families
stay in every dispatch Receipt's `not_verified` until they are real, and the day
they become real, G7 widens to include them.
