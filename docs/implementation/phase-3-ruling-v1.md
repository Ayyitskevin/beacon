# Phase 3 Ruling v1

This checkpoint records that a human decided about one exact Proposal. A Ruling binds one Proposal
digest, copies the Charter, Project Policy, and Repository revisions and the pinned base revision it
was decided under, and is itself identified by the SHA-256 of its own canonical bytes. It implements
[ADR 0037](../adr/0037-ruling-bound-to-proposal-digest.md) within that decision's fence.

**It creates no execution authority.** A Ruling is evidence that a person decided, and evidence does
not confer authority (`docs/BLUEPRINT.md:445-447`). Nothing here opens a Gate, prepares a worktree,
starts a process, invokes a model, makes a network request, or dispatches anything. No accessor
answers a question with a boolean. A future dispatch re-verifies the binding itself — that the digest
still names those bytes, that every copied revision still matches, that readiness reports no unmet
precondition — and refuses otherwise. Reading a Ruling is never a permission check.

It is numbered Phase 3 because it is the first slice that binds to the Phase 2 Proposal.

## One interface

```text
rule(proposal_digest, decision, reason, supersedes) -> Ruling
get(ruling_digest) -> Ruling
list_for_proposal(proposal_digest) -> [Ruling]      // chain order, oldest first
```

`rule` authenticates the Owner before anything else happens, so an unauthenticated caller receives
the account refusal whatever else is wrong with the request. It then checks the caller's own values
with the predicates the encoder uses — so a malformed argument never reaches the store, and because
both call the same predicate they cannot disagree about what is well-formed.

**The command takes typed arguments rather than a byte slice.** This is the one place Ruling v1 does
not copy Proposal v1, and the reason is the difference between the two records. A Proposal's bytes
are the caller's — untrusted, and the whole point is that a parsed object and a digest can never
disagree. A Ruling's bytes are Beacon's own, produced from an authenticated Owner command over
values Beacon already holds, so accepting them from a caller would be an input surface with nothing
to gain. Beacon canonicalises, hashes, and stores; every read re-derives the digest from the stored
bytes.

`Ruling` keeps private fields and derives no `Deserialize`, for the reason ADR 0034 gives: a
caller-supplied parsed object plus digest permits the two to disagree.

## The canonical bytes

The canonicalizer identity is `beacon.ruling.v1`. The grammar reuses `beacon.proposal.v1`'s
primitives exactly and its complete field table, widths, and tag values are in ADR 0037. The magic
is the literal ASCII `beacon.ruling.v1\0`, **17 bytes** — not the 19 the design claimed, which it
had inherited from the Proposal magic. A codec built to that number would have read every field two
bytes late, and it is the kind of error that mints permanent valid-looking identities.

`crates/beacond/src/ruling.rs` carries its own copy of the encoder and decoder primitives rather
than sharing one implementation with `proposal.rs`. That is deliberate: an identity grammar's whole
value is that it cannot move, and a shared mutable codec would couple two frozen grammars' futures,
so a change made for one could silently restate the other's identities. The primitives are small;
the coupling would not be.

### What "reordering is refused" can and cannot mean

The grammar is positional and untagged, so the claim has to be split, and ADR 0037 now splits it.

A reordering the decoder can **see** is refused: two adjacent length-prefixed strings swapped, a tag
where a length belongs, an unknown tag, a trailing byte, invalid UTF-8, an over-limit length.

A reordering of two **same-width** values is not detectable by construction. A record whose
`charter_revision` and `project_policy_revision` are swapped is byte-for-byte a legitimate record
whose revisions are those numbers, and no predicate can separate the two without knowing an intent
the bytes do not carry. What holds instead is that such a swap **changes the identity**. Field order
is fixed by the ADR's table and pinned by the identity vector below; it is not, and is not claimed
to be, enforced by a runtime check.

This correction came from gpt2's non-author review, which found the original test asserting only
that the bytes differ under a name that promised a refusal.

### The published identity vector

