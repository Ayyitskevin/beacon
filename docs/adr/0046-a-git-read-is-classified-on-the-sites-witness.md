---
status: proposed
---

# A git read is classified on the site's witness

Beacon reads a repository it does not own. Every one of those reads runs as a bounded child of a
pinned repository, and every one of them can end three ways that look alike from outside: the thing
is there, the thing is not there, or the read did not happen. Git reports all three through an exit
code, and an exit code cannot tell them apart — `cat-file` exits 128 both for "that path is not in
this tree" and for "this repository is broken", and `merge-base --is-ancestor` exits 1 to say "no".

Reading a non-zero exit as an absence is therefore not a shortcut at most sites; it is a wrong
answer that looks like a right one. A dispatch that took it reported `PreImageMismatch` — compared
and differed — for a comparison that never happened, and a declared file creation would have Applied
against a repository whose object store was corrupt. That defect was found by instrumentation, not
by a test, because nothing in the type system distinguished the two.

But the opposite absolute is also wrong, and cost a shipped test to learn: at a site whose
documented answer IS the exit status, discarding the code turns "no" into "could not tell". The rule
is therefore not "never the exit code" but "the site declares what kind of answer it gives".

Eight lanes built it: R46.1 typed the read, R46.3 keyed absence on what git said about the subject,
R47 and R47.1 generalised that to every read site, R49 found that the pin itself could be lost
silently, R53 and R53.1 held the read bound at both its edges, and R55/R55.1 divided the sites whose
absence is silence from the sites whose answer is the exit code. The contract they built is in the
code and in the rulings; it has never been in one document. This ADR is that document, so that the
next reader audits against rows rather than re-deriving the rule from tests.

Kevin decides this ADR. Every normative sentence below cites the ruling that decided it and a test
that fails when it is broken; a sentence with no such test is marked **UNTESTED** and listed under
Open.

## Decision

A pinned git read is classified by the **kind of answer its site declares**. At Message and
Silence sites a code alone never proves absence; at an ExitCode site the code is the answer.

1. A pinned byte read has three answers and never two: `Bytes`, `NotFound`, or
   `Unreadable(subcase)`. `NotFound` is a fact about the tree — git ran, under the hardening, in the
   still-pinned repository, and reported no such object. `Unreadable` is the *absence of a fact* and
   says nothing about the tree. (R46.1 ·
   `worktree::r46_read_classification::bytes_is_none_for_both_no_bytes_answers`)
2. Each read site declares its witness kind, and the kind decides what a non-zero exit means there.
   A Message site requires the named absence diagnostic; a Silence site treats a silent non-zero as
   `Unreadable(SilentExit { code })`; an ExitCode site returns a silent non-zero as its answer and
   treats a diagnostic as `Unreadable(ReadRefused)`. (R46.3, R47, R55, R55.1 ·
   `worktree::r46_read_classification::sol1_exit_128_with_empty_stderr_is_not_an_absence_at_every_silent_site`,
   `worktree::tests::a_read_whose_answer_is_its_exit_code_tells_no_apart_from_could_not_tell`)

   The second citation is the one that matters for the site's **declaration**: changing
   `read_pinned_exit`'s witness back to `Silence` leaves the classifier tests GREEN and turns that
   test RED (sol1's cold read, mutation 1). A rule about which site declares what needs a bar at the
   site, not only at the classifier.
3. One classifier serves every site. There is no per-site exit-code arithmetic, and a site
   participates by naming its witness, not by writing its own rule. (R47, R47.1 ·
   `worktree::r46_read_classification::output_defeats_both_witnesses`, which loops over two of the
   three witness kinds — see Open 2)
4. The rule fails closed. An unrecognised diagnostic is `Unreadable`, never assumed to be an
   absence. (R46.3 ·
   `worktree::r46_read_classification::an_unrecognised_diagnostic_is_unreadable_rather_than_assumed_absent`)
5. Bytes on stdout defeat the witness: a child that produced output produced *output*, and calling
   that an absence discards the very bytes it read. Tested at the Message and Silence witnesses; the
   ExitCode witness is held by the same code path but has no bar of its own — **UNTESTED**, Open 2.
   (R47 · `worktree::r46_read_classification::a_non_zero_exit_that_produced_output_is_never_an_absence`,
   `worktree::r46_read_classification::output_defeats_both_witnesses`)
6. The pin is re-checked before the exit code is read. Bytes from a repository that is no longer the
   pinned one are not evidence about it, whatever the child's status said. (R46.1 ·
   `worktree::r46_read_classification::a_pin_broken_after_the_read_is_unreadable_even_when_the_child_succeeded`)
