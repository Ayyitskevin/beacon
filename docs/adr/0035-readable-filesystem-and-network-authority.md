---
status: proposed
---

# Bind readable-filesystem and network authority before exposing execution

This proposal is written against exact Beacon base
`0a30798a58349e1fc338ababe0b107af97eb567f`. It records the design that the
current static evidence supports and names the runtime evidence that is still
missing. It is not accepted, grants no authority, and authorizes no probe or
production change.

## Context

Beacon's execution boundary currently composes several mechanisms that are
individually useful but do not yet satisfy the Blueprint's complete Run-I/O
contract.

The Landlock ruleset grants execute, file-read, and directory-read rights at
`/`, then grants write rights beneath the disposable worktree. Its network mask
governs TCP bind and connect only. It neither requests Landlock's newer UDP
rights nor implements the Blueprint's named-destination policy. A hostile
process that ever becomes reachable could therefore read host-accessible data
and may be able to send it over UDP. The ruleset's write containment does not
make those read and network gaps safe.

The reachability boundary must also be stated precisely. There is no production
transport or desktop command for `DeclaredCheckService::execute`; current
Declared Check execution is backend-only. Operator-reachable Check Preflight is
different: the desktop command creates a preflight Run and calls
`sandbox::detect()`. Detection opens an ephemeral loopback TCP listener and
forks a child that applies Landlock. Preflight does not run a Declared Check,
but it is not process-free or listener-free.

ADR 0034's `readable_context` is a digest-bound declaration of intended input.
It is not a grant. A future Ruling must select the exact readable authority for
one exact Run; execution may not infer authority from a Proposal, repository,
toolchain, or ambient host state.

## Threat model

The protected assets are host files outside the Run's declared inputs,
credentials and private material reachable through those files, network
destinations outside an accepted Ruling, and truthful capability and Receipt
records. The relevant threats are information disclosure through ambient reads,
exfiltration through an ungoverned protocol, symlink retargeting between review
and use, a child inheriting more authority than its Run, and a failed probe being
reported as positive enforcement.

The safety boundary is fail-closed. Missing capability, an inconclusive probe,
an unknown right, an unresolvable descriptor, or an unavailable network
mechanism refuses execution. None is repaired into a weaker grant.

## Static evidence

The citation-complete record is
[`2026-08-30-run-io-boundary-probes.md`](../evals/2026-08-30-run-io-boundary-probes.md).
Its load-bearing findings are:

1. both ruleset builders grant `READ_RIGHTS` at `/`; narrowing that grant is
   inseparable from changing the probe's `read_system_path` assertion;
2. the governed network mask contains only TCP bind and connect;
3. execution Receipt v2 adds its UDP uncertainty only after at least one check
   started, while a verified preflight adds the same uncertainty unconditionally;
4. sandbox capability is a closed typed value, not a boolean, and only
   `Enforcing` admits execution;
5. production working-directory resolution uses descriptor-bound `openat2` and
   `O_NOFOLLOW`, while the generic probe grant opens a path without
   `O_NOFOLLOW`; and
6. the probe scores `socket()` failure as refused TCP even though no Landlock
   connection check occurred. Whether that can make `Enforcing` reachable is
   unverified and remains a candidate defect.

## Proposed decision

### Readable-filesystem authority

Every executable Run must receive an explicit, immutable readable-file grant
set bound by its accepted Ruling. The set is assembled from two separately
identified sources:

- the Proposal's exact readable-context entries selected by the Ruling; and
- a versioned runtime/toolchain manifest containing only the executable,
  loader, libraries, configuration, and other support paths proven necessary
  for that Run kind.

The Proposal declares the first set but grants neither set. The runtime manifest
is Beacon-owned content with its own digest and version; it is not an ambient
allowlist and cannot expand after the Ruling. The exact minimum manifest is an
open measurement item, U4. Until U4 has a separately approved probe packet and
the manifest grammar is reviewed, execution remains unavailable rather than
falling back to reading `/`.

Every readable root is resolved from a trusted directory descriptor. The final
component is opened without following a symlink, and relative resolution uses
kernel-enforced containment rather than canonicalize-and-compare. Landlock
rules are added from the retained descriptors. A string path is display evidence,
not authority. A target that cannot be opened with the reviewed descriptor and
symlink semantics refuses the Run.

The ruleset is created for one Run and armed only in that Run's child. The
supervisor is never restricted. Restriction is inherited by every descendant,
and descendants receive no path around it through environment inheritance,
alternate working directories, or a second launcher. A future execution path
must probe the actual capability for that Run rather than reuse a preflight
Run's observation.

