---
status: proposed
---

# The Outcome Document renders records; it never makes one

BLUEPRINT requires that "every completed Mission publishes a human-readable Outcome
Document" summarising the result "against success conditions, delivered Artifacts,
Evidence, Evaluations, actual spend, unfinished work, uncertainty, and linked Receipts"
(`docs/BLUEPRINT.md:180-185`), that it stay readable "without opening Activity or raw
Trace" (`:1201-1202`), and that a PDF regenerated from its canonical Markdown "cannot
create a separate or divergent source record" (`:1203-1205`).

The cheapest way to satisfy that sentence is a prose template over a query. It is also
the way that makes Beacon lie: a renderer with no frozen inputs has to infer completion,
authorship, spend, and per-condition results, and a document that infers them is polished
and false.

This ADR records the architecture Kevin ruled on 2026-09-02 (§6 items 1-9) and
2026-09-03 (items 10-12) from the Outcome Document v1 design, and the exact shape slice 1
implements.

## Decision

An Outcome Document is an immutable, content-addressed Artifact containing deterministic
open Markdown, rendered from one immutable, digest-identified `OutcomeManifestV1` whose
references were each fetched through the owning module's verified reader.

Beacon adds an `outcome` module rather than putting rendering inside Mission, Receipt,
Proposal, Ruling, or Evaluation. Its narrow public surface is conceptually:

```text
prepare(mission_id, appendix_request) -> OutcomeCandidate
publish(mission_id, candidate_artifact_digest, evaluation_digest) -> PublishedOutcome
get(mission_id) -> PublishedOutcome
```

Slice 1 implements none of those three. It implements what they all stand on: the
manifest type, its canonical bytes, the four honesty states, the three bounds, and one
pure renderer.

### It creates no authority

A published Outcome is a document with an immutable history. Its existence is not
permission to proceed, not Mission completion, and not a success verdict. **No module or
surface may branch on `PublishedOutcome` existence to derive any of those predicates.**

This is stated as a rule the code can be checked against, because the design's own test
name -- `publish_does_not_change_mission_completion_or_create_authority` -- names a
property nothing had defined, and a test whose property is undefined cannot fail. The
Proposal module already carries the same discipline in one line: there is deliberately no
`is_ready`, because readiness is not permission to proceed (ADR 0034). The equivalent
here is that there is deliberately no `is_successful` and no `is_complete`.

### Publishing is not completing

Work/Mission records completion first, in `MissionCompletionV1`; the Outcome cites that
exact record. Coupling the two would create a cycle -- the Outcome needs completion facts
while completion waits for the Outcome -- and would hand a renderer authority over Work
state.

### Six hard prerequisites gate the publication path, not slice 1

`MissionCompletionV1`; a general Artifact entity; an Evaluation subject arm for an exact
Artifact digest; structured per-condition results; a measured actual-spend record; and a
durable Policy high-impact classification. None exists on `main` today. Slice 1 models
their shapes and constructs their values; slice 2 may not begin until they exist.

## The manifest is identities and digests; the values arrive separately

Two structures, not one.

`OutcomeManifestV1` carries Workspace scope, typed object identities, schema versions,
canonical digests, and honesty states. `VerifiedSources` carries the text and metadata
each owning module's verified reader returned at the manifest's boundary event. The
renderer takes both and refuses when they do not correspond exactly.

The split is what makes the read discipline enforceable. Relation tables and joins may
identify candidate IDs only; they may never supply a renderable claim. If the manifest
carried another module's text, a denormalized column could become a durable claim and
slice 2's decoy-projection test would have nothing to attack.

### Every collection is a state

The design's manifest listing writes `delivered_artifacts[]`. Slice 1 stores
`OutcomeState<Vec<SourceRef>>` instead, and the same for Proposals, Rulings, Evaluations,
Receipts, unfinished work, and uncertainty.