`crates/beacond/src/ruling.rs` pins one fabricated Ruling by its bytes and by its digest, as two
separate claims. The record is **234 bytes**:

```text
626561636f6e2e72756c696e672e7631003b1f8c0e5d4a2b697f8e1d0c3a5b7e9d2f4a6c8e0b1d3f5a7c9e1b3d5f7a9c
1e00000000000000067368613235360000000000000012626561636f6e2e70726f706f73616c2e763100000001000000
020000000300000000000000286131623263336434653566366131623263336434653566366131623263336434653566
3661316232010000000000000042546865206465636c617265642065666665637473207374617920696e736964652074
6865205265706f7369746f727920616e64207370656e64206e6f7468696e672e0100000001990970a000
```

Its identity is:

```text
8c9c3367fff1e77524014506a9bf48d0786d105b9011ccc373b79af0ae803b3d
```

Reproduce it with no Beacon code at all:

```sh
printf '%s' '<the hex above, whitespace removed, on one line>' | xxd -r -p | sha256sum
```

`sha256sum` and `openssl dgst -sha256` were both run against those bytes and agree. The
authoritative copy of this vector, without line wrapping, is ADR 0037 and the `FIXTURE_BYTES_HEX`
constant in `crates/beacond/src/ruling.rs`.

The vector was **constructed independently in Python by a non-author reviewer, from the ADR's table
alone, and found byte-identical to this encoder's output** — checked before it was pinned, not
asserted after. Two separate tests, deliberately: one compares `encode` against the literal bytes,
the other is SHA-256 over those literals and would still hold if the encoder were replaced
wholesale. Splitting them is what makes the digest evidence rather than a restatement, and the
split is visible: reordering the encoder's two envelope strings turns the first red and leaves the
second green.

The earlier version of that test rebuilt its expected buffer in Rust and then compared digests,
which is tautological once the byte comparison passes — an encoder and its hand-built expectation
can drift together, and nothing outside the file would notice.

## Two digest envelopes, both explicit

A Ruling carries two identities and neither is inferred from the other.

The **bound Proposal's** envelope — value, algorithm, and canonicalizer — is **copied from the
verified Proposal**, not rebuilt from this module's constants. The store's CHECK constraints make
those constants equal to what the Proposal holds, so a reconstruction produced correct bytes; it
produced them by coincidence, and equality is not provenance. `proposal_binding_facts` returns the
verified Proposal's own envelope and `rule` copies it whole.

The **Ruling's own** envelope is `sha256` over its canonical bytes under `beacon.ruling.v1`.

## The chain

The effective Ruling for a Proposal is the last link of its chain, which is only an answer if the
graph cannot fork:

- the **first** Ruling for a Proposal has no predecessor;
- every **later** Ruling must cite the unique current tail, and citing any other Ruling is refused as
  `NotTheCurrentTail`. Absent, cross-Proposal, and non-tail citations get three distinct refusals,
  because they are three different mistakes leading to three different corrections;
- the tail is read, compared, and extended inside **one immediate transaction**, so two concurrent
  rulings cannot both believe they extend it;
- `ruled_at_ms` is non-decreasing along the chain, equal allowed. The effective order is **chain
  order, never timestamp order**: a timestamp is evidence about when, and the chain is the record of
  what followed what.

`ruling_chain` is the only thing in the crate that knows what a chain is — recording asks it for the
tail, `list_for_proposal` for the order, reconciliation for both. It refuses a fork and a cycle,
which the store's own constraints do **not** cover: the primary key forbids citing two predecessors
and the unique index forbids being superseded twice, and neither forbids two independent chains
under one Proposal.

**A cycle that agrees with itself is infeasible, not merely unrecorded.** Each Ruling's canonical
bytes carry its predecessor's digest, so a cycle whose relation rows agree with its bytes would be a
cycle of SHA-256 preimages. What a store can actually hold is a cycle in the relation that its
Rulings' bytes contradict, and that is refused as the disagreement it is.