7. `Unreadable` carries a **closed vocabulary** of subcases, each with its own outward name, so a
   Receipt says *which* read failed rather than *that* one did. (R46, R46.3 ·
   `dispatch::tests::pre_image_unreadable_is_its_own_outcome_and_not_a_mismatch`, whose
   `subcases.len()` assertion is the closed-count witness)
8. The pin is a **capability**, not a declaration: a descriptor verified to still name the pinned
   repository and kept across `exec`, never a pathname re-resolved by the child. (R49 ·
   `worktree::tests::a_git_directory_that_opens_on_the_placement_descriptor_still_reaches_the_child`)
9. The read bound uses one clock checked before the first spawn, before every retry, and before
   publication. At or after the deadline the operation is `TimedOut`, and no new child starts.
   (R53, R53.1 ·
   `worktree::tests::sol1_a_child_finishing_one_millisecond_past_the_bound_is_timed_out`,
   `worktree::tests::sol1_a_zero_budget_never_starts_the_child`)

## The state table: witness kind × child result → read state

This table is the contract. It is `classify_pinned_read` (`crates/beacond/src/worktree.rs:1065`,
main `2b550b8c`) read as rows. "Diagnostic" means stderr carrying any non-empty line that is not prefixed `hint:`,
git's documented advice channel (R47.1 ·
`worktree::r46_read_classification::git_advice_is_not_a_diagnostic_but_everything_else_is`).

Rows that hold at every witness:

| child result | stdout | stderr | read state | ruling · test |
|---|---|---|---|---|
| could not start or be waited on | — | — | `Unreadable(ChildFailed)` | R46.1 · `worktree::r46_read_classification::a_child_that_could_not_start_is_unreadable_and_says_so` |
| outlived the read budget | — | — | `Unreadable(TimedOut)` | R46.1, R53 · `worktree::r46_read_classification::a_child_that_outlived_its_budget_is_unreadable_and_says_so`, `worktree::tests::sol1_a_child_finishing_one_millisecond_past_the_bound_is_timed_out` |
| finished, pin no longer holds | any | any | `Unreadable(PinBrokenAfterRead)` — decided **before** the code is read | R46.1 · `worktree::r46_read_classification::a_pin_broken_after_the_read_is_unreadable_even_when_the_child_succeeded` |
| terminated by a signal (no code) | — | — | `Unreadable(Signalled)` | R46.1 · `worktree::r46_read_classification::a_child_killed_by_a_signal_is_unreadable_and_says_so` |
| exit 0, still pinned | any | any | `Bytes(stdout)` | R46.1 · `worktree::r46_read_classification::a_non_zero_exit_in_a_pinned_repository_is_not_found_and_never_unreadable` |
| non-zero, still pinned, **stdout non-empty** | bytes | any | `Unreadable(ReadRefused { message })` | R47 · `worktree::r46_read_classification::a_non_zero_exit_that_produced_output_is_never_an_absence` |

Rows where the witness decides — non-zero exit, still pinned, empty stdout:

| witness kind | stderr | read state | ruling · test |
|---|---|---|---|
| `Message(m)` | contains `m` | `NotFound` | R46.3 · `worktree::r46_read_classification::an_absence_is_what_the_site_can_show_and_the_sites_show_it_differently` |
| `Message(m)` | anything else, including empty | `Unreadable(ReadRefused { message })` | R46.3 · `worktree::r46_read_classification::a_broken_repository_is_unreadable_and_never_not_found` |
| `Silence` | no diagnostic | `Unreadable(SilentExit { code })` — live (R55) | R55 · `worktree::r46_read_classification::sol1_exit_128_with_empty_stderr_is_not_an_absence_at_every_silent_site` |
| `Silence` | diagnostic | `Unreadable(ReadRefused { message })` | R47 · `worktree::r46_read_classification::an_absence_is_what_the_site_can_show_and_the_sites_show_it_differently` |
| `ExitCode` | no diagnostic | classifier: `NotFound` carrier; `read_pinned_exit`: the original code | R55.1 · `worktree::tests::a_read_whose_answer_is_its_exit_code_tells_no_apart_from_could_not_tell` |
| `ExitCode` | diagnostic | `Unreadable(ReadRefused { message })` — broken speaks at these sites (grok's measured table); the adapter then returns `None` | R55.1 · **UNTESTED as a named classifier row**, see Open 2 |

The `ExitCode` row's two halves are deliberately separate. `classify_pinned_read` has no way to
return "the code" — it returns a `PinnedRead` — so it answers `NotFound` as a **carrier**, and
`read_pinned_exit` (`worktree.rs:822-843`) turns that carrier back into the code it saved before
classifying. Reading the classifier alone would suggest an ExitCode site reports an absence, and it
does not: the absence never leaves the function.

## Every read site, and the witness it declares

| site | entry point | witness | why | ruling |
|---|---|---|---|---|
| `cat-file blob` — the pre-image gate and the effect post-condition | `read_pinned_bytes`, `worktree.rs:1033` | `Message("does not exist in")` | git *names* the path as absent; silence from `cat-file` is not an absence | R46.3 |
| text reads (`symbolic-ref --quiet`, and its neighbours) | `read_pinned`, `worktree.rs:785` | `Silence` | these answer negatively in silence and speak when they fail | R47 |
| reads whose answer is the exit code (`merge-base --is-ancestor`) | `read_pinned_exit`, `worktree.rs:822` | `ExitCode` | the legitimate negative *is* a silent exit 1, so discarding the code makes "no" and "could not tell" one value | R55.1 |
| the filter-driver inventory (`config --includes --name-only --null --list`) | `pinned_filter_drivers`, `worktree.rs:1168` | `Silence` | an empty inventory is exit 0 with empty bytes; a silent non-zero is `SilentExit`, and therefore an unreadable inventory — it does not mean "git listed nothing" | R47.1, R55 |

The filter inventory is a **precondition of every pinned read**, not one of its answers: it decides
which filter drivers the hardening suppresses, so a read that proceeds with an empty inventory
proceeds with the hardening switched off. R50 moved it off the `PinnedRead` type and onto the error
channel — `read_pinned_bytes` returns `Result<PinnedRead, FilterInventoryFailure>` — so the type
says which read failed. (R47, R50 · `worktree::tests::an_inventory_git_refused_is_never_an_empty_filter_set`,
`worktree::tests::every_configuration_git_cannot_read_is_a_refusal_that_carries_why`,
`dispatch::tests::r50_the_two_read_failures_are_not_interchangeable`)

## The closed read-failure vocabulary

Eight subcases on main `2b550b8c`, each with a fixed outward name, counted by an assertion that
fails when a row is added or deleted (`dispatch.rs:4390-4411`, inside
`dispatch::tests::pre_image_unreadable_is_its_own_outcome_and_not_a_mismatch` at `dispatch.rs:4360`).

| subcase | outward name | meaning | status |
|---|---|---|---|
| `ChildFailed` | `child_failed` | the child could not be started or waited on | live |
| `Signalled` | `signalled` | terminated by a signal; there is no exit code | live |
| `TimedOut` | `timed_out` | outlived the read budget (R53) | live |
| `PinBrokenAfterRead` | `pin_broken_after_read` | the repository is no longer the pinned one | live |
| `ReadRefused { message }` | `read_refused` | git refused and did not name the subject as absent; carries git's message, bounded to 120 characters (R28.4) | live |
| `FilterInventoryUnreadable` | `filter_inventory_unreadable` | RETIRED by R50; retained so already-written Receipts stay decodable | retired, decodable |
| `FilterInventoryRefused { message }` | `filter_inventory_refused` | RETIRED by R50, same terms | retired, decodable |
| `SilentExit { code }` | `silent_exit` | a `Silence` site exited non-zero saying nothing (R55) | live |

A retired variant is retained, never deleted: deleting it would make Receipts already in a store
undecodable, which is the same reason `DispatchOutcome::Admissible` is retained. Retirement means no
new dispatch produces it, not that the vocabulary forgot it.

## What each dispatch outcome does with each read state

The pre-image gate (`dispatch::pre_images_hold`) is where a read state becomes a decision. The
declared × observed table itself is not duplicated here: it belongs to
[ADR 0040](0040-outcome-document-renders-records-it-never-makes-them.md)'s family and is owned by
R46.8.

| read result | recorded observation | dispatch outcome | ruling · test |
|---|---|---|---|
| `Err(FilterInventoryFailure)` | none — no child ran | `FilterInventoryUnreadable`, carrying the inventory failure | R50 · `dispatch::tests::r50_inventory_receipt_requires_its_failure`, `dispatch::tests::r50_other_outcome_rejects_an_inventory_failure` |
| `Unreadable(subcase)` | **none** — a read that failed is not evidence about the tree | `PreImageUnreadable`, carrying the subcase | R46, R46.6 · `dispatch::tests::pre_image_unreadable_is_its_own_outcome_and_not_a_mismatch`, `dispatch::tests::astra_unreadable_receipt_requires_its_failure`, `dispatch::tests::astra_other_outcome_rejects_a_pre_image_failure` |
| `NotFound` | `ObservedPreImage::Absent` — an observed absence is a **value**, not a missing field | holds against a declared absence; `PreImageMismatch` against declared bytes | R46.7 · `dispatch::tests::a_mismatch_receipt_owes_the_digest_it_read` (the value is coherent) and `a_holding_cell_declared_absent_and_absent_applies_and_reopens` in `crates/beacond/tests/dispatch.rs` (it holds) |
| `Bytes` | `ObservedPreImage::Bytes { digest }` | holds if the digest equals the declared one, else `PreImageMismatch` carrying the observed digest | R46.5 · `dispatch::tests::a_mismatch_receipt_owes_the_digest_it_read` |

`PreImageMismatch` means **compared and differed**, and nothing else. The two failure fields are not
interchangeable: a Receipt carries a pre-image read failure exactly when its outcome is
`PreImageUnreadable`, and an inventory failure exactly when its outcome is
`FilterInventoryUnreadable`. (R46.6, R50 · `dispatch::tests::r50_the_two_read_failures_are_not_interchangeable`)

## The pin is a capability

`GIT_DIR` is baked into the child's environment before the fork and names
`/proc/self/fd/100`, so the pinned Git directory must *arrive* on descriptor 100 and must *survive*
the exec. `dup2(git_fd, 100)` normally does both, because `dup2` clears close-on-exec on the copy it
makes — except when there is no copy to make. POSIX defines `dup2(n, n)` on an open descriptor as a
successful no-op, so when the pin has itself opened on descriptor 100 — which happens once the
process's descriptor table has grown, as it does under a parallel test run — close-on-exec survives
the placement, the exec closes the descriptor `GIT_DIR` names, and git reports
`fatal: not a git repository`. The placement "succeeded" and the capability was gone.

`enter` therefore clears the flag directly with `fcntl(F_SETFD, 0)` in that case, rather than
detouring through a scratch descriptor: the detour allocates inside the pre-exec window and can fail
with `EMFILE` in exactly the exhausted descriptor table that produces the case, so the fallback
would be least available when it is most needed. (R49 ·
`worktree::tests::a_git_directory_that_opens_on_the_placement_descriptor_still_reaches_the_child`;
at dispatch level, R51 ·
`a_dispatch_whose_git_directory_lands_on_the_placement_descriptor_still_applies` in
`crates/beacond/tests/dispatch.rs`)

The pin is checked by device and inode, not by name, because the name is exactly what a rename can
move; both the working directory and the Git directory are pinned, because swapping either one alone
voids every measurement taken through the value. (R46.1 ·
`worktree::tests::a_pinned_repository_notices_a_rename_and_a_decoy_at_its_pathname`,
`worktree::tests::a_pinned_repository_is_entered_by_descriptor_and_not_by_its_pathname`)

## Invariants

- **Absent, broken, not-evaluated and elapsed never collapse.** Each is its own state with its own
  name at every layer that can tell them apart, and a layer that cannot tell them apart says so
  rather than choosing one.
- **A guard is keyed on the property, not on something next to it.** Absence is keyed on a positive
  absence witness; a command whose documented answer is an exit status declares `ExitCode` and keeps
  diagnostic failures separate. The pin is keyed on device and inode, never on the pathname beside
  them.
- **Closed vocabularies stay closed.** A vocabulary with a counted witness gains a row only by a
  decision that updates the count, and loses one never — retired variants stay decodable.

## Consequences

A new git read site inside a pinned repository **must declare its witness kind**, and the type
system makes that unavoidable: `classify_pinned_read` takes an `AbsenceWitness` argument, so a site
cannot be added without answering "what does this command do when the subject is genuinely absent?".
Declaring it wrongly is the failure this ADR cannot prevent by construction — R47.1 corrected three
premises about the sites by *measuring the real git*, and a new site owes the same measurement
rather than an assumption.

A new `Unreadable` subcase costs three edits, by design: the variant, its `name()` arm, and the row
plus count in the closed-vocabulary witness. Forgetting the third fails
`dispatch::tests::pre_image_unreadable_is_its_own_outcome_and_not_a_mismatch` on its `subcases.len()`
assertion with "a read-failure subcase was added or a row was deleted: the vocabulary is closed" —
the count is a separate list from the `match`, so an added variant that the author also added to the
match is still caught.

A caller that folds `PinnedRead` back into an `Option` re-creates the collapse this ADR exists to
prevent, and the type will not stop it. `read_pinned` does exactly that today
(`worktree.rs:803-810`), which is Open 4 below.

Operators gain a Receipt that names which read failed, rather than one asserting a comparison that
never happened. That is the point of the whole contract: an unreadable pre-image refuses as
`PreImageUnreadable` with its subcase, and a dispatch that could not establish the hardening refuses
as `FilterInventoryUnreadable` with its own.

## Open

1. **"Four states" and "three answers" are both in use, for different things.** The brief for this
   ADR describes a pinned read as having four states; `PinnedRead`'s own doc comment says "THREE
   answers, never two (R46/R46.1)" and R46.1's ruling text says `Bytes | NotFound |
   Unreadable(subcase)` — three. Both are right about different objects: the *type* has three
   variants, and the byte-read *entry point* has four results since R50 moved the inventory
   precondition to the error channel (`Result<PinnedRead, FilterInventoryFailure>`). This ADR
   writes three-plus-a-precondition because that is what the code says. If Kevin prefers the
   four-state framing, `PinnedRead`'s doc comment is the thing to change, not this table — a
   document that says four where the type says three sends the next reader to the wrong place.
   (brief vs. `worktree.rs:1007`, R46.1)

2. **Two `ExitCode` classifier rows are UNTESTED as named rows.** R55.1 rules that at an exit-code
   site any stderr is `Unreadable(ReadRefused)`, because broken speaks there. Neither
   `ExitCode` + diagnostic nor `ExitCode` + non-empty stdout has a bar of its own: both fall out of
   the general arms, and the current adapter test would still pass if either became a `NotFound`
   carrier with code 128 (sol1). `worktree::tests::a_read_whose_answer_is_its_exit_code_tells_no_apart_from_could_not_tell`
   covers the legitimate *negative*, not the speaking failure, and
   `worktree::r46_read_classification::output_defeats_both_witnesses` loops over Silence and Message
   only — its name is now one witness short of the enum. Decision 5 is marked UNTESTED for the
   ExitCode witness for the same reason.

3. **Two unreachable arms sit on main, one of them newly dead.**
   `worktree.rs:841` reads
   `PinnedRead::Unreadable(PinnedReadFailure::SilentExit { .. }) if false => code,` — guarded by a
   literal `false`, so it can never be taken, while the arm below returns `None` for every
   `Unreadable`. It arrived with R55 and is on main, not in a lane.
   `worktree.rs:1207` reads `PinnedRead::NotFound => Ok(Vec::new()),` in `pinned_filter_drivers`,
   under the comment "Silence with a non-zero exit: git found nothing to list and said nothing about
   it" — which R55 made false. At a `Silence` site the `SilentExit` guard
   (`worktree.rs:1092-1098`) is textually the same condition as `shows_absence(Silence, …)` and is
   evaluated first, so `NotFound` can no longer reach that site at all. Measured as well as argued:
   replacing that arm with `unreachable!()` leaves 31 `worktree::` lib tests, 24 `dispatch::` lib
   tests and all 94 `dispatch` integration tests GREEN (probe reverted, `git status` clean). Both
   arms are code cleanups, not behaviour changes, and neither can carry a behavioural probe —
   which is precisely why they should not stay.

4. **`read_pinned` still discards the typed failure — and it is not the only entry point that
   does.** `worktree.rs:793` does `pinned_filter_drivers(repository).ok()?`, so an inventory failure
   at the provenance gate becomes `SourceNotBeaconMade` — a read failure reported as a claim about
   the source. R52 is the proposed lane for it and is **not in this main**. Two sibling entry points
   make the same fold at `worktree.rs:827` (`read_pinned_exit`) and `worktree.rs:1133`
   (`write_pinned`), both returning `Option`, so an inventory that could not be established arrives
   at their callers as an ordinary negative. R52's text scopes itself to "the last untyped read path
   **in dispatch.rs**", which those two are not — so this is a note about R52's reach, not a claim
   it missed them.

5. **Source commentary still states the retired absolute.** `worktree.rs:919-924` ("Nothing is
   classified on the exit code, anywhere") and `worktree.rs:1083-1084` ("an absence is what the site
   can SHOW, never what the exit code was") describe the pre-R55.1 rule, immediately above the
   `ExitCode` arm that contradicts them. A code-documentation follow-up, not a product defect
   (sol1's P1-1).

6. **This ADR adds two sections the house template does not use.** No ADR in `docs/adr/` on main
   carries `## Invariants` or `## Open`; both were required by the brief. Kevin may want them folded
   into `## Consequences` and a review note respectively, in which case the content is unchanged and
   only the headings move. (Precision from sol1: 0045 is the previously allocated ADR and its
   checked-in status is Proposed, not Accepted.)

7. **R54.1 and R54.2 fold into nothing here.** Both are Receipt-wire owner rules — the observed-key
   presence check and the observed-digest validation — with GREEN controls at main
   (`r541_*`, `r542_*`, four tests each, in `crates/beacond/tests/dispatch.rs`). They do not change
   any of the four read-state-to-outcome mappings and close none of these rows, and this ADR has no
   era section for them to land in. Recorded so the next reader does not go looking.
