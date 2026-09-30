# 02 — Advisory-local historical capacity (CAP-A) — draft PR README

This is a fleet-workflow review packet, not an adopted Beacon product interface,
ADR, domain definition, or implementation plan. Excluding another proposal from
this packet does not resolve any dependency or standing hold.

**REVIEW PACKET: DRAFT / NOT ACCEPTED / NO IMPLEMENTATION.** This folder packages existing, already
written documents for Muse's **document-only** review. Nothing here is a live census,
lease action, pilot admission, fixture generation, or owner conclusion; B7 code
remains HOLD. Semantics live only in the copied sources; this README reports status
and never changes meaning.

**Historical-status note.** Status lines inside the copied sources (for example
"BLOCKED — named owner inputs missing", "PARTIAL / source-reference return",
"conditionally accepted as documentary proposal") describe their own freeze dates.
The current status is only what this README states below; copied text is preserved
unchanged and reads as history.

## Problem and previously proposed scope

The existing fleet workflow proposal compares supplied historical compiler-resource
records without introducing a live clock, process census, or issuer. The selected scope is
one **read-only advisory** module: `reconcile(supplied_selection, comparison_policy)
-> advisory_report` over explicitly supplied, sanitized artifacts, returning
`agreed / conflicting / unknown` with safe references and named gaps, and
**live_freshness always UNKNOWN**. CAP-A historical-only is selected; CAP-B/C and
their suggested freshness TTLs are unselected; no current-owner, free/ready, or
exclusive-custody result exists. Full text:
[sources/02-HISTORICAL-CAPACITY.md](sources/02-HISTORICAL-CAPACITY.md).

## Included sources (privacy-edited copies; illustrative origins and historical original SHA256 in PROVENANCE.json)

- [sources/02-HISTORICAL-CAPACITY.md](sources/02-HISTORICAL-CAPACITY.md) — original
  contract proposal (2026-09-19). Acceptance rows CAP.01–CAP.10 are **PROPOSED,
  NOT RUN**.
- [sources/02-CAPACITY-OWNER-PACKET.md](sources/02-CAPACITY-OWNER-PACKET.md) — canonical
  owner disposition packet with rows S/R/C/T/O/B/L and five exact owner questions.
- [sources/LEGACY-INPUT-RETURN.md](sources/LEGACY-INPUT-RETURN.md) — conductor's L
  source-recipe return (2026-09-23): exact existing dispatch/checkpoint fixture
  recipes and literal assertions; **no fixture bytes generated, nothing run**.
- [sources/OPTION-A-AND-CAPACITY-L-DISPOSITION.md](sources/OPTION-A-AND-CAPACITY-L-DISPOSITION.md)
  — helm overlay (2026-09-23): L's independent non-author review (documentary
  correctness PASS / scope PASS) **with the preserved eight-row erratum** and failed-
  command history; L remains PARTIAL awaiting Muse's documentary gate.
- [sources/MUSE-DISPOSITION.md](sources/MUSE-DISPOSITION.md) — Muse's 2026-09-26
  Capacity02-S disposition: **conditional acceptance of the S proposal as a document
  only**; S admission, pilot, appointments, implementation remain BLOCKED.

- [S conditional proposal](sources/CAPACITY02-S-CONDITIONAL-PROPOSAL.md) — the exact
  proposal bound by the included S document-only acceptance; operational holds remain.

## Documentary status as of 2026-09-28

- S is accepted **as a document only**; that acceptance admits nothing operational.
- The L return exists and passed independent documentary review (erratum preserved);
  **Muse's separate L documentary disposition is still pending** — that is the exact
  missing input for this contract (return SHA256 `70ce10fd…f444`). The L return
  identifies recipes; it is not materialized fixture bytes, and nothing may be
  materialized to "finish L" without an explicit later authorization.
- T requires a **caller-selected historical interval** plus owner-supported meanings;
  no interval is chosen here or anywhere in this package.
- Fresh bounded intake on 2026-09-28 found **no new authentic owner/Muse return** for
  this contract since 2026-09-27 16:16 EDT (named roots + later shared handoffs only).

## Held input rows and owners (values unchosen; no answer implied)

From the canonical owner packet — each row names its supplier, none is answered here:

- **S** — conductor under Muse pilot limits: separately authorized scope naming author,
  different reviewer, files, deadline, formats, finite fixture limits, permitted effects.
- **R** — Muse approval + resource owner: exact node-a compiler resource/host
  descriptor, approved aliases, exclusivity/cache/tree meanings.
- **C** — Muse + actual source/deployment owners: named custodians, issuers/observers,
  supported formats/scope, provenance records.
- **T** — caller interval + Muse profile + issuer/observer owners: historical interval,
  domains/epochs/units/precision, state/release/expiry/exclusivity relations.
- **O** — observer/issuer/deployment owners + Muse custody approval: sanitized
  presence/absence tuples with exact host/boot/PID/start and completeness declarations.
- **B** — Muse or delegated admission owner: finite bounds, typed domains, closed
  formats/selectors, privacy-safe ID policy.
- **L** — conductor supplies legacy pins; later builder/reviewer preserve public
  readers. Kevin-owned B7 owner/risk/privacy rulings require exact authentic human
  decisions; no worksheet or fixture bypasses them.

## Controls and evidence limits

- CAP.01–CAP.10 controls are proposed/unrun; the inherited CAPA design rows remain
  proposed — they are not an executed 39-case or 86-test claim. CAPA.09/12/14 subcases
  remain explicitly UNVERIFIED as recorded.
- Legacy test files cited by the L return are **source references only**; their
  synthetic literals are not real identities, security review, or runtime outcomes.
- Existing reader ceiling (16 MiB) is an upper ceiling, never a selected CAP allowance.
- For a GitHub-only reader: pre-included-file lineage (the CAP-A design package, its
  review, B7 review evidence, checkpoint seals) is **external and unverified** inside
  this PR.

## Independent review limitations

The L review returned documentary correctness PASS / scope PASS with **no actionable
finding**; it is non-author, not blind (historical seat involvement disclosed), and
its seven-vs-eight-row clerical erratum plus failed-command history are preserved in
the overlay rather than edited. The S review (Blue, non-author, disclosed prior
Capacity reviewing) verified hashes and fences only. No review executed fixtures,
tests, or runtime checks.

## Requested Muse review

Document-only: disposition the L return (SHA256 `70ce10fd…f444`) and confirm next
steps through the existing S/R/C/T/O/B rows and five owner questions. Documentary
acceptance grants no admission, pilot, appointment, fixture generation,
implementation, or execution. No cross-contract acceptance follows for Sidecar01,
Delivery03, or Facts04. All standing holds (P1 Muse-first, B1/M1, G3, B7, B9 parked,
D4/D7, S3, E14+/I1a–5a) are unchanged.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