**Supersession is a relation, not a nullable column.** `ruling_supersessions` holds one row per
citation with both foreign keys scoped over `(workspace_id, proposal_digest)`, so a link across
Workspaces or across Proposals cannot be stored rather than being refused by a Command something
might bypass. "No predecessor" is the absence of a row; the bytes say `supersedes = none`, and
**every read** — not only startup — checks that the two agree.

## Force is two facts, reported together

`RulingForce` is `{ superseded_by: Option<RulingDigest>, drift: Vec<UnmetPrecondition> }`. A Ruling
is in force exactly when the first is `None` and the second is empty. Both are always reported, so
no caller has to learn which of "superseded" and "drifted" wins — a Ruling can be both.

There is deliberately no `is_in_force`, and no public accessor in the module returns a `bool`. A
single bit read off an authority record is what `BLUEPRINT:446` warns about: it would invite a
consumer to skip the verification it is required to perform itself.

Drift is **exactly the four copied revisions**, computed at read time and never stored. The
Proposal's own readiness — undeclared constraint families, an unverified readable context, a moved
Resident lifecycle or Configuration — is a fact about the Proposal. Folding it in would report every
v1 Ruling as permanently drifted, for reasons `BLUEPRINT:906` does not name.

A Ruling bound to one digest says nothing about a different digest. That is **inapplicability**, not
invalidity, and no surface may report the second as the first.

## Persistence and reconciliation

Schema version 19 adds the immutable `rulings` table and the `ruling_supersessions` relation.
Migration 0019 follows the existing closed-Event rebuild pattern, preserves every v18 Event
sequence, payload byte, and timestamp, adds only `ruling.recorded`, creates empty Ruling state, and
advances the version only from 18. Update and delete triggers refuse to change a recorded Ruling or
a recorded citation.

An authenticated ruling is one immediate transaction: verify the bound Proposal against its own
bytes and copy its envelope and bindings; read the tail and compare; canonicalise and hash; insert
the row, the citation if there is one, and exactly one `ruling.recorded` Event carrying the digest
envelope, the exact bytes as canonical RFC 4648 padded base64, actor `owner`, and the ruling time.

Identical re-ruling needs no check of its own: two Rulings that differ in nothing produce one digest
and `rulings.digest_value` is the primary key, so the second insert collides. What a caller actually
hits first is the chain rule, because a deliberate reversal differs by its citation and therefore by
its digest — so two identical Rulings are not expressible while that rule holds.

Startup runs, per Ruling, the same proof every read performs — digest recomputed from the bytes,
bytes decoded and re-encoded, every indexed column and the supersession relation compared to the
parsed value — and then the one check no read performs: **the copied revisions still equal what the
bound Proposal recorded**. Then exactly one Event per Ruling with no orphan across the table, and
per Proposal the chain re-derived: one first link, one path, every Ruling on it, ruling times that do
not run backwards along it. Refusal means `Supervisor::bootstrap` returns an error instead of a
`StartupReport`, so no service is constructed against that store.

Force is derived along the way and never refuses startup by itself. Drift fills a list; it does not
raise an error, so a historical Ruling does not become corrupt because the world moved past it.

## Required evidence, and the test that carries each bullet

ADR 0037 states that each of its required-evidence bullets maps to one named test and that the
mapping is recorded here, "so that 'maps to a test' is a fact a reviewer can check rather than a
claim the author makes". This is that mapping. Bullets are numbered in the order the ADR lists them.
Five of them do not map cleanly, and those are stated as such rather than papered over.

Test locations: **[u]** = `crates/beacond/src/ruling.rs` unit tests, **[i]** =
`crates/beacond/tests/ruling.rs`, **[m]** = `crates/beacond/src/lib.rs` migration tests.

