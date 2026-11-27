# I1 pilot authority and stop candidate

Status: implementation candidate for the I1 fixture contract.
Not a production schema, live admission, or full I1
integration. Existing Charter `no_spend` / `prohibited`, Proposal, Ruling and
Invocation bytes and semantics remain unchanged. No transport is added.

## Owners and unavailable joins

The singleton authenticated account supplies `owner`; there is no Operator UUID.
Installation scope is the closed `current_store` value, never a caller-selected
installation. Workspace/Project, current Charter/Policy, Resident lifecycle,
configuration and activation are checked by their existing domain owners in the
caller's transaction. A role label does not establish a Resident continuation.
Production Assignment/Invocation continuation resolution is absent at the base:
it refuses explicitly. Test provenance constructors and the fake custodian are
`cfg(test)` only. Descriptor identity proves bytes, not support or enforcement.

Workspace allocation must exist before its Project successor. Project publication
cannot invent its parent. An explicit successor names old Charter/Policy revisions;
it does not turn the old prohibition into permission. The separately admitted
production Charter/schema integration remains owed before any effect is possible.
The seven ADR0034 constraint families plus the custody descriptor below must be resolved by owners before
production use; old Ruling v1 is not dispatchable and no competing Ruling is added.

## Canonical primitives and refusal

The three authority/intent grammars below are binary canonicalizers, SHA-256 over
exactly their bytes. The later observation records use explicitly named strict JSON.
The digest is never a second independent caller argument alongside parsed data.
Literal ASCII magic ends in NUL. UUID = 16 raw bytes; digest = 32 raw bytes;
`u32`/`u64` = unsigned big-endian. A string is `u64` UTF-8 byte length followed
by those bytes. A list is `u32` count followed by its entries. Optional digest
is `00` absent or `01` plus digest. No other tags are accepted. No normalization,
sorting, deduplication, fallback, or ignored trailing fields. Whole-record maximum
64 KiB; counts/lengths are checked against remaining bytes before allocation.
Nil UUIDs, zero revisions, non-increasing expiry, unknown purposes/roles, and
duplicates or non-canonical ordering refuse. Non-sensitive synthetic inputs only.

`Units` has exactly these twelve `u64`s, in order: requests, input tokens, output
tokens, CPU milliseconds, memory bytes, disk bytes, inodes, capture bytes,
wall milliseconds, paid USD micros, request bytes, response bytes. Paid USD micros
must be zero. Request bytes, response bytes and capture bytes are independent
dimensions, each reserved per operation and cumulatively at every allocation
level; none is inferred from token count or another byte dimension. Capture
accounts retained evidence bytes, not an implicit substitute for wire bytes.
Units are independent,
never converted or summed across kinds. Bounds are explicit even when zero.
Reservations are conservatively cumulative in all twelve dimensions; this first
slice does not implement reusable peak-memory accounting. Proven actual use may
replace a reservation; unknown use retains it. Checked arithmetic refuses overflow.

`StopPlan` is four `u64` stage caps: soft stop, escalation, physical observation,
retention, in milliseconds. Each must be positive and their checked sum must be
strictly below the operation wall ceiling. No unbound default supplies a limit.

`Descriptor` is tag `00` not declared, or `01` followed by kind:string,
version:u32, canonicalizer:string, digest. Tokens contain only lowercase ASCII
letters, digits, `.` and `-`, 1–64 bytes; version is positive. An inert grammar
may retain a bound identity without claiming its owner understands it.

### Workspace allocation: `beacon.pilot.workspace.v1`

In order: magic `beacon.pilot.workspace.v1\0` (26 bytes); installation tag `01`
(`current_store`); Workspace UUID; predecessor optional digest; issued-at UTC
milliseconds:u64; expires-at UTC milliseconds:u64; Units. Publication derives the
actual authenticated Owner and current store. Successor order is explicit, not
timestamp-wins. Expiry is exclusive. Backward wall-clock observations refuse;
monotonic useful-work deadlines are separately established before setup.

### Project successor: `beacon.pilot.authority.v1`

In order: magic `beacon.pilot.authority.v1\0` (26 bytes); installation tag `01`;
Workspace UUID; Project UUID; Charter revision:u32; Policy revision:u32;
Workspace-allocation digest; predecessor optional digest; issued-at:u64;
expires-at:u64; pause generation:u64; posture tag `01` (`supervised`);
three actor entries, in fixed Lead/Builder/Reviewer order; Project Units;
Project-preparation Units; Mission-allocation list; eight Descriptors in the
order execution-policy, sandbox, network, model-provider, command-allowlist,
tool-allowlist, evaluations, custody; readable-path list; writable-path list.

An actor entry is Resident UUID, configuration UUID, activation UUID. All three
Resident IDs are distinct. Role is position, not a claim that a caller holds it.
Mission allocation is Mission UUID plus Mission Units plus three Resident Units
in the same role order. Mission IDs are strictly ascending raw UUID bytes.
Preparation plus every Mission ceiling must fit Project; all Resident children
sum within their Mission; Project fits the stored Workspace parent. Bootstrap
uses the explicit Project-preparation allocation, no fabricated Mission. Its
charge stays at Project/Workspace exactly once after actual Mission creation.

