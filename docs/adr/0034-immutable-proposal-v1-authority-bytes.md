---
status: accepted
---

# Bind inert Proposal v1 to the exact bytes Beacon stores

Kevin accepted exact proposal commit
`5a3e400dcf501bb4e1174ea1315b8180c6d5b7df` on 2026-08-30 as Product Steward.
Implementation remains bound to the backend-only, inert fence and required
evidence below. Live migration, production transport, and execution exposure
remain separate decisions.

Beacon needs a stable Proposal identity before it can add Material Artifacts,
Evaluations, Rulings, or dispatch. A Proposal is an immutable description of
intended effects, inputs, bounds, checks, and budget, identified by its digest
(`CONTEXT.md`). Execution may later rely on that identity, so an object and a
caller-supplied digest must never enter a future authority path as independent
values. The bytes are the record.

This decision deliberately creates no authority. Proposal submission records an
Operator-attributed, Resident-requested description for review. It does not
create a Gate or Ruling, consult an execution capability, prepare a worktree,
start a process, invoke a model, make a network request, populate a Receipt, or
dispatch anything. No Proposal command crosses the production transport in this
slice.

## Decision

Beacon introduces `ProposalV1` behind one deep Rust interface:

```text
submit(canonical_bytes) -> Proposal
get(digest) -> Proposal
list_for_mission(mission_id) -> [Proposal]
```

`submit` authenticates the current Owner before inspecting untrusted bytes. It
hashes the supplied byte slice, strictly decodes that same slice, re-encodes the
decoded value, and requires byte-for-byte equality. There is no public function
that accepts `(ProposalV1, digest)`, parsed fields plus bytes, or a digest without
the bytes it names.

The immutable v1 payload contains these fields:

```text
scope
  workspace_id, project_id, mission_id
requester
  resident_id, resident_configuration_id
  owner_bootstrap_reason
bindings
  charter_revision, project_policy_revision, repository_revision
  pinned_base_revision
intent
  summary
  readable_context
  target_paths
  declared_mutations
  declared_check_runs
  declared_checks
requirements
  execution_policy, sandbox, network, model_provider
  command_allowlist, tool_allowlist, evaluations
bounds
  wall_clock_ms, cpu_seconds, address_space_bytes
  file_size_bytes, output_bytes, token_ceiling
budget
  no_spend
rollback
  discard_disposable_worktree
```

The Project and Mission identities are both present even though one can be found
through the other. The redundant scope makes a cross-Workspace or cross-Project
substitution independently detectable. The same rule applies to the Repository
revision and pinned base revision, and to the Resident and Configuration
identities. The Mission must belong to the named Project, the requesting Resident
must be one of its admitted members, and `resident_configuration_id` must equal
that member's immutable admission Configuration. The Policy and Repository
revisions must belong to the same Project and bind its named Charter revision.

`summary` must arrive trimmed and contain non-empty human text of at most 2,000
Unicode scalar values. The Owner bootstrap reason follows the same validation
with a 500-scalar limit. Neither value may contain NUL or non-whitespace control characters.
Future consumers render both as plain text, never as a link, path, or command.

`target_paths` are distinct UTF-8 POSIX paths relative to the Repository root.
Absolute paths, backslashes, NUL, empty components, `.` components, and `..`
components are refused. The canonical order is ascending raw UTF-8 bytes.
Callers must supply that order; Beacon refuses unsorted or duplicate input rather
than silently changing the submitted Proposal.

`declared_checks` contain the exact name, argument vector, and working directory
of each selected Declared Check. Every entry must match the bound Repository
revision exactly and use the Repository module's canonical name order.

`declared_mutations` is an ordered, duplicate-free closed v1 vocabulary:
`create_file(target)`, `modify_file(target)`, and `delete_file(target)`. Every
mutation must name one `target_paths` entry and every target must be named by at
least one mutation. `declared_check_runs` separately lists distinct check names
in Repository-canonical order, and every name must resolve in `declared_checks`.
Together they are the Proposal's declared effects; only mutations are inputs to
future Safety Snapshot classification. They are intended sequences, not
authorization or assertions that anything occurred.

V1 supports only `no_spend` and `discard_disposable_worktree`. Wider Budget
Envelopes, paid providers, remote effects, and a different rollback strategy
require a later versioned contract.

## Exact canonical bytes

The canonicalizer identity is `beacon.proposal.v1`. Its byte grammar is binary,
fixed-order, and self-delimiting:

- the literal ASCII magic `beacon.proposal.v1\0` starts every record;
- UUIDs are their 16 raw bytes;
- SHA-256 values are their 32 raw bytes;
- integers are unsigned big-endian values of the field's fixed width;
- enums use an explicitly assigned one-byte tag that is never renumbered;
- strings are valid UTF-8 preceded by an unsigned big-endian 64-bit byte length;
- byte strings and nested requirement identities use the same 64-bit framing;
- lists begin with an unsigned big-endian 32-bit count and encode entries in
  their specified order;
- no declared length or count is ever used to reserve capacity; the decoder
  first proves it fits within the remaining input and the field's own limit,
  then appends list entries as it decodes them;
- optionality is always a closed one-byte variant, never a missing field, null,
  empty string, zero, or sentinel digest; and
- the decoder consumes the entire input and refuses unknown tags, trailing
  bytes, over-limit lengths, invalid UTF-8, and non-canonical ordering.

Paths are not Unicode-normalized or case-folded. Those transformations can name a
different filesystem object. Human text is validated and encoded exactly as
submitted. `submit` refuses a slice larger than 64 KiB before parsing or making
any size-derived allocation.

The Proposal identity is SHA-256 over the exact canonical bytes. The stored
digest envelope records algorithm `sha256`, canonicalizer
`beacon.proposal.v1`, and lowercase hexadecimal value. The final digest is not
embedded inside its own preimage. Any future grammar is a new canonicalizer;
existing v1 bytes are never reinterpreted under v2 rules.

This follows the repository's existing length-framed binary digest precedent in
`crates/beacond/src/sample_project.rs` rather than introducing a new
canonical-JSON dependency. Every accepted byte difference changes the Proposal
identity; alternate encodings of the same parsed value are non-canonical and
refused.

## Readable context is explicit and digest-bound

`readable_context` is a mandatory tagged field with two distinct states:

```text
not_declared
declared(entries, manifest_sha256)
```

Each declared entry contains one validated Repository-relative path and the
SHA-256 identity of the exact file bytes presented as context. Entries use the
same canonical path order as targets. `manifest_sha256` is recomputed over the
canonical encoded entry list as part of strict decoding and must match. Submit,
get, list, and startup replay all reject the same nested mismatch. The entries
stay in the Proposal; the manifest digest is a comparison key, not a substitute
for them.

Internal manifest consistency does not prove that a path/digest pair existed at
the pinned base revision. A later readiness diagnostic independently compares
every entry with that revision when the bound Repository is locally available;
otherwise it reports that comparison as not verified. No surface may label the
context verified from the manifest's self-consistency alone.

`not_declared` is not ambient read access, a default, or an empty manifest. It
makes the Proposal incomplete for Ruling or dispatch. `declared([])` is the
different, positive assertion that no Repository file is readable. These states
produce different canonical bytes and different Proposal digests. A later
Proposal must be submitted to replace either state; the existing record is never
updated.

The current sandbox still permits broad host reads and Landlock does not govern
UDP. Therefore a declared readable context records intended scope only. It must
not be exposed as enforced or copied into Receipt evidence until the separate Run
I/O decision supplies a probed enforcement mechanism.

## Undeveloped constraint families fail closed

The Blueprint requires sandbox, network, model/provider, command/tool,
Evaluation, Policy, and budget constraints to participate in Proposal identity.
Several of those domains do not yet have an executable Beacon contract. Guessing
their internal shape in an irreversible v1 grammar would turn design intent into
false authority.

Each undeveloped constraint field therefore uses the same explicit binding:

```text
not_declared
bound(kind, version, canonicalizer, sha256)
```

The `bound` form identifies an immutable descriptor; it contains no secret
value, credential, URL, command text, or capability. The referenced descriptor
must exist in Beacon-owned content state, re-hash correctly, and be understood by
the owning module before a future readiness check can succeed. V1 submission
does not resolve, interpret, or execute it. Until those content and owning-module
surfaces exist, the only accepted value is `not_declared`.

This four-field value is the shared content identity, not the descriptor's
internal schema: `kind` and `canonicalizer` are lowercase ASCII tokens of at
most 64 bytes, `version` is a positive unsigned 32-bit integer, and `sha256` is
32 raw bytes. The descriptor's own canonical bytes and vocabulary remain owned
by its future module. A later module may accept the already-defined `bound` tag
only after a separate reviewed decision supplies that content state and parser;
it may not reinterpret an existing identity or silently treat an unknown kind as
supported.

The exact Project Policy revision remains a first-class binding rather than one
of these descriptor identities. Its current Team Lead policy is real persisted
state. `execution_policy` is separate because the current Project Policy does not
define execution admission.