"The completion listed no delivered Artifacts" and "no Artifact record exists at this
boundary" are different claims. An empty vector can only say the first if something says
so explicitly; deriving the second from it is the renderer synthesizing a result from a
missing row, which §5 rule 2 forbids. A `verified` collection with zero entries renders
one fixed sentence that says it is not `absent`.

`success_conditions` is the one collection that is **not** a state. A Mission's declared
success conditions are the document's subject rather than evidence about it, and
BLUEPRINT requires the Outcome be linked to them, so a manifest without them is refused
rather than rendered empty.

### Success-condition completeness belongs to completion

`MissionCompletionV1`'s verified reader validates that every declared success condition
appears exactly once. Outcome **refuses** a completion that fails that validation; it
does not reimplement the rule, and it does not enforce ordinal uniqueness of its own.
Two consumers of one invariant drift. The canonical order of conditions is therefore
`(ordinal, condition_id)`, which is well defined whether or not ordinals repeat.

## Exact canonical bytes

Manifest canonicalization follows the ADR 0034/0037 discipline, unchanged: versioned
magic (`beacon.outcome.manifest.v1\0`), positional fixed-width fields, closed one-byte
tags that are never renumbered, explicitly length-prefixed UTF-8, and strict decoding
that refuses unknown tags, over-limit or invalid lengths, trailing bytes, invalid UTF-8,
non-canonical text, and non-canonical array order. Decode, re-encode, and the bytes must
be equal.

`canonicalizer_version` selects one fully specified profile and is itself inside the
hashed bytes. Any future grammar is a new canonicalizer; v1 bytes are never reinterpreted
under v2 rules.

Arrays are sorted **before the bytes exist**, so two insertion orders of one manifest
produce one identity. Decoding refuses input that is not already in that order, and
refuses a repeated key: two spellings of one identity must not both decode.

Every reference must name the manifest's Workspace. A cross-Workspace reference, a digest
mismatch, an unknown schema, or a corrupt envelope **aborts** the operation. Those are
integrity failures and must not be normalized into an honest domain-level `broken` row.

### One comparator, code point, never a collation

Every textual sort key is already in its canonical lowercase form -- a non-canonical key
is refused rather than folded -- and keys compare by Unicode code point, which for UTF-8
is unsigned byte order. Never `localeCompare`, never a database collation. A renderer
that sorts one way while a store paginates another produces a different document from the
same manifest, and the two disagree exactly where punctuation appears: an ICU primary
strength comparison puts `ab` before `a-z`, and this rule puts `a-z` first.

The rule has one executable form, `compare_canonical_keys`, so it cannot drift from its
prose.

## Determinism and the fixed Markdown shape

For the same manifest and the same verified sources, independent implementations must
emit byte-identical Markdown: UTF-8 without BOM, LF, no trailing spaces, exactly one final
newline, fixed front-matter key order, fixed section order, no optional empty-section
omission, lowercase canonical UUIDs and digests, stored UTC timestamps rendered RFC 3339
with `Z`, and money rendered from stored integer minor units through integer arithmetic.
No clock, timezone, locale, filesystem, hash-map order, or current state can influence a
byte.

The sections are fixed: identity, success conditions, delivered Artifacts, Evaluations
and cited support, actual spend and coverage, unfinished work, uncertainty, Rulings and
Proposal lineage, linked Receipts, verification appendix.

Links are stable textual identity triples and internal Markdown anchors only. v1 invents
no application URL scheme; the deep-link authority decision is still Kevin's, and an
immutable document is the worst place to preempt it.

### `renderer_version` owns the escaping function

An unknown renderer version is refused, never silently escaped by whatever function is
newest. The v1 function is three rules, each a function of the string alone:

1. `\` `` ` `` `*` `_` `[` `]` `<` `>` `&` `|` `~` `!` are escaped wherever they appear;
2. a leading `#` `+` `-` `=` `.` `)` is escaped;
3. a leading run of digits followed by `.` or `)` has that punctuation escaped.