| # | bullet | tests |
|---|---|---|
| 1 | a known byte fixture independently reproduces its SHA-256 digest | `the_encoder_produces_the_published_identity_vector` [u] · `the_published_identity_vector_reproduces_its_digest_with_no_encoder` [u] |
| 2 | every identity-bearing field changes the digest; a changed byte, a reordered field, or a trailing byte is refused or produces a different identity | `each_identity_bearing_field_changes_the_digest` [u] · `every_single_byte_of_the_fixture_is_load_bearing` [u] · `a_trailing_byte_is_refused` [u] · `a_truncated_record_is_refused` [u] · `a_reordering_the_grammar_can_see_is_refused` [u] · `two_same_width_values_swapped_are_a_different_record_rather_than_a_refusal` [u] |
| 3 | ruling on a digest that names no stored Proposal writes neither row nor Event | `ruling_on_a_digest_that_names_no_proposal_writes_neither_row_nor_event` [i] · `a_proposal_outside_this_services_workspace_is_not_found` [i] · `a_ruling_for_an_unknown_proposal_is_refused_by_the_store` [m] |
| 4 | the copied revisions equal the bound Proposal's at ruling time; copies that disagree refuse startup | `the_copied_revisions_are_the_proposals_not_the_current_ones` [i] · `the_bound_proposal_envelope_is_copied_from_the_proposal_not_rebuilt_from_constants` [i] · `startup_refuses_copies_that_disagree_with_the_bound_proposal` [i] |
| 5 | each bound revision that can move independently names that revision and only that one; a moved pinned base moves its Repository revision too and both are named | `a_moved_charter_is_named_alone_and_the_record_does_not_change` [i] · `a_moved_policy_revision_is_named_alone` [i] · `a_new_repository_revision_with_the_same_base_reports_only_the_revision_as_moved` [i] · both-facts case: `a_rebind_that_moves_the_pinned_commit_names_both_facts` [i] · control: `a_fresh_ruling_reports_neither_drift_nor_supersession` [i] |
| 6 | a drifted Ruling stays readable and attributable, and startup still succeeds | `a_moved_charter_is_named_alone_and_the_record_does_not_change` [i] · `a_drifted_chain_still_starts_and_stays_readable` [i] |
| 7 | a second Ruling supersedes the first, both stay readable, the effective Ruling is the last link | `a_second_ruling_supersedes_the_first_and_both_remain_readable` [i] · `any_decision_may_follow_any_decision_and_every_link_carries_its_own_reason` [i] |
| 8 | a `supersedes` naming no Ruling, or one bound to a different Proposal, is refused | `a_first_ruling_that_cites_an_unrecorded_digest_is_refused_as_unrecorded` [i] · `citing_another_proposals_ruling_is_refused_as_that_and_not_as_unrecorded` [i] · `citing_a_link_that_is_no_longer_the_tail_is_refused` [i] · `a_later_ruling_that_cites_nothing_is_refused_as_not_the_tail` [i] |
| 9 | two predecessors or a double supersession refused by the store; a fork or a cycle refuses startup | `the_store_refuses_a_second_predecessor_and_a_second_supersession` [i] · `each_composite_foreign_key_independently_refuses_a_cross_proposal_relation` [i] · `startup_refuses_a_forked_chain` [i] · `startup_refuses_a_supersession_cycle` [i] · `a_forked_chain_has_no_last_link_and_is_refused` [i] · `a_supersession_cycle_has_no_first_link_and_is_refused` [i] |
| 10 | a ruling time earlier than the link it supersedes is refused when recorded **and** again at startup | recording half: `a_ruling_time_earlier_than_the_tail_it_cites_is_refused_when_recorded` [i] · startup half: `startup_refuses_a_ruling_time_that_goes_backwards_along_the_chain` [i] |
| 11 | update and delete against a recorded Ruling are refused by the store | `ruling_rows_refuse_update_and_delete` [i] · `a_recorded_ruling_refuses_update_and_delete` [m] |
| 12 | an unauthenticated caller receives only the authentication refusal | `unauthenticated_recording_returns_only_the_account_refusal` [i] |
| 13 | identical re-ruling is refused rather than duplicated | `a_later_ruling_that_cites_nothing_is_refused_as_not_the_tail` [i] — the same call twice, refused the second time with nothing written. The digest primary key is the deeper guard and is unreachable while the chain rule holds. |
| 14 | missing, duplicate, orphaned, altered, or differently encoded Ruling state or Event evidence refuses startup | `startup_refuses_a_ruling_without_its_event` [i] · `startup_refuses_a_duplicate_ruling_event` [i] · `startup_refuses_two_events_for_one_ruling_and_none_for_another` [i] · `startup_refuses_an_orphan_ruling_event` [i] · `startup_refuses_altered_ruling_bytes` [i] · `startup_refuses_an_indexed_column_that_disagrees_with_the_bytes` [i] · `startup_refuses_a_differently_encoded_event_payload` [i] · `startup_refuses_a_supersession_relation_that_disagrees_with_the_bytes` [i] · `startup_refuses_an_event_naming_an_actor_this_version_has_no_arm_for` [i] · `startup_refuses_an_event_whose_time_disagrees_with_its_row` [i] · `startup_refuses_an_event_recorded_under_another_workspace` [i] · `a_ruling_event_that_does_not_match_its_row_refuses_startup` [i] · control: `a_truthful_store_starts_and_reconciles` [i] |
| 15 | a trailing byte is refused; a structurally visible reordering is refused; a same-width reordering changes the identity | trailing byte: `a_trailing_byte_is_refused` [u] · visible reordering: `a_reordering_the_grammar_can_see_is_refused` [u] · same-width: `two_same_width_values_swapped_are_a_different_record_rather_than_a_refusal` [u] |
| 16 | `reason` is mandatory: empty, whitespace-only, or over-long is refused | `the_reason_must_be_trimmed_bounded_and_free_of_control_characters` [u] · `a_malformed_reason_or_digest_writes_neither_row_nor_event` [i] |
| 17 | `decision` and `decider` are closed: any unassigned tag is refused | `an_unassigned_tag_is_refused_in_every_closed_position` [u] · `the_store_refuses_a_ruling_row_whose_digest_envelopes_are_not_this_versions` [i] · `startup_refuses_an_event_naming_an_actor_this_version_has_no_arm_for` [i] |
| 18 | no accessor reports force as a boolean — enforced by review of the public surface, not by a test | **none, and the bullet now says so.** The force-reporting accessors are `RulingForce::superseded_by`, `RulingForce::drift`, and `Ruling::force`; the other Ruling value accessors — `digest`, `canonical_bytes`, `payload`, `grammar`, and `candidate` — also return non-boolean domain values. `a_ruling_can_be_both_superseded_and_drifted_and_says_so` [i] is behavioural evidence about compound state and is NOT a guard on this: adding `is_in_force() -> bool` leaves it passing (gpt2) |
| 19 | recording adds exactly one Ruling row and one Event and leaves every other **stored** count unchanged; process, network and filesystem inertness is a review property | `recording_a_ruling_creates_no_run_receipt_worktree_or_external_effect` [i] — Run, Receipt and security-event rows unchanged, exactly one new Event and one new Ruling row, and the absent `invocations`/`capabilities` tables. That is the whole of what the bullet claims. Direct process/network/non-store-filesystem call sites: reviewed, not tested — Residual 5, **option (a) applied**, Kevin may swap it for (b) |