Readiness and staleness are derived at read time and never stored. A Proposal is
not ready while any required constraint is `not_declared`, its readable context
is `not_declared`, a bound descriptor is absent or unsupported, or a current
Project, Policy, Repository, base, Resident lifecycle, or Configuration no
longer matches. Submission records a Proposal; it does not decide that the
Proposal may proceed.

Readiness is a diagnostic list of unmet preconditions, never a boolean
authorization. No interface may present it as permission to proceed.

## Truthful Resident request and Owner bootstrap

Proposal bytes always name the requesting Resident and the exact Resident
Configuration. The audit Event separately names the authenticated command actor
as `owner`. Beacon does not claim that an unauthenticated Resident issued the
command.

V1 requires a bounded, non-empty Owner bootstrap reason. It records that the
Owner selected this Resident to request this one Proposal before Beacon has a
Track Record and Team Lead selection path. The reason is inside the canonical
bytes, so it cannot become ambient authority or silently apply to another
Proposal. It creates no Track Record evidence and contributes nothing to future
selection.

There is no Operator UUID in the current singleton account model. V1 therefore
records the closed authorization basis `owner`, not an invented identity. V1 has
no `none` or `policy_selected` bootstrap variant: accepting either before the
corresponding evidence path exists would assert a check Beacon cannot perform.
A later evidence-backed selection contract is additive and does not rewrite v1.

## Persistence, idempotence, and reconciliation

The implementation will use schema version 18 and migration
`0018_proposals.sql`. One immutable Proposal row stores the canonical bytes,
digest algorithm, canonicalizer, digest value, scope and binding columns,
requesting Resident and Configuration, bootstrap basis and reason, and original
submission time. Every state discriminator is `NOT NULL`; meaningful absence is
an explicit parsed variant, never SQL null. Composite foreign keys preserve
Workspace and Project scope. Store triggers refuse update and delete.

An authenticated submission performs one immediate transaction:

1. hash, decode, re-encode, and compare the canonical bytes;
2. validate referent existence and Workspace/Project scope consistency using
   narrow crate-private readers owned by the relevant domain modules, without
   comparing any binding to the current revision;
3. insert the immutable Proposal row; and
4. insert exactly one `proposal.submitted` Event containing the digest envelope,
   exact canonical bytes as canonical RFC 4648 padded base64, actor `owner`, and
   the submission time.

If the digest already exists with identical bytes, submission returns the
original record and writes no second Event. The same digest with different bytes
is a hard store-integrity failure, not an idempotent match.

Startup recomputes the digest from stored bytes, strictly decodes and re-encodes
them, compares every indexed column to the parsed value, verifies all structural
references, and requires one Event per Proposal whose decoded byte field exactly
equals the stored Proposal bytes, with no orphan Proposal Event. It also verifies
the Owner account still exists for every v1 bootstrap record. Current readiness
is then derived; historical Proposals do not become corrupt merely because a
referenced current revision moved.

Migration 0018 follows the existing closed-Event-table rebuild pattern. It
preserves every v17 Event sequence, payload byte, and timestamp, adds only
`proposal.submitted`, creates empty Proposal state, and advances the version
only from 17. Rollback tests must prove the exact prior schema and evidence are
restored before forward startup.

## Required evidence before implementation can land

- a known byte fixture independently reproduces its SHA-256 digest;
- one changed byte, reordered set, changed list order, trailing byte, or
  semantically equivalent reformat produces a different identity or is refused;
- `not_declared` and `declared([])` readable contexts have distinct bytes and
  digests, and a mismatched manifest digest is refused;
- distinct requester, Configuration, bootstrap reason, Project Policy,
  Repository, base, targets, mutations, check runs, checks, bounds, and budget
  each change the digest;
- duplicate, escaping, absolute, and non-canonical paths are refused rather than
  normalized;
- unauthenticated malformed bytes return only the authentication refusal;
- cross-Workspace and structurally inconsistent references write neither row nor
  Event;
- identical resubmission is idempotent, while a forced digest/byte collision is
  refused;
- missing, duplicate, orphaned, altered, or differently encoded state/Event
  evidence refuses startup; and
- submission leaves Run, Receipt, Ruling, worktree, process, model Invocation,
  network, capability, and external-effect counts unchanged.

The full repository gate and a non-author exact-head review are required after
implementation. Applying migration 0018 to live data remains a separate RED
action.

## Rejected alternatives

- **Caller-supplied parsed object plus digest:** permits the values to disagree.
- **Canonical JSON:** adds a second canonicalization discipline and makes parser
  configuration part of the authority kernel.
