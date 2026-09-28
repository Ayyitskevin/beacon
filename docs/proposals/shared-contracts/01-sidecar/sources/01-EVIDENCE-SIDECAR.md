# 01 — Optional-local evidence sidecar

DRAFT ONLY · author document assessment is in RESULT.md; independent review pending.
No implementation/adoption grant. All acceptance controls below are PROPOSED, NOT RUN.

<a id="authority"></a>
## Authority and supersession

Optional-local scope is selected by [scope disposition](./private-evidence/kevin-words-reuse-map-20260916/DISPOSITIONS.md:16).
Later [A2 selection](./private-evidence/beacon-overnight-v2-20260917/MUSE-DISPOSITIONS-20260917.md:11) selects B5-A; the
old E10 mirror promise is narrowed to local packet/publication compatibility,
**mirror compatibility NOT ASSESSED**. Corrected [B5-A](./private-evidence/beacon-overnight-v2-20260917/a1-draft/corrections/options/B5-E10-SCOPE.md:13)
controls over the original/R2 E10 wording. The retained legacy checkpoint assertion
is not proof that proposed sidecar absence handling exists
([reuse limit](./private-evidence/beacon-overnight-v2-20260917/a1-draft/corrections/options/B5-E10-SCOPE.md:15)).

This consolidates [revised sidecar interface](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:27)
and its original [acceptance source](./private-evidence/shared-contract-evidence-sidecar-20260916/REUSE-GAP-MAP.md:42).
No shared packet/seat-report/registry schema changes or compulsory rollout follow.

<a id="interface"></a>
## Inputs, outputs and ownership

One optional read-only module interface consumes a supplied run root, pinned packet
and selected round, pinned existing correction registry, and optional pinned sidecar.
It returns a local check result; it does not write a receipt or execute a gate.
The run owner owns frozen artifacts; the existing correction registry remains the
sole correction source. The sidecar owns only added claim-to-witness checking.
Deleting the module loses that added check, not packet/publication machinery.

| Input group | Proposed meaning / absent behavior |
|---|---|
| Header | Reviewed format/version, exact packet path/full digest, selected round and registry pin. Version spelling in older drafts is a proposal, not a new adopted shared schema. |
| Claim | Stable ID, literal assertion/scope, declared kind gate/tests/source-comparison/restoration/inference, typed subject (Git object format+commit versus artifact SHA256), existing claim/obligation links, source-format-bound witnesses and selectors. |
| Execution count evidence | Inert command/filter, actual exit, selected/executed/skipped/available counts, units/selection universe, null plus reason when unknown. No inferred denominators or double counting retries/helper suites. |
| Intervention | Explicit applicability; restoration YES needs exact before/after evidence, NA needs a reason, NO remains adverse and UNKNOWN unresolved. File restoration does not prove recovery of undeclared external effects. |
| Corrections | Resolve existing registry IDs and packet bind/omit decisions. No second registry or model relevance filter. |
| Profile/custody/bounds | Approved formats/selectors, exact identity/statement comparison rules, witness provenance and finite read/count/reference/text limits. Missing values are named unresolved inputs, never defaults. |

Output: per-claim `supported / unresolved / contradicted`, plus scoped binding or
format errors. **Absence means NOT CHECKED, not PASS.** These are evidence-support
labels, not author/gate/reviewer/publication verdicts or prose-truth certification.
No overall ready/approved/merge-ready aggregate. Supplied invalid sidecar is not
treated as absent. Existing packet/refusal semantics remain untouched.
Sources: [claim contract](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:34),
[count semantics](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:35),
[output limits](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:38).

<a id="semantics"></a>
## Error, uncertainty, retry and ordering

Freeze evidence → packet → sidecar. Changed packet, selected round, registry or
witness means a new immutable sidecar/version and revalidation; never rewrite
history. Rechecking identical frozen inputs is observationally idempotent.
Retry an unreadable source only through a new explicit supplied observation,
without turning an earlier refusal into proof of success.

A current-head gate claim needs that exact subject/round; legitimate historical
evidence remains historical rather than blanket-rejected. Git SHA256 and artifact
SHA256 are different types even at equal length. Unknown runner format remains
unverified; zero selected tests cannot support “tests passed.” A known denominator
is needed for a total-suite claim, not for a precisely narrowed supported count.
Later narrow PASS does not recolor older PARTIAL/FAIL/SKIP or close a different
subject/scope. The checker reports evidence support; it never performs the
authoritative obligation transition. Source error and unsupported semantic claim
remain distinct. Input order cannot manufacture evidence or priority.
Sources: [freeze ordering](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:40),
[subject controls](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:48),
[count controls](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:52),
[inherited scope](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:54).