The filesystem capability must remain typed. A host that cannot prove the exact
handled-rights mask, readable roots, write roots, descendant inheritance, and
required runtime support reports an unavailable or inconclusive reason and does
not admit execution.

### Network authority

The current TCP-only Landlock mask is not the Blueprint's network authority.
The required contract is deny-by-default for every supported protocol, including
UDP, followed by grants only for destinations and operations named by the exact
Ruling. Unknown protocols and mechanisms that can express only a wider grant
remain denied.

This proposal deliberately selects no UDP or named-destination mechanism.
Landlock ABI availability must first be measured on every exact host in a
Product Steward-defined AppImage host matrix, and UDP behavior must be measured
with separately reviewed source. One kernel observation is evidence for one
host, not for a host class or an AppImage artifact. Any additional broker,
namespace, filtering, or operating-system mechanism requires its own reviewed
evidence. If a target host cannot prove the complete contract, Beacon reports
network authority unavailable and refuses execution; it does not silently fall
back to TCP-only containment.

### Capability and Receipt truth

`Enforcing`, `FilesystemOnly`, `Unavailable`, `ProbeRefused`, and
`ProbeInconclusive` remain distinct. A socket or listener that could not be
created is an inconclusive observation unless the enforcement operation itself
was reached and refused. A probe that did not answer is never an answer of no.

This proposal changes no Receipt version and does not reinterpret stored
Receipt v1 or v2 bytes. The conditional execution-v2 UDP gap, preflight's
current gap, capability serialization, and all existing strings remain as they
are. Any later change to their meaning or closed shape requires the separately
reviewed Receipt-version decision that ADR 0031 demands.

Future Receipts must name the exact capability evidence and Ruling-bound grant
identities actually used by that Run, along with every unverified property. A
configured rule, a Proposal declaration, or an earlier preflight observation is
not evidence that the execution Run enforced it.

## Evidence required before acceptance or implementation

The paired evaluation keeps U1-U7 explicit. It contains exact, not-executed P0
source for one-host ABI/right-mask observation. P1-P3 are design specifications,
not execution-ready probes: their source bytes and digests are intentionally
absent and require a later non-author review plus exact approval. U4, the minimum
runtime/toolchain readable set, is intentionally outside that planning record and
requires its own proposal. Probe results may revise this proposed decision; the
decision may not be fitted silently to a preferred result.

Before this ADR can authorize production work:

1. A non-author reviews the complete source bytes and digests for every probe to
   be run, and Kevin explicitly approves those exact bytes; a general request to
   investigate or approval of this ADR draft is insufficient.
2. The approved probes run once, preserve raw errors and cleanup evidence, and
   stop on every pre-stated unexpected result.
3. The proposed design is revised to match the observed evidence without
   weakening the Blueprint by implication.
4. A non-author reviews the exact ADR commit with re-runnable evidence.
5. Kevin explicitly accepts that exact commit as Product Steward.

Acceptance of this ADR would open only a later G2 implementation proposal. It
would not expose Declared Check execution, apply a live migration, grant a
network destination, accept ADR 0034's incomplete canonical bytes, or authorize
any Run.

## Smallest later implementation boundary

Production work is out of scope here. The G2 boundary includes any change that:

- narrows `READ_RIGHTS` or its root grant;
- changes the probe's system-read assertion or handled-rights mask;
- adds UDP or named-destination enforcement;
- changes `grant()` symlink behavior;
- changes socket-failure classification;
- changes or adds a launcher, descendant-inheritance boundary, inherited
  environment, or working-directory authority path;
- changes a capability variant, field, or serialized meaning;
- changes a Receipt gap, its conditionality, or its version; or
- exposes a Declared Check execution adapter.

Each is security or authority behavior and requires the accepted record,
tests that fail against the plausible wrong answer, non-author review, and the
separate human gate. No item is authorized by this proposed document.

## Consequences

The complete execution path remains closed while the evidence and design are
incomplete. Preflight may continue to observe the current TCP-only capability,
but its Receipt must retain the UDP limitation and must not be reused as proof
for a later execution Run. Existing backend-only Declared Check execution is not
made Operator- or Agent-reachable.

The design rejects a convenient but unsafe equivalence: write containment is not
read containment, TCP denial is not network denial, and declared input is not
authority. It also makes AppImage-host measurement part of the mechanism choice
rather than a packaging afterthought.

This proposal becomes accepted only with Product Steward sign-off on the exact
reviewed commit.