- **Digest-only storage:** cannot prove what bytes were actually reviewed.
- **Mutable Proposal rows:** lets a Ruling drift onto different content.
- **Null or empty as absence:** collapses missing scope into a positive empty
  declaration.
- **Silent sorting, deduplication, or path normalization:** changes the submitted
  meaning before it is bound.
- **Invented Track Record or Operator identity:** records evidence the product
  does not possess.
- **Concrete unimplemented sandbox/provider schemas:** freezes guesses into v1
  and invites later code to treat declarations as enforcement.
- **Transport or desktop exposure in this slice:** creates an input surface before
  the authority, evaluation, and Run I/O paths exist.

## Product Steward decision

Kevin's acceptance of exact proposal commit `5a3e400dcf501bb4e1174ea1315b8180c6d5b7df`
decides that:

1. Proposal v1 uses the fixed binary canonicalizer above;
2. readable context is mandatory and digest-bound, with `not_declared` distinct
   from `declared([])` and always non-dispatchable;
3. unavailable constraint families remain explicit and non-dispatchable rather
   than receiving guessed defaults;
4. fresh Residents use one Proposal-scoped Owner bootstrap reason rather than
   fabricated Track Record evidence or an Operator UUID;
5. identical submission is idempotent; and
6. implementation remains backend-only and inert.

Proposal persistence implementation may now proceed only within this exact
contract and its required evidence. A material change to the canonical bytes,
identity semantics, authority boundary, or implementation fence requires a new
Product Steward ruling before merge. Applying migration 0018 to live data and
opening any production transport or execution surface remain separately gated.

## Amendment (accepted 2026-09-02): the byte layout, recorded

**Status: accepted 2026-09-02** — the Product Steward's ruling at the merge of
#38 (`6b0df56`). The decision above remains `accepted` as written; nothing here
changes it. This section records what `beacon.proposal.v1` *is*, at the level of
detail a second implementation needs, so the grammar can be reproduced without
reading Rust.

### Why an amendment, not a new ADR

The accepted text above requires "an explicitly assigned one-byte tag that is
never renumbered" and integers "of the field's fixed width" — and then assigns
neither. It states the shape of the grammar without stating the grammar. A
reader could not build a second encoder from it and get byte-identical output,
which is what the identity depends on.

That is a gap in the record, not a different decision. Every value below was
read out of the implementation this ADR authorised, and every one of them is a
choice the accepted text already committed to making. Recording them changes no
accepted behaviour. A layout that *differs* from this one is a different
canonicalizer and takes a new identity (`beacon.proposal.v2`), never a silent
change under the v1 name — which is the accepted text's own rule.

Derived from `main` at `b93822a` (the rebase-merged form of the branch commit this
amendment was written against — same patch-id), by reading `encode()`, its
decoder, and the constants they share.

### Ordered field layout

Every field, in `encode()` order. `u32`/`u64` are unsigned big-endian.
`string` is a `u64` big-endian **byte** length followed by that many UTF-8 bytes.
`list<T>` is a `u32` big-endian count followed by that many entries. `uuid` is 16
raw bytes; `sha256` is 32 raw bytes. Neither is length-framed.