<a id="privacy"></a>
## Privacy, trust and explicit non-effects

Use existing confined/stable regular-file and digest mechanisms, not a convenience
manifest checker claimed equivalent. Static source confirms the intended reader
mechanism ([reader](./private-evidence/tools/packet.py:89) and [digest](./private-evidence/tools/packet.py:127));
this is not runtime proof. Hashes bind bytes, not identity or truth. Only approved
sanitized evidence/labels and permitted exact quotes may be displayed; private or
untrusted diagnostics use fixed classifications and safe input indices, not raw
paths/commands/exception text. Unknown custody cannot be filled by an actor string.
No sanitization implementation or shared source-label profile is silently adopted;
proposal05 is a separate unapproved B7 decision, not a new dependency for this draft.

Commands/grants remain inert. No gate launch, subprocess, network, dispatch, receipt
publication, correction writes, watcher, node-b consumption or legacy backfill.
The existing missing-checkpoint refusal remains a refusal; it is not replaced
with sidecar NOT CHECKED ([legacy assertion](./private-evidence/tools/test_checkpoint.py:154)).
Existing publication remains MUSE-ONLY / UNKNOWN
([publication literal](./private-evidence/tools/packet.py:484)).

<a id="acceptance"></a>
## Acceptance matrix — PROPOSED, NOT RUN

Each row crosses the proposed public interface, uses independent literal expected
results and checks unchanged inputs/no forbidden effects. Runtime status for every
row is UNVERIFIED. Full original-case lineage is in [ACCEPTANCE-MAP.md](ACCEPTANCE-MAP.md).

| ID | Positive control | Contrary control / required outcome |
|---|---|---|
| E01 | Current-head exact gate evidence supports that claim; historical comparison remains visible. | Wrong head/round cannot support current gate; no combined-head inference. |
| E02 | Correctly typed same-kind identity binds. | Equal-length Git/artifact digests never join as the same subject. |
| E03 | Confined stable bytes/full digest bind, original unchanged. | Drift/escape/symlink/nonregular or bound violation refuses the witness, never silently truncates. |
| E04 | Existing correction bound or omitted with reason remains visible. | Missing/stale registry decision or sidecar/packet disagreement refuses support. |
| E05 | Declared positive selection/count/exit fits one supported runner/universe. | Zero selected/inconsistent counts/unknown denominator/unsupported runner cannot imply broader success; a narrower evidenced count remains possible. |
| E06 | YES restoration binds appropriate evidence; NA has applicability reason. | Missing/UNKNOWN unresolved, NO adverse; file-only evidence cannot certify external restoration. |
| E07 | Existing IDs/scope/text preserved; matched evidence supports only its obligation. | Omission/relabel/wrong-scope witness refuses; checker cannot mutate closure state. |
| E08 | Gate/review/author remain separately evidenced. | Later gate PASS or unchanged-file comparison cannot rewrite older PARTIAL or become combined-head PASS. |
| E09 | Exact permitted quotes/hashes/commands retained as data. | Command-like text has zero effects; source hashes unchanged; private text never appears in diagnostics. |
| E10 | Legacy local packet bytes/strict shape and author/gate/publication distinctions preserved; absent proposed sidecar NOT CHECKED. | No sidecar PASS, no weakening missing-checkpoint refusal, mirror compatibility NOT ASSESSED. |

Original rows: [R2 E01–E10](./private-evidence/beacon-overnight-revision-20260917/revisions/SIDECAR-R2.md:48);
selected narrowing: [B5-A E10](./private-evidence/beacon-overnight-v2-20260917/a1-draft/corrections/options/B5-E10-SCOPE.md:13).

<a id="missing"></a>
## Missing inputs, owners and decision map

Muse must approve precise local format, source semantics, permitted diagnostic/
quote disclosure and build fence; source/run owners supply immutable packet,
registry, supported runner evidence and custody; delegated admission owner supplies
finite bounds with explicit units. The conductor supplies legacy regression pins
and names a non-author reviewer. Mirror/node-b scope requires separate owners and
review; it is not part of selected E10. No supplied value is invented here.

Supersession: scope-only → this local proposal; R2 E01–E09 preserved/clarified;
R2 E10 broader mirror wording → B5-A narrower local controls; B5-B unselected.
Model-memory dependency sought: remembering exact evidence scope, counts and
corrections during filing; no measured savings claimed.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
