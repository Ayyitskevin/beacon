# Operator throttle slow-CPU evaluation — 2026-08-28

Scope: the public `OperatorAccountService::authenticate_password` seam under CPU throttling.
The evaluation exercises the real Argon2 verifier and the persisted password failure counter; it
does not mock time or credential verification.

## Security basis

- NIST SP 800-63B-4 requires verifiers to apply rate limiting after failed authentication attempts
  and to reset the failed-attempt count after successful authentication:
  <https://pages.nist.gov/800-63-4/sp800-63b.html#rate-limiting-throttling>.
- OWASP recommends login throttling and increasing delays after failed attempts:
  <https://cheatsheetseries.owasp.org/cheatsheets/Authentication_Cheat_Sheet.html#login-throttling>.
- Rust documents `SystemTime` as a wall-clock time source that is not monotonic. Beacon retains it
  because retry deadlines are persisted across process restarts; this repair changes when the time
  is sampled, not the clock or persistence model:
  <https://doc.rust-lang.org/1.97.1/std/time/struct.SystemTime.html>.

## Reproduction and repair proof

Focused command, with `<quota>` replaced by a percentage:

```sh
systemd-run --quiet --user --wait --collect --pipe \
  -p CPUQuota=<quota>% \
  --working-directory=/example/checkouts/beacon \
  cargo test -p beacond --test operator_account \
  repeated_password_failures_are_throttled_and_success_clears_the_counter \
  -- --exact --nocapture
```

| CPU quota | Before repair | After repair |
|---:|---|---|
| 100% | Pass | Pass |
| 80% | Pass | Pass |
| 60% | Pass | Pass |
| 50% | Pass | Pass |
| 45% | Pass | Pass |
| 40% | Pass | Pass |
| 35% | Pass | Pass |
| 30% | Pass | Pass |
| 25% | **Fail**: valid retry accepted immediately | Pass |
| 20% | **Fail**: valid retry accepted immediately | Pass |
| 15% | **Fail**: valid retry accepted immediately | Pass |
| 10% | **Fail**: valid retry accepted immediately | Pass |

The 20% condition was also used for a revert check in one working tree:

1. Unfixed source: expected assertion failure observed.
2. Repaired source: pass observed.
3. Repair deliberately reverted: the same assertion failure returned.
4. Repair restored: pass observed again.

The assertion is at the public service seam: after the first failed password attempt, an immediate
correct-password attempt must return `AuthenticationThrottled`. No production threshold was
changed to obtain the result.

## Adjacent regression coverage

```sh
cargo test -p beacond --test operator_account
cargo test -p beacond --lib
```

Observed: 9/9 operator-account integration tests and 28/28 beacond library tests passed. The final
whole-repository gate, independent review, and hosted CI belong to the exact branch handoff; this
evaluation does not authorize merge or deployment.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