| # | field | encoding | bound, and where the bound comes from |
|---|---|---|---|
| 1 | magic | 19 literal bytes `beacon.proposal.v1\0` | fixed; not length-framed |
| 2 | `scope.workspace_id` | uuid | — |
| 3 | `scope.project_id` | uuid | — |
| 4 | `scope.mission_id` | uuid | — |
| 5 | `requester.resident_id` | uuid | — |
| 6 | `requester.resident_configuration_id` | uuid | — |
| 7 | `requester.bootstrap` | tag | `0x01` = `owner`; the only assigned value |
| 8 | └ `reason` | string | ≤ 500 Unicode scalars (ADR); decoder byte bound 2,000 = 500 × 4 |
| 9 | `bindings.charter_revision` | u32 | ≥ 1 |
| 10 | `bindings.project_policy_revision` | u32 | ≥ 1 |
| 11 | `bindings.repository_revision` | u32 | ≥ 1 |
| 12 | `bindings.pinned_base_revision` | string | exactly 40 bytes, `[0-9a-f]` (ADR) |
| 13 | `intent.summary` | string | ≤ 2,000 Unicode scalars (ADR); decoder byte bound 8,000 = 2,000 × 4 |
| 14 | `intent.readable_context` | tag | `0x01` = `not_declared`, `0x02` = `declared` |
| 15 | └ if `declared`: entries | `list<context_entry>` | envelope |
| 16 | &nbsp;&nbsp;└ `path` | string | envelope |
| 17 | &nbsp;&nbsp;└ `content_sha256` | sha256 | — |
| 18 | └ if `declared`: `manifest_sha256` | sha256 | — |
| 19 | `intent.target_paths` | `list<string>` | envelope |
| 20 | `intent.declared_mutations` | `list<mutation>` | envelope |
| 21 | &nbsp;&nbsp;└ `kind` | tag | `0x01`/`0x02`/`0x03` |
| 22 | &nbsp;&nbsp;└ `target` | string | envelope |
| 23 | `intent.declared_check_runs` | `list<string>` | each name ≤ 64 bytes (Repository Binding) |
| 24 | `intent.declared_checks` | `list<declared_check>` | envelope |
| 25 | &nbsp;&nbsp;└ `name` | string | ≤ 64 bytes (Repository Binding) |
| 26 | &nbsp;&nbsp;└ `argv` | `list<string>` | envelope; Repository Binding sets no per-argument limit |
| 27 | &nbsp;&nbsp;└ `working_directory` | string | envelope; Repository Binding sets no length limit |
| 28 | `requirements` | 7 × `constraint`, fixed order | see below |
| 29 | `bounds.wall_clock_ms` | u64 | zero is legal |
| 30 | `bounds.cpu_seconds` | u64 | zero is legal |
| 31 | `bounds.address_space_bytes` | u64 | zero is legal |
| 32 | `bounds.file_size_bytes` | u64 | zero is legal |
| 33 | `bounds.output_bytes` | u64 | zero is legal |
| 34 | `bounds.token_ceiling` | u64 | zero is legal |
| 35 | `budget` | tag | `0x01` = `no_spend` |
| 36 | `rollback` | tag | `0x01` = `discard_disposable_worktree` |

The seven `requirements`, in this exact order: `execution_policy`, `sandbox`,
`network`, `model_provider`, `command_allowlist`, `tool_allowlist`,
`evaluations`. Each is a `constraint`:

```text
constraint := 0x01                                        -- not_declared
            | 0x02 kind:string version:u32 canonicalizer:string sha256:sha256
```

`kind` and `canonicalizer` are ≤ 64 bytes and `version` must be ≥ 1. The
`sha256` member is 32 raw bytes and is **not** length-framed: the "byte strings
and nested requirement identities use the same 64-bit framing" bullet above
applies to the two string members, and the requirement-identity paragraph's own
"`sha256` is 32 raw bytes" governs the digest. Only `not_declared` is accepted
by submission today; `0x02` decodes so that a later reviewed decision needs no
new tag, and is then refused.

"Envelope" above means the record's own 64 KiB limit, not a narrower per-field
number. It is what governs every field the accepted text does not size
individually. The accepted text does size four: the 64 KiB whole-record envelope,
the 40-character pinned base revision, the 2,000- and 500-scalar limits on the
summary and the Owner bootstrap reason (`:82-84`), and the 64-byte descriptor
tokens (`:206-209`) — all of which the table above records. Everything else takes
the envelope, except where a field must byte-match another module's record, where
the limit is that module's and never a tighter one invented here: Repository
Binding caps a check name at 64 and places no limit on an argument or a working
directory, so neither does this grammar.

### Tag assignments

Every one-byte tag, by position. **`0x00` is unassigned everywhere**, so a
zero-filled buffer cannot decode as any valid variant. Any value not listed is
refused as an unknown tag; none is ever renumbered.

| position | `0x01` | `0x02` | `0x03` |
|---|---|---|---|
| bootstrap basis | `owner` | — | — |
| readable context | `not_declared` | `declared` | — |
| mutation kind | `create_file` | `modify_file` | `delete_file` |
| constraint (each of the 7) | `not_declared` | `bound` | — |
| budget | `no_spend` | — | — |
| rollback | `discard_disposable_worktree` | — | — |

### Ordering, distinctness, and the manifest preimage

`target_paths`, `declared_check_runs`, `declared_checks` (by name), and declared
readable-context entries (by path) are each **strictly ascending by raw UTF-8
bytes**. Equal neighbours are a duplicate; a descending pair is non-canonical.
Both are refused rather than sorted or de-duplicated.

`declared_mutations` is the caller's intended sequence: order is preserved,
carries identity, and is not checked for sortedness. Duplicates are refused on
the exact `(kind, target)` pair.

`manifest_sha256` is SHA-256 over the canonical encoding of the entry list
**including its `u32` count prefix** — byte-for-byte the bytes fields 15-17
occupy. It is recomputed during strict decoding and must match.

### Field validation

Widths and tags alone do not determine what is accepted. A second implementation
must apply these too, or it will accept records Beacon refuses.

