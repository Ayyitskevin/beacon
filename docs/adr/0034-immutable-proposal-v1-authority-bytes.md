---
status: proposed
---

# Bind inert Proposal v1 to the exact bytes Beacon stores

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

## Product Steward ruling required

Acceptance of this exact record decides that:

1. Proposal v1 uses the fixed binary canonicalizer above;
2. readable context is mandatory and digest-bound, with `not_declared` distinct
   from `declared([])` and always non-dispatchable;
3. unavailable constraint families remain explicit and non-dispatchable rather
   than receiving guessed defaults;
4. fresh Residents use one Proposal-scoped Owner bootstrap reason rather than
   fabricated Track Record evidence or an Operator UUID;
5. identical submission is idempotent; and
6. implementation remains backend-only and inert.

No Proposal persistence implementation or merge is authorized until the Product
Steward accepts or amends this candidate.