Bullet 18 (bullet 19 before Residual 2 removed the duplicate) is an **absence**, and the note
previously called it "structural, not test-mappable" while still citing a test beside it. That was
an overclaim; Residual 4 below says what is actually true, and the bullet now says it in ADR 0037
itself.

Also verified but not required by any bullet: the codec's own round trip
(`decode_then_encode_is_the_identity`, `the_record_starts_with_the_seventeen_byte_canonicalizer_magic`,
`absence_is_a_value_so_the_two_supersedes_states_have_different_identities`,
`the_largest_legal_record_fits_the_stores_envelope`,
`only_a_sha256_beacon_proposal_v1_envelope_is_accepted`,
`hostile_stored_bytes_carrying_an_uppercase_base_revision_are_refused_by_the_decoder`,
`digests_and_the_base_revision_are_refused_rather_than_case_folded`), the query surface
(`an_owner_records_one_ruling_bound_to_the_exact_proposal_it_named`,
`the_audit_event_carries_the_exact_bytes_as_padded_base64_and_the_owner_actor`,
`get_refuses_a_digest_that_names_nothing_and_list_separates_unknown_from_undecided`), read-time
integrity (`a_stored_ruling_that_does_not_name_its_bytes_is_refused_by_every_read`,
`a_row_wearing_another_rulings_digest_is_refused_by_the_digest_it_claims`,
`an_indexed_column_that_disagrees_with_the_bytes_is_refused`,
`a_supersession_row_that_disagrees_with_the_bytes_is_refused`,
`startup_refuses_a_ruling_whose_owner_account_is_gone`), the write/read seam
(`a_competing_writer_cannot_hand_the_tail_read_a_stale_answer`), and the migration
(`version_eighteen_store_preserves_event_bytes_and_adds_empty_ruling_state`,
`migration_nineteen_advances_the_version_only_from_eighteen`,
`migration_nineteen_rolls_back_exactly_before_forward_startup`,
`the_version_guard_alone_refuses_a_structurally_ready_store_that_is_not_stamped_eighteen`).