**Human text** (`summary`, bootstrap `reason`): non-empty; equal to its own
trimmed form, so leading or trailing whitespace is refused rather than stripped;
within its scalar limit; and free of control characters other than whitespace
(which excludes NUL, and permits newline and tab).

**Repository-relative paths** (`target_paths`, mutation targets, declared
readable-context entry paths): non-empty; not beginning with `/`; containing no
NUL and no backslash; and no component that is empty, `.`, or `..`. Paths are
neither Unicode-normalized nor case-folded, so two paths differing only by
normalization form or case are two distinct entries.

**Declared check working directory**: `.`, or a repository-relative path by the
rule above.

**Declared check name and check-run name**: first character `[a-z0-9]`, the rest
`[a-z0-9._-]`, at most 64 bytes. This is Repository Binding's own rule; a
Proposal's declared checks must reproduce a bound check exactly, so the two
vocabularies cannot diverge.

**Declared check arguments**: the argument vector itself must be non-empty, and
each argument must be non-empty and free of NUL. Repository Binding imposes no
length limit on an argument, so neither does this grammar.

**Descriptor tokens** (`kind`, `canonicalizer` inside a `bound` constraint):
non-empty, at most 64 bytes, and drawn from lowercase ASCII letters, digits, `.`
and `-`. Note that `_` is **not** accepted here, though it is in a check name;
the accepted text says only "lowercase ASCII tokens", and this is the narrower
set the implementation chose.

**Pinned base revision**: exactly 40 bytes, each `[0-9a-f]`. Uppercase hex is
refused rather than lowered — a case-folded Binding would silently name the same
commit under a different identity.

**Cross-field rules**: every declared mutation names a `target_paths` entry, and
every target is named by at least one mutation; every `declared_check_runs` name
resolves in `declared_checks`, though a declared check with no run is accepted.
All four intent lists may be empty.

### Refusal before allocation

No declared length or count is used to reserve capacity; every list is built by
appending. Before decoding any entry, a count is checked against the envelope
and then against the remaining input using the smallest possible size of one
entry: 8 bytes for a string entry, 40 for a context entry, 9 for a mutation, 20
for a declared check. A count that could not fit is refused before the first
entry is read.

### Canonical fixture

A complete 690-byte v1 record and its identity. A second implementation that
produces these bytes for this value, and this digest for these bytes, agrees
with Beacon.

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

```text
sha256 = a40cb728defbb93cdad8c9c81286f31a8bca9287b92ec1baa5f260016f7ae404
```

Reproduce the identity with no Beacon code:

```sh
printf '%s' '<the hex above, joined onto one line>' | xxd -r -p | sha256sum
```

The value it encodes: scope `11111111-…`/`22222222-…`/`33333333-…`, requester
`44444444-…` at configuration `55555555-…` with an `owner` bootstrap reason;
charter, policy and repository revisions all `1`; pinned base revision
`a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2`; a declared readable context of
`src/lib.rs` and `tests/invariants.rs`; the same two files as targets, modified
and created respectively; one check run and one declared check
`cargo-test`/`["cargo","test","--workspace"]` in `.`; all seven requirement
families `not_declared`; bounds `300000, 240, 2147483648, 268435456, 1048576, 0`;
`no_spend`; `discard_disposable_worktree`.

### Readiness vocabulary as shipped

Readiness is a list of unmet preconditions with no boolean accessor, over this
closed vocabulary: `ExecutionPolicyNotDeclared`, `SandboxNotDeclared`,
`NetworkNotDeclared`, `ModelProviderNotDeclared`, `CommandAllowlistNotDeclared`,
`ToolAllowlistNotDeclared`, `EvaluationsNotDeclared`,
`ReadableContextNotDeclared`, `ReadableContextNotVerified { reason }`,
`CharterRevisionMoved`, `ProjectPolicyRevisionMoved`, `RepositoryRevisionMoved`,
`BaseRevisionMoved`, `ResidentLifecycleChanged`, `ConfigurationChanged`.

`ReadableContextNotVerified` carries one of six reasons, each decided by
inspecting the Repository revision the Proposal *names*, and each keeping a
different state apart as AGENTS.md requires — "Terminal, absent, broken, and not
evaluated are distinct states". They mirror the vocabulary Check Preflight
already uses rather than introducing a second framing for the same facts:

