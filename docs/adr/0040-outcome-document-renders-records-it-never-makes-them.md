---
status: accepted
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

**Amended 2026-09-07** (status unchanged: accepted). Four statements described `main` as it
stood on 2026-09-05 and had since become false: the six prerequisite records now exist, the
collector and candidate Artifact and publication relation are built, the front-matter
candidate key is decided rather than pending, and the scope paragraph named as absent things
that had landed. Nothing about the architecture changed; the decision text is untouched. What
changed is the description of what `main` holds, re-measured at `4baca12`, with a test named
for each claim. The one genuinely open item moved to a new `## Open` section.

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

### Narrative text carries line breaks; a label does not

Human text divides into two classes, and the division is by rendered position rather than by
field name.

**Narrative text** -- success-condition statements, unfinished-work items, uncertainty items --
is prose a reader is meant to read, and it renders as CommonMark blocks. LF is structure and is
preserved: a blank line is a paragraph break and a run of them is kept one for one, and every
other newline is a hard line break written as a trailing backslash, because the byte rules forbid
the two trailing spaces CommonMark's other hard break uses. TAB is content and is preserved,
wherever it sits; a leading TAB may render as indentation, which is a rendering effect of a value
Beacon was told to keep, not an alteration of it. Every line passes through the versioned escaping
function first, so no stored value becomes structure.

The whitespace rule is one property applied per line: **anything the renderer would collapse is
refused, so a stored value and its document correspond one to one.** A line may not begin with a
space, because CommonMark strips it. A line may not end with whitespace of any kind, because the
byte rules forbid trailing whitespace. The first and last lines may not be blank, because a leading
or trailing newline renders identically to its absence. A leading TAB is none of those things -- it
survives into the emitted bytes -- so it is accepted, and refusing it would contradict the sentence
above. Every one of those refusals is `untrimmed_text`, and the name covers whitespace the document
cannot carry rather than whitespace `trim()` would remove.

For that to work the prose has to sit in a block position, so a condition's statement is a
paragraph under its `### 2.N` heading rather than a bullet, and each narrative item gets its own
`### 6.N` / `### 7.N` heading. Prose in a bullet cannot carry a paragraph break without ending the
bullet.

**Label text** -- the Mission declaration title, an Artifact title, and every honesty-state
reason -- renders inside a table cell, where a line break ends the row. Labels refuse one. The two
alternatives are raw HTML, which is forbidden, and replacing it with a space, which silently
alters a stored value; refusing is the only honest third option, and it is named
`line_break_in_label` so a caller knows which rule it broke.

**Every text refusal names itself.** Empty, padded, too long, carrying a control character, and
carrying a line break in a label are five different faults: `empty_text`, `untrimmed_text`,
`text_too_long`, `control_character`, `line_break_in_label`. One name for all five tells a caller
nothing about which it hit, and a refusal a caller cannot act on is a refusal that gets retried.

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

### The Policy bit is copied and re-checked, and it is a boundary claim

R13.7, stated exactly because slice 2 implements this seam and a sentence left out of an ADR is a
seam nobody builds. At `prepare`:

1. the owning Policy reader returns the classification through its verified read -- stored bytes,
   recomputed digest, Workspace scope, strict decode, exact re-encode;
2. `prepare` writes **both** the cited `SourceRef` (identity, version, digest) **and** the derived
   `policy_high_impact` bool into the manifest;
3. `prepare` **re-checks** the copied bool against that exact record and refuses if the two
   disagree.

Copying is ADR 0037's argument for copying a Proposal's bindings into a Ruling: a copy makes later
drift detectable without trusting the source row to have stayed put. Re-checking is what stops the
copy from being a caller's assertion.

