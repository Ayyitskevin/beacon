# 01 — Optional-local evidence sidecar — draft PR README

This is a fleet-workflow review packet, not an adopted Beacon product interface,
ADR, domain definition, or implementation plan. Excluding another proposal from
this packet does not resolve any dependency or standing hold.

**REVIEW PACKET: DRAFT / NOT ACCEPTED / NO IMPLEMENTATION.** This folder packages existing, already
written documents for Muse's **document-only** review. Nothing here is a build,
adoption, workspace-creation, test, or execution grant, and no owner input is
supplied or narrowed by this packaging. Semantics live only in the copied sources;
this README reports status and never changes meaning.

**Historical-status note.** Status lines inside the copied sources (for example
"BLOCKED — owner returns missing" in the owner packet, "independent review pending"
in the original proposal, "conditionally accepted as documentary draft" in the Muse
disposition) describe their own freeze dates. The current status is only what this
README states below; copied text is privacy-edited; its status statements are historical and must be read as history.

## Problem and previously proposed scope

The existing fleet workflow proposal addresses claim-to-witness checking
while keeping evidence support separate from gate and author authority. The
selected scope is one **optional, read-only, local** module that consumes a supplied
run root, pinned packet, selected round, pinned existing correction registry, and an
optional pinned sidecar, and returns per-claim `supported / unresolved / contradicted`
evidence labels. Absence of the sidecar means **NOT CHECKED**, never PASS; there is no
overall ready/approved aggregate and no gate, receipt, or verdict transfer. Mirror
compatibility is **NOT ASSESSED** (B5-A narrowed the old E10 wording to local
packet/publication compatibility only). Full text: [sources/01-EVIDENCE-SIDECAR.md](sources/01-EVIDENCE-SIDECAR.md).

## Included sources (privacy-edited copies; illustrative origins and historical original SHA256 in PROVENANCE.json)

- [sources/01-EVIDENCE-SIDECAR.md](sources/01-EVIDENCE-SIDECAR.md) — original contract
  proposal (2026-09-19). Its acceptance matrix E01–E10 is **PROPOSED, NOT RUN**.
- [sources/01-SIDECAR-OWNER-PACKET.md](sources/01-SIDECAR-OWNER-PACKET.md) — canonical
  owner disposition packet with rows R01/R02/R03/R08/R09 and their suppliers.
- [sources/R09-CONDITIONAL-PROPOSAL.md](sources/R09-CONDITIONAL-PROPOSAL.md) — R09
  conductor contribution R1 (2026-09-25): proposed future bounded fence, NOT BUILT.
- [sources/R09-CONDITIONAL-PROPOSAL-R2.md](sources/R09-CONDITIONAL-PROPOSAL-R2.md) — R2
  delta: exact absolute-path digest-bound legacy-reader loading proposal; supersedes
  only R1's unspecified loading mechanism and command cwd binding. NOT IMPLEMENTED.
- [sources/R09-MUSE-DISPOSITION.md](sources/R09-MUSE-DISPOSITION.md) — Muse's 2026-09-25
  disposition: **conditional acceptance as documentary draft only** of R1+R2.

## Documentary status as of 2026-09-28

- R09 R1+R2 exist and were conditionally accepted **as documents** by Muse; the
  disposition explicitly authorizes nothing beyond the drafts' existence.
- The proposed candidate fence/loader mechanism remains **proposed and unrun**; the
  future workspace path named in R2 was checked absent at proposal time and this
  README does not create it.
- Fresh bounded intake on 2026-09-28 found **no new authentic owner/Muse return** for
  this contract since 2026-09-27 16:16 EDT (named contract roots + later shared
  handoffs only; bounded absence, not omniscience).

## Held input rows and owners (values unchosen; no answer implied)

From the canonical owner packet — each row names its supplier, none is answered here:

- **R01** — Muse semantics + source owners: local profile/formats/selectors, typed
  identity/statement comparison, runner/count semantics with documentation.
- **R02** — Muse disclosure + source/run owners: permitted diagnostic/label/quote
  disclosure disposition and authorized custody references.
- **R03** — source/run owners: selected run root/round, immutable packet, sole
  correction registry and runner-witness pins.
- **R08 (01 portion)** — admission owner/delegate: finite applicable limits with units.
- **R09 execution prerequisites** — conductor supplies fence/reviewer inputs; Muse's
  separate build-fence approval would be required **if implementation is ever sought**;
  a Kevin-only stop applies where a witness depends on Kevin's exact action.

## Controls and evidence limits

- E01–E10 acceptance controls are proposed/unrun; historical legacy test files are
  cited as **source references only**, not evidence the proposed module works.
- The B5-A scope correction is selected; mirror/node-b scope needs separate owners.
- Privacy proposal 05 (B7) is a separate unapproved decision and is **not** included
  in this draft or treated as a dependency.
- For a GitHub-only reader: evidence lineage older than the included files
  (overnight revisions, option selections, reuse maps) is **external and unverified**
  inside this PR; the included documents cite it, this package does not contain it.

## Independent review limitations

Blue's independent review of R1+R2 returned PASS **for conditional documentary
acceptance only**; it verified hashes, cited source lines, and non-execution. The R1
PARTIAL finding is preserved in the record. Prior involvement is disclosed where it
exists; reviews are independent, not blind. No review executed code, fixtures, or a
runtime check, and none is claimed.

## Requested Muse review

Document-only: confirm the record as packaged (proposal, owner packet, R09 R1/R2
contribution, conditional acceptance) and disposition next steps through the existing
R01/R02/R03/R08 rows and any future explicit, authenticated fence decision. No
cross-contract acceptance follows: this acceptance does not cover Facts04, Delivery03,
or Capacity02. All standing holds (P1 Muse-first, B1/M1, G3, B7, B9, D4/D7, S3,
E14+/I1a–5a) are unchanged.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
