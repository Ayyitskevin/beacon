# 04 — Local facts display relative to immutable baseline membership — draft PR README

This is a fleet-workflow review packet, not an adopted Beacon product interface,
ADR, domain definition, or implementation plan. Excluding another proposal from
this packet does not resolve any dependency or standing hold.

**REVIEW PACKET: DRAFT / NOT ACCEPTED / NO IMPLEMENTATION.** This folder packages existing, already
written documents for Muse's **document-only** review. Nothing here is another
authority store, a time-based "latest" view, a build/adoption grant, or an execution
readiness claim. Semantics live only in the copied sources; this README reports
status and never changes meaning.

**Historical-status note.** Status lines inside the copied sources (for example
"BLOCKED — owner returns missing", "documentary review and Muse gate pending")
describe their own freeze dates. The current status is only what this README states
below; copied text is privacy-edited; its status statements are historical and reads as history.

## Problem and previously proposed scope

The existing fleet workflow proposal addresses a local display of observations
relative to explicit baseline membership, without assigning time authority. The selected scope is one **read-only, local** module over a
pinned checkpoint plus an explicit baseline-membership artifact: closed
obligation/review payloads only (B2-A), immutable set membership (B3-A), exact typed
comparison, preserved provenance, and explicit UNKNOWNs — no `value:any` union, no
timestamp-elected winner, no readiness/authority/dispatch/publication result. "Newly
seen" never means temporally newer; a missing baseline is UNKNOWN, not empty.
Full text: [sources/04-LOCAL-FACTS.md](sources/04-LOCAL-FACTS.md).

## Included sources (privacy-edited copies; illustrative origins and historical original SHA256 in PROVENANCE.json)

- [sources/04-LOCAL-FACTS.md](sources/04-LOCAL-FACTS.md) — original contract proposal
  (2026-09-19). Acceptance matrix F01–F10 is **PROPOSED, NOT RUN**.
- [sources/04-FACTS-OWNER-PACKET.md](sources/04-FACTS-OWNER-PACKET.md) — canonical owner
  disposition packet with rows R04–R09 and their suppliers.
- [sources/FACTS04-R09-PROPOSAL.md](sources/FACTS04-R09-PROPOSAL.md) — R09 conductor
  contribution (2026-09-25): proposed future author fence, exact legacy source
  references with their limits, and candidate-verification obligations, all
  **NOT RUN / NOT CREATED**.

## Documentary status as of 2026-09-28

- The R09 contribution exists with an independent Blue PASS; **Muse's separate Facts
  R09 documentary disposition is still pending** — that is the exact missing input
  for this contract (proposal SHA256 `9ada0f17…485d0`). Sidecar01's R09 conditional
  acceptance expressly does **not** cover Facts04.
- The proposed prototype root named in the R09 proposal was checked absent at
  proposal time; this README does not create it. Legacy helpers/runners remain
  **source references only** — never imported, executed, replayed, or retrofitted;
  Delivery03's retired collector is excluded from any reuse.
- Historical F04/F07 synthetic demonstrations are bounded: structural retention and
  mismatch refusal only; they do not demonstrate authentic withdrawal, the proposed
  CONFLICT output, or same-key provenance aggregation.
- Fresh bounded intake on 2026-09-28 found **no new authentic owner/Muse return** for
  this contract since 2026-09-27 16:16 EDT (named roots + later shared handoffs only).

## Held input rows and owners (values unchosen; no answer implied)

From the canonical owner packet — each row names its supplier, none is answered here:

- **R04** — Muse semantics + source owners: local profile, supported
  formats/selectors/adapters, relationship/typed-comparison meanings, exact
  result-envelope/error behavior.
- **R05** — Muse disposition + source/run owners: baseline custody/coverage and the
  separately pinned selection artifact with declared coverage.
- **R06** — source/run owners: pinned checkpoint/packet/head and observation
  references with provenance/version bindings.
- **R07** — Muse disclosure + source/run owners: permitted output-field/label/quote
  disposition and approved custody references.
- **R08 (04 portion)** — admission delegate: finite applicable limits with units;
  historical parser limits/timeouts are NOT defaults.
- **R09 execution prerequisites** — actual appointments, exact candidate/dependency
  identities, disjoint fences, confinement method, and an explicit authenticated
  Muse build-fence decision. A Kevin-only stop applies where an instruction or
  intended withdrawal is claimed.

## Controls and evidence limits

- F01–F10 acceptance controls are proposed/unrun; the R09 candidate-verification CLI
  is a proposed interface with **no implementation**.
- Confinement is unresolved: source/input snapshots alone do not prove absence of
  transient/off-fence writes, network, or unintended processes.
- Proposal 05 (B7 privacy) is a separate unapproved decision and is **not** included
  or treated as a dependency; compiler comparison belongs solely to proposal 02.
- For a GitHub-only reader: pre-included-file lineage (R2 revisions, B2/B3 option
  selections, overnight build experiments and their manifests) is **external and
  unverified** inside this PR.

## Independent review limitations

Blue's independent review of the R09 contribution returned PASS as a **non-author,
not blind** documentary review (Blue previously reviewed legacy Facts; Astra drafted,
helm integrated; actual replies are preserved in the source root's returns record).
The review verified hashes and source bindings; it executed no code, fixtures, or
runtime check.

## Requested Muse review

Document-only: disposition the Facts R09 contribution (SHA256 `9ada0f17…485d0`) and
confirm next steps through the existing R04–R09 rows. Documentary acceptance cannot
close implementation prerequisites or authorize code, adoption, or real-data
collection. No cross-contract acceptance follows for Sidecar01, Delivery03, or
Capacity02. All standing holds (P1 Muse-first, B1/M1, G3, B7, B9, D4/D7, S3,
E14+/I1a–5a) are unchanged.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