Raw HTML never survives, because `<` and `>` are in rule 1. Table cells survive, because
`|` is. The function is deliberately not context-dependent: a function whose output
depended on where its result landed would change bytes when a section moved, and
`renderer_version` could not honestly name it.

Only human text is escaped. State literals, kind tokens, UUIDs, and digests are
renderer-owned values from closed vocabularies and are emitted verbatim inside code
spans, so `not_evaluated` reaches the document as `not_evaluated` rather than as
`not\_evaluated`. Values that must sit inside a code span but come from another module --
media types, currency and unit tokens -- are **validated** against a charset that excludes
backtick and `|` rather than escaped, because a backslash inside a code span is literal.

### v1 refuses multi-line human text

The fixed shape puts human text in table cells and list items, where a line break changes
the structure. Neutralizing one would need raw HTML, which is forbidden; replacing it with
a space would silently alter a stored value. So v1 refuses it at the boundary and records
the limitation here. A shape that carries multi-line narrative is a renderer version
change.

## The four honesty states

Every condition, referenced fact, and optional source is exactly one of `verified(value)`,
`absent(reason)`, `broken(reason)`, `not_evaluated(reason)`. They stay literal in
Markdown and are never converted to blanks, em dashes, `N/A`, success, failure, `$0`, or
each other.

- A missing spend measurement renders its state and an explicit sentence that it is not
  zero. The document states no amount.
- A success condition without one of the four states aborts.
- Beacon-store corruption is not a domain `broken`. It aborts.

A `verified(value)` in a published document is a claim about the manifest's boundary
event, not about read time, and every surface that shows one says so. `get()`
re-verifies the manifest bytes and digest, the candidate Artifact's bytes and digest, and
the qualifying Evaluation's identity, digest, subject, and publication relation -- and
nothing else. The appendix labels each row `verified at boundary event N`.

## Bounds are the function's, and each names itself

At most 256 cited objects, 16 MiB of verified source bytes read, and 1 MiB of emitted
Markdown. Caller-owned limits would make identical inputs produce different documents; no
limits permit resource exhaustion; silent truncation makes a completeness claim false.

An exceeded bound refuses the whole render and names the exact bound that bit --
`citation_count`, `verified_source_bytes`, or `emitted_markdown_bytes` -- with its
configured value and an **observed-at-least** count. The count is a lower bound on
purpose: the walk stops at the moment the bound is exceeded rather than measuring the
whole input, so the honest figure is "at least this many" and never a total nobody
finished counting. An exhausted total budget that reported itself as some other bound is
a defect this fleet has already paid for once.

Each bound is separately testable and distinguishable from the other two **and from its
own absence**, because deleting one silently is the failure mode. The Markdown buffer is
owned by the renderer and dropped on refusal: there is no path that returns bytes and a
refusal together.

Citation counting is by distinct object -- typed identity, schema version, and digest --
so one object cited twice is one cited object, and the appendix's per-object rows are
deduplicated by the same key so the census and the table cannot disagree.

## The verification appendix, and the manifest proof in every mode

A verified Policy classification decides whether the appendix is mandatory. The API
accepts `policy_default` or `include` and has **no** `omit` arm; high-impact always
resolves to include. The resolved mode is derived from the request and the classification
rather than stored, because a stored resolution can disagree with its own inputs. The
document prints the rule that selected it.

Every appendix mode -- including the literal non-high-impact policy state -- embeds a
manifest-proof block: the exact canonical manifest bytes as lowercase hexadecimal, with
byte length, canonicalizer, digest algorithm, and digest. Without it the one identity the
whole document hangs from could only be trusted, never recomputed, and BLUEPRINT's
cold-reader requirement could not be run: a reader would have every cited object's digest
and not the manifest's. The stored manifest bytes are retained for the lifetime of every
published Outcome version that cites them.