**The manifest therefore asserts what Policy said at the boundary event, not what Policy says now.**
Every surface that shows it says so, which is what the appendix's `verified at boundary event N;
not re-verified at read time` label is for. A Policy re-classified after publication is visible
through `get()`'s read-time tail beside the frozen bytes, never by editing them.

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

**Correction (R30.2, slice 4b).** "with the profile named in front matter" is superseded: the
profile is the renderer's, not the document's, and naming it in front matter would change every
byte of every document and require a new `renderer_version`. The profile names itself in the
PDF's `Creator`.

## The rest of the design contract this ADR carries

gpt2's specification review found ten invariants the first draft of this ADR compressed away. They
bind slices 2 to 4, and an invariant that lives only in a design file is one nobody implements, so
they are recorded here in their own words.

1. **Publication qualification is exact.** `publish` requires the candidate Artifact's exact
   digest, an Artifact-subject Evaluation of *that* digest reading `meets`, an evaluator who is not
   the accountable author, and an external immutable publication relation
   `(candidate_artifact_digest, evaluation_digest, publication_version)`.
2. **Corrections are new versions.** A correction creates a new candidate and a new publication
   version that supersedes the old one. Nothing is edited, deleted, or re-rendered in place.
3. **The six prerequisite records keep their full ownership.** `MissionCompletionV1` owns the
   finishing cursor, accountable Resident, per-condition outcomes, delivered Artifact references,
   unfinished work, uncertainty and the spend reference; the Artifact entity owns immutable bytes,
   digest, media type, accountable authorship and supersession; measured spend owns amount, unit,
   coverage boundary and verification state. Outcome refuses an invalid one; it never repairs one.
4. **The verified-read discipline is four rules, not one.** One read transaction at the recorded
   boundary; relations and joins may identify candidate IDs only and may never supply a renderable
   claim; every candidate is re-read through its owning module before it is a typed value; and a
   decoy-projection test must lie in every joined column while the owned object stays correct, with
   the rendered Markdown following the object or refusing.
5. **The front-matter field list is authoritative.** Manifest identity and digest, render profile,
   boundary event sequence, accountable Resident, Workspace, Mission, completion, and appendix
   policy. The candidate Artifact is deliberately absent (R13.1) and adding it is a new
   `renderer_version`.
6. **The appendix's per-object contract is every column, not a summary.** Typed identity, schema
   version, digest algorithm, digest, owning canonicalizer, pinned revision or an explicit `absent`
   with its reason, and the verification result; plus Receipt checksums labelled as Outcome
   checksums, candidate media type and renderer version, manifest digest and boundary sequence,
   Policy identity/digest and the rule that selected the mode, and the PDF profile when one exists.
7. **Only verified stored allowlisted fields become positive prose.** The renderer never synthesizes
   a result from counts, Activity, Trace, filenames, current branch state, or missing rows.
8. **A missing qualifying Evaluation blocks publication and preserves the candidate.** A
   non-qualifying candidate stays an Artifact; it is never deleted and never presented as the
   outcome.
9. **Owning readers refuse their own unknown fields, tags, states and versions** before a value
   crosses the interface. The renderer has no unknown-field tolerance and is never a second, laxer
   decoder.
10. **The Policy bit is copied and re-checked** — the section above.

## What slice 1 did not do, and what has since been built

Slice 1 shipped the pure model and the Markdown renderer only: no database, no migration, no
Activity, no PDF, no UI, no collector, no candidate Artifact, no publication relation.

Every one of those except Activity has since landed, and this section is kept rather than deleted
because the boundary it drew is the reason each piece could be built separately.

| slice-1 exclusion | state at `4baca12` | evidence |
|---|---|---|
| database + migrations | built | `0029_outcome_manifests.sql`, `0030_outcome_manifest_canonicalizer_v2.sql`, `0031_outcome_publications.sql` |
| collector | built | `crates/beacond/src/outcome_collector.rs`. Each source it collects is read through the module that owns it rather than joined: proved for Proposals, Rulings and Evaluations on the negative side and for completion, classification and spend on the positive side by `outcome_collector :: collector_reads_each_source_through_its_owning_verified_reader`; for a delivered Artifact by `outcome_collector :: a_delivered_artifact_scoped_to_another_mission_is_refused`; and against a completion whose own word disagrees with the owned record by `outcome_collector :: a_lie_in_the_completions_support_is_refused_while_the_owned_record_is_correct`, which covers a Receipt checksum disagreement, a Receipt version disagreement and an absent Evaluation |
| candidate Artifact | built | `outcome_prepare :: a_completed_mission_becomes_a_candidate_outcome`, `:: candidate_artifact_digest_covers_exact_markdown_bytes`, `:: a_manifest_cannot_exist_without_its_candidate` |
| publication relation | built | `outcome_prepare :: a_published_outcome_is_never_edited_or_deleted_in_place`, `:: a_mission_with_no_publication_exports_as_absent` |
| PDF | built | `crates/beacond/src/outcome_typst.rs`, `outcome_pdf.rs` |
| desktop Outcome surface | built, read-only | `apps/desktop/src/MissionOutcomes.tsx`, mounted at `apps/desktop/src/ProjectMissions.tsx:169`: it lists every published version of a Mission's Outcome and shows the selected one's **exact stored bytes**, decoded and never rendered to HTML, "because doing it here would put a renderer between an operator and bytes whose whole point is that they are the exact ones that were published and evaluated" (`MissionOutcomes.tsx:40-44`) |
| Activity | **still out of scope** | no module; unchanged by this amendment |

The desktop surface reads and nothing else: `loadMissionPublications` and `loadOutcomeDocument` are
its only two calls, and it asks for the **selected version** every time rather than "the newest",
because a superseded version stays readable at its own number and a view that quietly showed the
current document for every row would show bytes no Evaluation of that version was ever about
(R12.11, `MissionOutcomes.tsx:76-78`). What is still out of scope on that surface: publishing,
withdrawing, and PDF export are service operations with no control in this view.

The canonicalizer has also been through one full version transition since slice 1 —
`beacon.outcome.manifest.v1` retired and refused by name, its frozen vector slot emptied, and
`v2` frozen in its place. That is the versioning discipline this ADR describes, exercised
once, and it is why a later renderer or canonicalizer change is a cost rather than a barrier.

## Consequences

- A Mission cannot get an Outcome Document until six records exist. That is the cost of not
  inferring them, and it is the point. **All six now exist**, each read through its owning
  verified reader rather than joined into the document:
  `MissionCompletionV1` (`mission_completion.rs`, migration `0026`, `mission_completion ::
  the_canonical_vector_is_exactly_these_bytes_and_a_second_encoder_reproduces_them`);
  the Artifact entity (ADR 0043, `artifact.rs`, migration `0027`, `artifact ::
  a_known_artifact_fixture_reproduces_its_published_digest`);
  the Artifact-digest Evaluation subject arm (migration `0028`, `evaluation ::
  a_valid_artifact_subject_evaluation_survives_restart`);
  structured per-condition results (ADR 0041, and **four** arms rather than slice 1's two —
  `outcome :: owner_condition_outcomes_map_without_collapsing_inconclusive_into_not_evaluated`);
  the measured spend record (ADR 0042, `spend.rs`, migration `0025`, `outcome_prepare ::
  a_spend_measured_after_the_last_run_reaches_the_manifest_as_measured`);
  and the durable Policy classification (ADR 0044, `mission_impact.rs`, migrations `0025` and
  `0032`, `outcome_prepare :: prepare_for_mission_cites_the_newest_classification_at_or_before_the_boundary`).
- Every array in a manifest is sorted and strictly ordered on the wire, so a store that
  paginates by a different collation cannot produce a second spelling of one document.
- Prose is escaped conservatively enough to be safe and loosely enough to stay readable;
  the exact set is versioned, so widening it later is a new renderer version and a new
  document, not a silent change to old ones.
- Narrative text carries line breaks; label text does not. A completion record whose Artifact
  title or state reason spans lines is refused rather than flattened, and moving one of those
  fields out of a table cell is a renderer version change.
- The appendix embeds the manifest twice its byte length in hexadecimal. For a large
  Mission that is the largest part of the document, and it is what makes the document
  self-verifying.
- The front matter does **not** name the candidate Artifact, because a candidate's
  identity is its content digest and a document cannot contain its own digest (R13.1, and
  the field list above says the same). **Adding the key later changes every byte of every
  document, so it is a renderer version change** — that constraint is unchanged and is why
  this was decided rather than deferred.

  It is decided. The Artifact entity landed with a content identity and no surrogate UUID
  (ADR 0043), so the horn where a candidate gets a non-digest identity is closed. The binding
  runs the other way instead: the store holds the candidate's digest and proves it by
  requiring **that Artifact to embed the manifest's exact canonical bytes**, re-checked on
  every read and not only at startup (`outcome_prepare ::
  candidate_artifact_digest_covers_exact_markdown_bytes`, `:: astra_candidate_projection_must_be_verified_on_read_and_retry`).
  The manifest's reference identity is derived from its own content digest rather than minted,
  so the two cannot disagree (R18.2).

  How a reader gets from the document to its Artifact is therefore the store, not the front matter,
  and that is the honest statement of it. The candidate's record identity is **not** derivable from
  the document: an `ArtifactV1` hashes its `recorded_at_ms`, which `prepare` takes from the service
  clock (`outcome_store.rs`, `service_clock` at prepare; `artifact.rs`, `recorded_at_ms` inside the
  encoding), and changing that field alone changes the identity (`artifact ::
  each_identity_bearing_field_changes_the_digest`). What is idempotent is **retrieval within one
  store**: repeating `prepare` for the same manifest and renderer version returns the candidate
  already prepared instead of recording a second one (`outcome_prepare ::
  prepare_is_idempotent_for_same_manifest_and_renderer_version`). So the key is not merely
  redundant — it is impossible, for the reason above, and the reverse binding is what replaces it.

## Open

One item, and it is a shape call rather than a defect. Everything else this ADR once listed as
pending has been built or decided; see the amendment note above.

**Does a state reason carry line breaks?** The consequence above splits narrative text (line
breaks allowed) from label text (refused as `line_break_in_label`), and every honesty-state
reason currently renders inside a table cell, so a reason containing a line break is refused
rather than flattened. R13.5 said narrative text carries line breaks; whether it meant reasons
too was never settled. The two options:

- **Keep reasons as labels** — a completion whose state reason spans lines is refused; the
  refusal is named and the document shape is unchanged. No version change, and some real
  completion records will be unrenderable.
- **Move reasons out of their table cells** — reasons become narrative and carry line breaks;
  the fixed Markdown shape changes, which is a new `renderer_version`, and every document
  rendered under v1 stays at v1.

Kevin's call. Nothing in this amendment presumes either.