Paths are finite, nonempty, repository-relative UTF-8 POSIX paths: no slash prefix,
backslash, NUL, empty/`.`/`..` component or wildcard (`*`, `?`, `[` or `]`). Lists
are strictly ascending raw UTF-8 bytes, maximum 256 entries each. Readable and
writable scopes are separate. They are declarations, not filesystem handles or
evidence of containment. Source/base/target identity and exact commands belong
inside the understood resource descriptors, not a generic executable string.

### Operation intent: `beacon.pilot.operation.v1`

In order: magic `beacon.pilot.operation.v1\0` (26 bytes); operation UUID;
authority digest; Workspace UUID; Project UUID; scope tag `01` Project preparation
or `02` Mission followed by Mission UUID; role tag `01` Lead, `02` Builder,
`03` Reviewer; Resident UUID; configuration UUID; activation UUID; Assignment
tag `00` absent (Project bootstrap only) or `01` plus UUID; parent Invocation
digest; purpose tag; input digest; reservation Units; StopPlan.

Purpose tags: `01` Lead bootstrap, `02` Lead Mission planning, `03` Builder
candidate, `04` Reviewer evaluation. Project scope admits only `01` and Lead;
Mission scope requires an actual Assignment and the matching role for `02`–`04`.
These intents authorize nothing: production continuation and descriptor joins
remain unavailable. A caller role/ID cannot construct verified provenance.
Preparation/check/final dispatch purposes are deliberately unsupported here;
they need the owning purpose-specific Ruling integration, not a guessed tag.

## Admission and stop semantics

One outstanding operation, unique operation claim, zero automatic retries. Claim
and worst-case reservation precede possible send. Recheck exact current authority,
scope, resolved actor, descriptor support, allocation, expiry, pause generation,
and deadline at the simulated act seam. A cached admission is not a permit.
Pause increments generation and durably commits before fake interruption. If Pause
wins, no claim/launch; if claim wins, Pause includes that outstanding operation.

Revocation is serialized with claims in the same store write-transaction domain.
Its immutable `pilot.authority_revoked` record names the exact authority digest
and the sorted IDs of every outstanding operation under it (at most one in this
serial pilot). The record commits before any fake interruption request: it is
the durable selection of those operations into stopping, not merely a flag that
blocks later admissions. If revocation wins, claim/launch refuses; if claim wins,
revocation includes that operation and the launch seam refuses useful work.
Reservations and resource/target custody remain encumbered until positive physical,
capture and usage settlement; revocation never refunds, deletes or silently closes
them. Already-authorized bounded stop/observation/retention duties remain available.
No real interruption adapter exists in I1. Production revocation-to-custodian
wiring remains unimplemented; the fixture proves only the durable ordering seam.

Management terminal, physical completion, durable capture and measured usage are
independent observations. Disconnect, timeout, session lock, lease expiry and
restart are not physical exit. Unknown delivery/exit/capture/usage stays HOLD with
reservation and target/resource fence retained. Late positive settlement records
evidence without reviving the old attempt. No refund without proven use.
Owner acceptance of uncertainty records review only and does not clear custody.
Only authenticated Owner resume is possible; shared unknown custody still refuses.
Restart never resends and requires a new monotonic epoch before new useful work.

Stage launch requests specify a cap. Refuse when it exceeds the stage allocation
or remaining monotonic time minus all later reserves. Expiry closes useful work,
not the already-authorized bounded stop/observation/retention obligation.

## Fixture-only persistence authority and production boundary

The event-kind extension is fixture-only.
The unnumbered SQL fixture may rebuild the event-kind CHECK only on a disposable
v37 test store, retaining
all original rows, links, kinds and triggers. The new closed kind allowlist is
`pilot.workspace_published`, `pilot.authority_published`, `pilot.authority_revoked`,
`pilot.operation_claimed`, `pilot.operation_observed`, `pilot.uncertainty_reviewed`.
Fixture installation is compiled only under `cfg(test)`. Existing event-chain
append/verify and transaction owners remain the only audit chain. No existing
kind is overloaded. Production SQL, startup, SCHEMA_VERSION and numbered migration
ladder are untouched. Production new-record startup remains unsupported; a
fixture reopen is not a production startup claim.

### Inert journal-record canonical forms

Three additional finite records use compact UTF-8 JSON with no whitespace or
trailing newline. Key order is exactly the order below; unknown/missing/duplicate
keys, alternate key order or spelling, and non-canonical values refuse on exact
parse/re-serialization comparison. Integers are unsigned decimal without leading
zeroes. Booleans are `true`/`false`; unknown usage is an explicit closed state,
never null or an absent key. Digests are arrays of
exactly 32 integers in 0–255; UUID strings use lowercase hyphenated form. A reason
is 1–1024 printable ASCII bytes, not all spaces; quote/backslash use `\"`/`\\`.
Each canonicalizer field has the exact string below and version must be `1`.