The appendix never reproduces raw Trace, secret-bearing payloads, private Evidence bytes,
or prose claims not already in the core document.

## Receipts are cited as Receipts, not given a new identity

A Receipt is cited by `(Workspace, run UUID, Receipt version)` plus a SHA-256 over its
exact stored immutable `receipt_json`. That checksum is an **Outcome verification
checksum**, not a retroactive Receipt content digest: ADR 0027 and Receipt v2 define none,
and minting one here would contradict them and force an identity migration later. The
label `beacon.outcome.receipt-checksum.v1` travels inside the canonical bytes, where no
later reader can drop it.

## Evidence is not cited in v1

The design's first draft carried an `Evidence` allowlist row and `evidence_refs[]`. There
is no first-class Evidence entity in Beacon: what exists is a bounded Query over Runs and
Receipts, with no identity, no version, no digest, and no payload crossing it. A manifest
reference to it would send a builder looking for a verified reader nobody wrote. Evidence
enters the allowlist when it has an identity, a version, a digest, a payload owner, and a
verified reader -- and that is a seventh hard prerequisite, not a slice.

## Supersession, withdrawal, and who may read

- **Supersession (Kevin, item 10).** The canonical bytes stay frozen on the exact cited
  digests. `get()` returns those immutable bytes and, beside them rather than inside or
  instead of them, the currently verified supersession tail for every cited Evaluation and
  Ruling, labelled as read-time state. Never silently the tail; never a mutated document.
  This matches Evaluation v1's settled both-links rule.
- **Withdrawal (Kevin, item 11).** Deferred from v1. `OutcomeWithdrawalV1` is **reserved
  by name here and not built**: no deletion, no in-place alteration, ever. When Kevin
  rules it, it is an external append-only relation naming the exact publication version,
  actor, reason, event boundary, and supporting references.
- **Access (Kevin, item 12).** Retrieval inherits the cited Mission's existing Workspace
  access checks. Export is an explicit authenticated Operator action under the same
  scope. Publication is never public or cross-Installation visibility, and the appendix
  does not bypass source access merely because it carries digests rather than payloads.
  Any sharing or redaction model is Kevin's.

## Export and PDF

The canonical export is a byte-for-byte copy of the stored candidate Artifact's Markdown
bytes. Export never reruns a query or a renderer. `pdf_bytes = render_pdf(exact_markdown_bytes,
pinned_profile)`, with the profile named in front matter, embedded local assets, and no
network, clock, host path, locale, or random identifier. A PDF is a deletable derivative;
it is never a source of truth. Slice 4 builds this; slice 1 asserts only that the
document declares no PDF profile.

## What slice 1 deliberately does not do

No database, no migration, no Activity, no PDF, no UI, no collector, no candidate
Artifact, no publication relation. Migrations take their final sequence numbers only
after rebasing on current main, and arrive with slice 2.

## Consequences

- A Mission cannot get an Outcome Document until six records exist that `main` does not
  have. That is the cost of not inferring them, and it is the point.
- Every array in a manifest is sorted and strictly ordered on the wire, so a store that
  paginates by a different collation cannot produce a second spelling of one document.
- Prose is escaped conservatively enough to be safe and loosely enough to stay readable;
  the exact set is versioned, so widening it later is a new renderer version and a new
  document, not a silent change to old ones.
- Multi-line narrative text is refused in v1. Real completion records may carry it, and
  the first one that does is a renderer version change rather than a bug.
- The appendix embeds the manifest twice its byte length in hexadecimal. For a large
  Mission that is the largest part of the document, and it is what makes the document
  self-verifying.
- The front matter does **not** name the candidate Artifact, because a candidate's
  identity is its content digest and a document cannot contain its own digest. Naming it
  needs the Artifact entity and an idempotent identity rule; adding the key later changes
  every byte of every document, so it is a renderer version change and is called out here
  rather than discovered in slice 3.