### Gap 1 — closed

Bullet 10 was half met when this note was first written: `InconsistentRuling::RuledBeforeTail`
existed and nothing exercised it. `a_ruling_time_earlier_than_the_tail_it_cites_is_refused_when_recorded`
now does, by planting a tail whose ruling time the clock cannot have produced and then citing it. It
goes red when the guard is removed, and it asserts that the refusal is `RuledBeforeTail` rather than
`NotTheCurrentTail` — the call cites the correct tail, so the time rule and the chain rule are
separate refusals, and a test that could not tell them apart would prove neither.

### Residual 1 — bullet 15 contradicts the amended ruling on reordering

Bullet 15 says a reordered field "is refused rather than tolerated". Bullet 2 already carries the
amended wording ("refused **or produces a different identity**"), and the body of ADR 0037 now
states plainly that same-width reordering is undetectable by construction. Bullet 15 is a leftover
from an earlier amendment and is false as written. Proposed wording: *"a trailing byte is refused
rather than tolerated; a structurally visible reordering is refused, and a same-width reordering
changes the identity."*

**Applied in ADR 0037** in the commit that carries this line (a commit cannot cite its own
hash; the report for this change gives it). Applied as written, and the reworded bullet turned out to be
fully mappable at main, which the residual did not expect: `a_reordering_the_grammar_can_see_is_refused`
asserts the visible half and `two_same_width_values_swapped_are_a_different_record_rather_than_a_refusal`
asserts the same-width half. Row 15 of the table above now cites all three.

### Residual 2 — bullets 14 and 18 are the same bullet

Bullet 18 restates bullet 14 in longer form; both arrived from separate amendments. A list that must
map one-to-one onto tests cannot carry two spellings of one claim. Proposed change: delete bullet
18, whose specifics bullet 14 already covers.

**Applied in ADR 0037** in the commit that carries this line (a commit cannot cite its own
hash; the report for this change gives it). Deleting a bullet renumbers the list: old 19 and 20 are now 18 and 19,
and the table above follows them.

### Residual 3 — bullet 5 asks for something the revision model cannot produce

Bullet 5 wants each of the four bound revisions, moved alone, to name "that revision and only that
one". Three of them can. The pinned base **cannot move alone**: it is a field of a Repository
revision and Repository Bindings are append-only, so changing the pinned commit necessarily
publishes a new revision, and both `RepositoryRevisionMoved` and `BaseRevisionMoved` become true
together. `a_rebind_that_moves_the_pinned_commit_names_both_facts` asserts exactly that, and it is
right to: reporting one fact when two are true would be the same defect in the opposite direction as
the round-two bug that reported two when one was.