| reason | state | Check Preflight's equivalent |
|---|---|---|
| `RepositorySourceNotLocal` | a Git URL: nothing local, and no fetch is performed | `SourceKindUnsupported` |
| `RepositoryUnreadable` | broken: the path did not open as an object store | `RepositoryUnreadable` |
| `InspectionFailed` | errored: the store opened and an inspection step failed | `PreflightFailed` |
| `InspectionTimedOut` | not evaluated: the inspection did not finish inside its budget | `PreflightTimedOut` |
| `PinnedRevisionAbsent` | a negative answer: the store opened, the revision is not in it | `BaseRevisionUnreachable` |
| `ContentUnreadableAtRevision` | terminal: the store opened and holds the pinned revision, and content still cannot be read | — |

Broken, errored and not evaluated are three states, not two: a store that would
not open, an inspection that failed once it had, and one that never finished are
different facts about different things, and AGENTS.md requires them kept apart.

The budget behind `InspectionTimedOut` is **best-effort, not a hard bound**. It
is consulted on entry and between every object-store step, and is the same
deadline `ObjectStore::contains` consults, but a single call into that module can
still overrun it — `open` takes no deadline and reads a `.git` file whole, and a
pack index is read whole before that module's own first deadline check. Startup
also grants each declared-context Proposal its own budget rather than sharing one.
Those residuals are recorded in `docs/implementation/phase-2-proposal-v1.md`; the
ADR should not be read as promising a bound that is not enforced.

**A stated limit.** The "when the bound Repository is locally available …
compares every entry with that revision" branch of this ADR's readable-context
section is **not implemented**, and `ContentUnreadableAtRevision` is the terminal
state today. Resolving a path to its bytes at a revision requires decompressing
commit and tree objects; `crates/beacond/src/git_object_store.rs` is deliberately
presence-only, `beacond` declares no decompression dependency, and ADR 0027
forbids spawning `git`. No code path can therefore conclude that a declared entry
*agrees* with the pinned revision, and none claims to — which satisfies this
ADR's binding requirement that no surface label the context verified from the
manifest's self-consistency alone. There is deliberately no "mismatch" outcome,
because nothing can currently produce one. Completing that branch needs a
decompression dependency, which is a separate decision.

### What this amendment does not add

The reconciliation record-count and total-byte limits proposed in Athena BCN-40
(1,024 records / 16 MiB) are **not** written here, because they are not
implemented. Startup reconciliation walks every Proposal row with no record-count
or aggregate-byte ceiling; the only size enforcement is per record — the 64 KiB
envelope, mirrored in migration 0018 as
`CHECK (length(canonical_bytes) BETWEEN 19 AND 65536)`. Whether to add those
limits is an open question for the Product Steward, not a fact this amendment may
record.


## Amendment (accepted 2026-09-02): the readable-context comparison, as built

**Status: accepted 2026-09-02** — the Product Steward's ruling at the merge of
#45 (`020ea85`), recorded as Janus gate `gb40af242989`. The decision above remains `accepted`
as written; nothing here changes its bytes, its identity, or its authority
boundary. This section records what the readable-context comparison now does, so
the readiness vocabulary a consumer sees is described where the vocabulary is
defined. Kevin's merge is the acceptance; until then this is a proposal from the
implementing seat.

### What this proposes to change

**If the Steward accepts this at merge, it supersedes the "A stated limit"
section above.** That section is left exactly as accepted until then: an
implementing seat cannot pre-apply a decision it is asking for, and editing
accepted text while the amendment says "nothing here changes it" would make both
claims false at once. A non-author review caught that this draft had done exactly
that, and it is right — this is a public authority contract under GOVERNANCE.

What that section says is now out of date rather than wrong-when-written: it said
the comparison branch was not implemented because `beacond` had no decompressor
and `git_object_store.rs` was presence-only. `flate2` landed with the
Git-hardening work, and the object store gained a loose-object reader and a
commit → tree → blob path walk, so for a local repository whose objects are loose
the comparison now runs.

The comparison resolves each declared entry's path at the pinned base revision,
reads the blob, and compares **SHA-256 of the file's bytes** with the entry's
`content_sha256`. That is this ADR's own definition — "the SHA-256 identity of
the exact file bytes presented as context" — and deliberately not the Git object
name, which is a SHA-1 over a header and the same body. Comparing names would
fail every entry that is actually correct.

Every object on the path is proved to be the object its own name hashes to before
anything is concluded from it, and a commit or tree that authenticates but does
not parse as a commit or tree is refused rather than walked: authentication
proves the bytes match the name, not that they are the object that name is
supposed to hold.

### One new readiness item

`ReadableContextMismatch { entries }` joins the vocabulary. It is a fact about the
**Repository**: the comparison ran, and one or more declared entries do not hold
at the pinned revision. Each entry names its path and how it failed —
`DigestDiffers` or `PathAbsent` — and deliberately not the digest that was
actually found: knowing a declaration is wrong does not require publishing the
contents of a file the Operator may not have meant to surface in a diagnostic.

