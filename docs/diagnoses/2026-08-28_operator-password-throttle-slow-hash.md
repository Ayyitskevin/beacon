# Password throttle deadline could be consumed by slow verification

Date: 2026-08-28

Affected surface: `OperatorAccountService::authenticate_password`

Disposition: repaired on a security-review branch; merge and deployment remain separately gated

## Symptom

Beacon's existing integration test requires the first valid retry immediately after a failed
password attempt to be throttled. It passed on ordinary hardware but failed deterministically when
the test process was limited to 25% CPU or less. Under the failing conditions, the Argon2 password
verification lasted longer than the one-second first retry delay.

## Root cause

`authenticate_password` sampled `now` before loading the account and running Argon2 verification.
On failure it passed that stale timestamp to `register_failure`, which persisted
`password_retry_after_ms = now + delay`. Expensive verification could therefore consume all of the
delay before the failure was recorded and returned to the caller.

The same stale-time calling convention existed for Recovery Key failures. Recovery verification is
normally fast, but it had the same incorrect ordering if the thread was suspended between sampling
time and registering failure.

## Repair

`register_failure` no longer accepts a caller timestamp. After it begins the immediate transaction
and reads the prior counter, it samples `current_time_ms()` itself, increments the counter, computes
the retry deadline, records the security event, and commits. Both password and recovery failures
use this invariant.

This is intentionally narrow:

- the first delay remains one second and the exponential schedule is unchanged;
- the 100-attempt disablement is unchanged;
- counters and deadlines remain persistent and separated by credential kind;
- same-machine-only password and recovery access is unchanged;
- generic authentication errors and successful-authentication counter resets are unchanged;
- the database schema, dependencies, and public API are unchanged.

## Why this clock remains correct for the slice

Beacon stores retry deadlines so they survive process restarts. Rust's `SystemTime` represents wall
clock time and is explicitly not monotonic, while a process-local monotonic instant cannot be
persisted with the same meaning. The defect was sampling order, not a newly discovered requirement
to redesign persisted-clock handling:
<https://doc.rust-lang.org/1.97.1/std/time/struct.SystemTime.html>.

## Verification

The full matrix and reproducible command are in
[`docs/evals/2026-08-28-operator-throttle-slow-cpu.md`](../evals/2026-08-28-operator-throttle-slow-cpu.md).
Before repair, quotas of 25%, 20%, 15%, and 10% failed; after repair, all twelve tested quotas from
100% through 10% passed. A deliberate revert restored the failure and reapplying the repair restored
the pass.

Standards context: NIST treats throttling as a response to failed authentication attempts, and
OWASP likewise describes delay/lockout behavior following failed logins. Those sources support
anchoring the deadline to failure handling rather than to the start of credential verification:
<https://pages.nist.gov/800-63-4/sp800-63b.html#rate-limiting-throttling>,
<https://cheatsheetseries.owasp.org/cheatsheets/Authentication_Cheat_Sheet.html#login-throttling>.