So the bullet is unachievable as written for one of its four cases, and the note previously claimed
it clean. Found by gpt2's slice-4 review. Proposed wording: *"each bound revision that can move
independently — Charter, Project Policy, and Repository — names that revision and only that one; a
moved pinned base necessarily moves its Repository revision too, and both are named."*

**Applied in ADR 0037** in the commit that carries this line (a commit cannot cite its own
hash; the report for this change gives it). The reason the base cannot move alone is carried into the bullet itself
rather than left in this note, so a reader of the ADR does not have to find this file to learn why
one of its four cases reads differently.

### Residual 4 — bullet 19 has no guard, and the cited test is not one

Bullet 19 is an **absence**: `RulingForce` has no `is_in_force`, and no public accessor in the
module returns a `bool`. A test cannot assert that a function does not exist, so the note originally
called the row "structural, not test-mappable" and cited the compound-state test as nearest
evidence.

That was still an overclaim, and gpt2 demonstrated it: adding a public `is_in_force() -> bool` to
`RulingForce` leaves `a_ruling_can_be_both_superseded_and_drifted_and_says_so` **passing**. The
cited test shows both facts reported together; it does not, and cannot, notice a third accessor
appearing beside them. Citing it as evidence implied a guard that is not there.

So the honest statement is that **this bullet is unguarded**. The property holds today — verified by
reading the module's public surface, where the only accessors are `superseded_by`, `drift`,
`digest`, `canonical_bytes`, `payload`, and `force` — and nothing would turn red if a later change
broke it. Closing it needs a compile-time check, which is a `trybuild`-style dependency this
workspace does not carry and should not gain for one assertion. The alternative is that this stays a
review obligation and ADR 0037 says so, rather than a required-evidence bullet promising a test that
cannot exist. Proposed wording: *"no accessor reports force as a boolean — enforced by review of the
public surface, not by a test."*

**Applied in ADR 0037** in the commit that carries this line (a commit cannot cite its own
hash; the report for this change gives it). A **fifth** wrongness surfaced while applying this one, and is fixed in
the same commit: the sentence closing the evidence list read "Each bullet above maps to one named
test", which was already false for the many-test bullets and became self-contradicting beside a
bullet that says it has none. It now says each bullet maps to one or more named tests *except where
the bullet itself says otherwise*, and why that exception is visible.

All four residuals were edits to ADR 0037, which was under non-author review when they were written.
They were recorded here rather than applied, so the reviewer was not reading a moving document; ADR
0037 is accepted and they are applied now.
Residuals 1 and 2 were found by writing this mapping; residuals 3 and 4 were found by gpt2 reviewing
it, which is the argument for having written it down at all.

### Residual 5 — bullet 20 is proved for stored state only

The test named for this bullet counts rows and checks that two tables still do not exist. That is
the whole of what it observes, and the bullet's wording promises more: worktree, process, network,
and "external-effect" counts. gpt2 showed the gap by mutating `rule` to spawn an actual `true`
process; the test still passed.

It is not an accident of this slice. `submission_creates_no_run_receipt_worktree_or_external_effect`
in the Proposal suite has the same shape, so the repository's existing convention for this assertion
is "no new stored state", stated in a sentence that claims more than it measures. Two honest ways
out, and this is a decision rather than a defect to patch quietly:

- **Narrow the bullet** to what the store can witness — no Run, Receipt, Ruling-adjacent or Event
  row beyond the one recorded, and no Invocation or capability state — and say that process,
  network, and filesystem inertness rest on the module having no such call sites, which is a review
  property.
- **Widen the test**: assert the worktree root is unchanged before and after, and observe process
  and network inertness the way `sandbox` and `check_preflight` already do for Runs. That is real
  work and belongs with dispatch, when there is something that could plausibly spawn.

**Status: Kevin's decision, not taken here.** Option (a) is APPLIED in ADR 0037 in the commit that
carries this line, so the accepted document does not keep claiming what it does not measure while
the decision is open. That is a starting point, not the choice: if Kevin prefers (b), the bullet
returns to its wider wording and the test is widened to match. Applying (a) costs nothing that (b)
would have to undo beyond one bullet's wording.