`ReadableContextNotVerified` keeps its meaning exactly: a fact about what **Beacon
could reach**. The two are never collapsed, and no failure path produces silence,
because silence is the only outcome that reads as agreement.

### Five new not-verified reasons

These five replace `ContentUnreadableAtRevision`, the single terminal state the
accepted "A stated limit" section named, and the list is closed: every way the
comparison declines to answer a declared entry is one of them. Naming three of the
five would leave a consumer to discover the other two from a diagnostic.

| reason | state |
|---|---|
| `PackedObjectUnreadable` | an object on the path is packed, and this reader reads only loose objects |
| `ObjectTooLargeToInspect` | one object on the path exceeds the comparison's own per-object byte bound |
| `PathTooDeepToInspect` | the declared path is deeper than the comparison's walk descends |
| `ContextBudgetExhausted` | the comparison's aggregate byte allowance was spent before every declared entry resolved |
| `EntryIsNotARegularFile` | the declared path resolves to a symlink, a gitlink, or a directory |

`PackedObjectUnreadable` is claimed **only after an authenticated pack lookup says
the object is present**. An object that an authenticated commit or tree names and
that lives in neither loose nor authenticated pack storage is `InspectionFailed`:
that store is broken, and calling it "packed" would excuse a defect as a
limitation of Beacon.

The three resource refusals — one object too large, the path too deep, and the
whole reading out of budget — are named separately from each other, because they
are different limits and a reader deciding what to change needs to know which one
was hit. Reporting an exhausted aggregate as `ObjectTooLargeToInspect` told an
Operator their six-byte file was too large, which sends them to shrink the wrong
thing.

`EntryIsNotARegularFile` is neither a resource refusal nor a mismatch. It is the
state this amendment deliberately leaves undecided below: Git stores a `120000`
symlink as a blob holding the link text, so the mode alone does not establish which
bytes the submitter presented, and calling that a false declaration would invent
contract. It is listed here because it is a reason an Operator can be shown, and a
closed vocabulary that omits one is not closed.

### Mismatches are published only from a completed comparison

A mismatch is an accusation about the Operator's declaration, and a walk that did
not finish has not earned one — the entry it could not reach might have explained
the one it did. So mismatches are buffered, and published only after **every**
declared entry has completed from a fully read, self-authenticated object. If one
entry disagrees and a later one is packed, oversized, too deep, malformed, or out
of budget, the buffer is discarded and the whole comparison is not-verified.

### What this amendment does not decide

- **Packed and delta objects are not read.** A repository that has been cloned or
  `git gc`-ed reports `PackedObjectUnreadable` for every entry, which may well be
  the common case. The comparison is still worth having — it makes this ADR's
  comparison branch true for a real class of repositories, namely the ones an
  Operator has been working in — but nobody should read it as "the comparison now
  runs". A pack reader is its own slice, with its own delta-format risks and caps.
- **A path resolving to a symlink, gitlink, or directory is not-verified, not a
  mismatch.** Git stores a `120000` symlink as a blob holding the link text, so
  the mode alone does not establish which bytes the submitter presented, and this
  ADR does not define the case. Treating it as a false declaration would invent
  contract. Deciding it either way is a public authority contract and therefore
  the Product Steward's, not the implementing seat's.
- **Content that differs only because Git normalized it.** `core.autocrlf`,
  `text=auto`, and clean/smudge filters mean a blob body can legitimately differ
  from the file the submitter hashed. Such an entry is reported as
  `DigestDiffers`, which is true and is not an accusation of misconduct. Whether
  Beacon should distinguish that case, and how, is a contract question this
  amendment deliberately leaves open.

### Required evidence

- a declared entry that holds at the pinned revision adds no readiness item;
- an entry whose digest differs is a mismatch naming that entry;
- an entry whose path is absent at the revision is a mismatch, not an inspection
  failure;
- a packed repository is not-verified, and the pack lookup is what earns the word
  "packed"; an object present in no storage at all is `InspectionFailed`;
- a mismatch found before an unreadable entry is discarded rather than published;
- a path resolving to a symlink is not-verified rather than mismatched;
- an object that does not hash to its own name, a commit that does not parse as a
  commit, and a tree naming one path twice are each refused before any result is
  drawn from them;
- the comparison stops at its deadline rather than answering, at every step;
- every bound the comparison owns is its own, not the object store's.

The mapping from these to their tests is recorded in
`docs/implementation/phase-2-proposal-v1.md`.
