---
status: accepted
---

# Enforce the execution sandbox with Landlock, and prove it before reporting it

Alpha 1 requires execution inside an operating-system sandbox that limits filesystem writes to
declared targets and denies network access by default. Beacon's current ruleset implements only the
filesystem-write and TCP portions; the remaining network gap is recorded below. This record fixes
the mechanism, and fixes the rule that Beacon never reports a sandbox it has not demonstrated.

The mechanism is Landlock, reached through the `libc` dependency the crate already carries: three
syscalls and one `prctl`. Beacon does not depend on `bubblewrap`, on a sandbox crate, or on
unprivileged user namespaces. Three facts decide this. Alpha's acceptance criteria require a
downloaded build to launch on a clean supported host, and a host is not obliged to have `bwrap`
installed. Unprivileged user namespaces are disabled by default on current distributions —
`kernel.apparmor_restrict_unprivileged_userns` is set on the machine this was measured on, and
`bwrap` is degraded there in consequence — so a namespace-based design would fail on exactly the
ordinary hosts the product targets. Landlock ABI 4 and later support restricting TCP bind and
connect, and Beacon's current ruleset requests those rights. That implements the filesystem
requirement and the TCP portion of the network requirement only. The ruleset does not request
[Landlock's UDP bind and connect-send rights][landlock-network], available from ABI 10, so UDP
remains unrestricted and residual; no named-destination enforcement exists.

A sandbox that is assumed is not a sandbox. Beacon therefore constructs its ruleset, applies it in a
child process, and has that child attempt a write inside a declared target, a write outside every
declared target, a read of a system path, and a TCP connection to a listener the parent opened. The
capability is reported only from what the child observed. This is not a test that runs in CI and
then is trusted forever: the probe runs per Run, because a cached capability is a claim about a
machine that may since have changed.

The reported capability is a typed value and never a boolean. `Enforcing` names the ABI and records
that the filesystem observations held and that the probe reported TCP refusal.
`FilesystemOnly` is a distinct capability for a kernel whose Landlock predates TCP restriction;
it never presents as `Enforcing`, and it is not sufficient to run a check, because the Blueprint
denies network by default and a weaker sandbox wearing the same name is the failure this design
exists to prevent.
`Unavailable` carries its reason, and two of those reasons must stay apart: `ProbeRefused` means the
syscalls reported success and enforcement did not hold, while `ProbeInconclusive` means the child
never reported. A probe that did not answer is not a probe that answered no.

Landlock is armed on the resolved capability alone and never on whether some other layer succeeded,
because a guard keyed on another layer's success disarms itself precisely in the degraded states
where it is the only remaining enforcement. Arming is the fail-closed direction.

Two properties of the mechanism shape the implementation and are recorded so they are not
rediscovered. `PR_SET_NO_NEW_PRIVS` and `landlock_restrict_self` cannot be undone for the calling
thread, and restriction is inherited by children. The supervisor therefore never restricts itself:
restriction happens between `fork` and `exec`, and the probe is a real child process because there
is no way to test enforcement in-process and recover from it.

## Consequences

`Verified` becomes reachable in Check Preflight. A binding that satisfies every verification step on
a host with an enforcing sandbox now reports that execution would be admissible, and its Receipt
records the ABI it was proven under rather than the literal `none`. Preflight's residual-uncertainty
list loses its sandbox line and keeps its reachability line; that difference is the visible evidence
this decision took effect.

The sandbox is a containment boundary and not yet a resource boundary. Landlock bounds no CPU,
memory, or wall-clock time, and the Blueprint's requirement that time, output, memory, and cost are
bounded is met by resource limits in a later slice. Until then a Receipt must not be read as
evidence that a Run was bounded. Beacon's current ruleset asks Landlock to restrict TCP bind and
connect but does not request the UDP bind and connect-send rights available from ABI 10. UDP
therefore remains unrestricted by this ruleset, so a Run could still emit UDP, and no Receipt may
imply otherwise. Hosts that are not Linux report `Unavailable`, and Execution refuses there; Alpha
1 names a Linux build, and this decision does not widen that.

[SECURITY.md](../../SECURITY.md) summarizes the same current limit. The older mechanism-level UDP
wording in [ADR 0031](0031-execution-receipt-v2.md) and
[ADR 0034](0034-immutable-proposal-v1-authority-bytes.md), plus the operator-facing Preflight and
Receipt text, predates this correction and remains deferred to the separately gated G2 slice.

Nothing is stored. A capability is detected at runtime, and no migration accompanies this record.

This proposal becomes accepted only with Product Steward sign-off on the reviewed branch.

[landlock-network]: https://www.kernel.org/doc/html/latest/userspace-api/landlock.html#network-flags