The same overclaim exists one module over — `submission_creates_no_run_receipt_worktree_or_external_effect`
in the Proposal suite has the identical shape — so whichever way this goes, it is a convention to
settle rather than one bullet to patch.

Until one is chosen, the mapping should not claim the bullet whole. Proposed wording for the narrow
option: *"recording a Ruling adds no Run, Receipt, Invocation, or capability state and exactly one
Event; process, network, and filesystem inertness follow from the module having no such call
sites."*

## Knowingly unreachable checks

Four checks exist that no store state can reach, kept deliberately and named here so that a
reviewer rules on them rather than discovering them.

1. **The re-encode identity check** in `verified_ruling`. No byte sequence this grammar can decode
   re-encodes differently — every field is fixed-width or exactly length-prefixed, so there is no
   second spelling of one value. It is ADR 0037's stated discipline and the line that would refuse a
   second spelling the day a field becomes optional or a list appears.
2. **The Owner-account check** in `verify_ruling_state`. Every Ruling binds a Proposal, Proposal
   reconciliation makes the identical check, and it runs first, so this can never be the arm that
   fires.
3. **`ruling_chain`'s two map-construction conditions** — a predecessor cited twice, a Ruling citing
   two predecessors. The primary key and the unique index forbid both outright;
   `the_store_refuses_a_second_predecessor_and_a_second_supersession` proves those constraints
   instead. Its third condition — a predecessor outside this Proposal's Ruling set — is forbidden by
   the relation's scoped composite foreign keys, proved by
   `each_composite_foreign_key_independently_refuses_a_cross_proposal_relation`.
4. **The orphan-Event total** in `verify_ruling_state`. Per-Event verification runs first and already
   refuses an Event naming a digest no row carries, by resolving it in that Event's *own* Workspace,
   so the later whole-table total can fire only if that call ordering changes.
   `startup_refuses_an_orphan_ruling_event` proves the store is refused;
   `startup_refuses_an_event_recorded_under_another_workspace` needs **both** arms removed before it
   goes red, which is what says the two cover each other rather than one covering nothing.

   This entry was missing from the first version of this list, although
   `C14-opus-build-report.md` had recorded it. gpt2's slice-4 review caught the regression: a
   checkpoint that drops a fact its own source packet established is worse than one that never
   knew it.

The rule applied: **redundancy inside one function is removed; redundancy across modules or across
call ordering is kept and named.** One function should have one way to fail, and a module's
reconciliation should not be complete only because of the order `inspect_store` happens to call
things in. Two checks were removed under the first half of that rule during this work — an explicit
fork branch subsumed by the walk-length comparison, and a pre-insert duplicate-digest query subsumed
by the primary key.

## Left out of this checkpoint, and named rather than implied

No Gate entity, so nothing reports which Proposals await a decision. No expiry and no withdrawal,
because neither is a ruling: those are states of a Gate, and an Owner who changes their mind records
a new Ruling that refuses and cites the prior, leaving the original decision legible instead of
erasing it. No non-Owner decider, so the rule that an Agent cannot approve its own Proposal has no
code path to exercise yet — the closed one-arm decider is what keeps that honest rather than a check
that cannot fail. No wire and no desktop surface: nothing can consume a Ruling until dispatch
exists, and a surface that displayed "approved" without a dispatch path would be the thing
`BLUEPRINT:446` warns against. When dispatch lands, the Query and the enforcement path can be
reviewed together as one authority surface, which is where the interesting mistakes are.

## Verification

From the repository root:

```sh
cargo test -p beacond --lib ruling
cargo test -p beacond --test ruling
cargo test -p beacond --lib version_eighteen_store_preserves_event_bytes_and_adds_empty_ruling_state
cargo test -p beacond --lib migration_nineteen_advances_the_version_only_from_eighteen
cargo test -p beacond --lib migration_nineteen_rolls_back_exactly_before_forward_startup
scripts/gate all
```

Applying migration 0019 to live data remains a separate RED action.