- `beacon.pilot.revocation.v1`: keys `canonicalizer`, `version`, `authority`
  (digest), `reason`, `stopping` (0–1 operation UUIDs, strictly ascending bytes).
  The stop set must equal the operations still outstanding under this authority
  at the serialized revocation boundary, not a caller-selected subset.
- `beacon.pilot.observation.v1`: keys `canonicalizer`, `version`, `operation`,
  `ordinal` (positive contiguous u32, per operation), `management_terminal`,
  `physical_complete`, `capture_complete`, `usage`. The exact closed usage forms
  are `{"state":"unknown"}` and `{"state":"measured","units":[...]}` in that
  key order. `units` is an array of exactly twelve unsigned JSON integers in
  0–18446744073709551615, ordered: requests, input tokens, output tokens, CPU
  milliseconds, memory bytes, disk bytes, inodes, capture bytes, wall milliseconds,
  paid USD micros, request bytes, response bytes. Paid USD micros (index 9) is
  exactly zero; request attempts (index 0) is exactly one for an observed claim.
  There is no unknown-to-zero conversion. Unknown carries no units member;
  measured requires it. Null, missing/extra keys, unknown states, a units member
  on unknown, omitted units on measured, floats, negatives, overflow, wrong array
  length or positive money are refusals, not alternate spellings.
  Positive observations cannot regress; measured use must fit the reservation,
  includes the permanently consumed request attempt, and is not transferable to
  another operation. Closure needs physical completion, capture and known usage;
  a management terminal flag alone never closes. An incomplete record is retained.
- `beacon.pilot.uncertainty.v1`: keys `canonicalizer`, `version`, `operation`,
  `reason`. Authenticated Owner acknowledgment only; no settlement or refund.

The existing event-chain canonicalizer remains the only audit chain. Its payload
binds record kind, canonical digest/length, Workspace/Project, owner basis and
time; immutable normalized rows must agree in both directions. `owner_basis`
names the originating policy Owner, not a fabricated Operator author for Resident
work; the operation retains its actual Resident binding, unresolved in production.
This slice reconciles at most 256 pilot records / 64 KiB per record, 10,000 total
events / 16 MiB total event payload; exceeding an intake bound refuses, never
truncates. These are parser bounds, not runtime allocations.

## Independent vectors / coverage

`crates/beacond/tests/fixtures/i1/workspace.hex` is hand-transcribed from the field
table, not generated by the Rust encoder. It uses Workspace `11` repeated 16
times, no predecessor, issued 1, expires 100; Units 1,2,3,4,5,6,7,8,100,0,9,10.
Reproduction uses only `xxd -r -p` and `sha256sum`; the eventual test compares
literal bytes and independently pinned identity, not encoder-vs-itself.
The Project vector has no Missions, all eight descriptor slots explicitly
undeclared, readable `README.md`, no writable path; actor UUIDs use repeated
`33/44/55`, `66/77/88`, `99/aa/bb` bytes. It is an inert declaration, not admission.
The operation vector is Project-scoped Lead bootstrap with operation `cc`, claimed
authority `dd`, parent Invocation `ee`, input `ff`, the same Lead binding, Workspace
`11` and Project `22`; reservation 1,2,3,4,5,6,7,8,100,0,9,10; stop caps 1,2,3,4.
Those cross-record references are intentionally opaque identities, not a claim
that the three illustrative records form an admitted execution chain.

| Literal file | Decoded bytes | SHA-256 of decoded bytes |
|---|---:|---|
| workspace.hex | 156 | 0b2e4324709248b9505f54e5b8516638941c0e90ec0e87f5c443448466333030 |
| authority.hex | 498 | d9a92e8f186f4dc20ddaf2e6bd9004094fd11aecb76abbd17250e78109def591 |
| operation.hex | 350 | d4b7bb2993d4b09ff1b3f49e2f689d31aab1e8e9470b721bf0d74947a249a6dd |

Hashes obtained by `xxd -r -p <literal-file> | sha256sum`, without running candidate
code. Acceptance-to-test mapping is retained before source freeze.
Strict-JSON literals in the same directory exclude their final file newline from
canonical bytes (`tr -d '\n' < <file> | sha256sum` on these one-line fixtures):
`revocation.json` e129cd350f6a2aa89eecefcd131f0152c46374c0a1675fd92304dcb3324f2393;
`observation.json` 44eae7cf94ce79f6c2a8cf3b72ca99736b0b1a7951680932db58264353a75a12;
`observation-measured.json` 2aa7112072638f53558dd9dd60e393a4988e4b4662926361475e69d0669a81c4;
`uncertainty.json` 1bd7ddbb9614c939a784592a839c9831bd4f84400e6568c9f7024b4788cd193b.
The first names digest byte17 repeated, reason `fixture`, one `cc` operation;
the unknown observation independently demonstrates manager-terminal with physical
exit false, capture true and explicit usage state unknown. The measured observation
has physical/capture true and units 1,2,3,4,5,6,7,8,100,0,9,10, not zero usage;
the uncertainty literal acknowledges that operation only.
No literal claims to be a production authority or a complete execution chain.
All runtime results are UNRUN until the centrally recorded invocation receipts exist.
