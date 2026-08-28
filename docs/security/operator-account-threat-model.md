# Operator Account threat model

Status: Phase 0 implementation contract
Scope: one local Owner account, same-machine password fallback, offline Recovery Key, and the
desktop session lock

## Security boundary

`beacond` is the separate authority process for the account record, credential verifiers,
authentication throttling, and the live session. Tauri connects through a versioned Unix-domain
socket protocol protected by a 256-bit capability token; the runtime directory is owner-only and
the socket, token, and process lock are private files. A held kernel lock prevents a second
supervisor from replacing the live server's socket. The desktop never owns a database handle.
React receives only typed status and one-time setup or recovery output; it does not persist a
password, Recovery Key, verifier, IPC capability, or session secret. Authentication permits the
human to enter the Workspace surface. It never authorizes an agent effect or bypasses Policy and
Gates.

The first implementation is deliberately local-only. The password route is refused by design for
every future remote adapter. Passkeys remain the preferred account method, but Linux builds expose
them as unavailable until the platform has a trustworthy native authenticator adapter. Tauri uses
WebKitGTK on Linux, and WebKit's GTK/WPE WebAuthn implementation issue remains open as of this
decision; presenting the browser API as functional would be a false security claim.

Sources:

- Tauri's Linux WebView is WebKitGTK: https://github.com/tauri-apps/wry#linux
- WebKitGTK/WPE WebAuthn support issue: https://bugs.webkit.org/show_bug.cgi?id=205350
- WebKitGTK 2.52 release highlights (no WebAuthn support added):
  https://webkitgtk.org/2026/03/18/webkitgtk-2.52-highlights.html

## Credential rules

- A fallback password is 15–1,024 Unicode scalar values. Spaces and Unicode are accepted, no
  character-class composition rule is imposed, and input is never silently trimmed or truncated.
- Password verifiers use a unique salt and Argon2id with `m=19 MiB`, `t=2`, `p=1`, serialized as a
  PHC string so the algorithm and cost can migrate later.
- A Recovery Key contains 160 random bits, is displayed once in grouped Base32, and only a SHA-256
  verifier is stored. Formatting hyphens and ASCII case are ignored on entry. Successful recovery
  atomically invalidates the old key, installs the new password verifier, and returns a newly
  generated one-time key.
- Setup cannot commit without the Owner's explicit assertion that the Recovery Key was saved.
- Password and recovery failures use separate persistent counters and exponential delays. One
  hundred consecutive failures disable that credential until the other credential recovers it.

These rules follow NIST SP 800-63B-4 password, random-value, throttling, session, and saved-recovery
requirements and OWASP's Argon2id minimum:

- https://pages.nist.gov/800-63-4/sp800-63b/authenticators/
- https://pages.nist.gov/800-63-4/sp800-63b.html#account-recovery
- https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html

## Abuse cases and required controls

| Abuse case | Required control and proof |
|---|---|
| A caller asks for Workspace data before setup or sign-in | Exact locked status has no Workspace fields; navigation remains disabled. |
| A caller claims setup is complete without saving recovery material | Enrollment transaction refuses `recovery_saved=false`; no account row is written. |
| A database copy is stolen | It contains salted Argon2id PHC output and a one-way Recovery Key verifier, never either plaintext secret. |
| A password or Recovery Key is guessed repeatedly | Persistent per-credential attempt counters impose exponential delay and disable at 100 failures. |
| An old Recovery Key is replayed after recovery | Recovery replaces the verifier atomically before success returns. |
| The desktop process restarts after sign-in | Session state is memory-only; an existing account reopens locked. |
| A UI field or command contains extra undeclared data | Both Rust serialization and TypeScript runtime validation use closed, phase-specific shapes. |
| Another local process connects to the supervisor socket | Every request requires the random capability token and token comparison is constant-time; wrong-token requests change no state. |
| An unauthenticated client opens a partial request and stops writing | Per-connection read and write deadlines bound the stall before the single-threaded supervisor accepts the next request. |
| A second supervisor starts while one is live | A non-blocking kernel file lock rejects it before stale-socket cleanup, preserving the live socket and single-writer invariant. |
| A remote adapter tries password fallback | The domain interface requires an access origin and refuses anything except `same_machine`. |
| The system sleeps or the OS screen locks | The Linux supervisor accepts only sender-bound signals from an owned supported screen-lock provider or systemd-logind and locks the session. Other platforms remain release-blocking follow-up work. |
| A compromised renderer reads a secret while the Owner enters or views it | Renderer hardening and a native secret-entry surface remain defense-in-depth follow-ups; no secret survives in browser storage or command output after the step completes. |

## Session semantics

The default inactivity window is 30 minutes. The live unlock timestamp exists only in the Rust
process. Status checks do not renew it. While the Workspace is visible, the desktop refreshes the
authoritative status every second and closes access when the supervisor reports expiry or cannot be
reached. It also conceals Workspace material immediately on window focus loss and re-verifies the
supervisor before revealing it again. Explicit authenticated Owner activity may renew the server
timestamp; manual lock invalidates it immediately. Agent work is outside this human-session state
and continues when the session locks.

The Owner may configure the inactivity window from 1 minute through 24 hours. The default remains
30 minutes and Beacon never offers a no-timeout value. A shorter policy applies to the existing
session immediately; changing the policy requires an unlocked Owner session and is not a Cloud Link
preference. NIST requires documented, context-sensitive session limits and termination when the
inactivity limit expires; it also requires subscriber activity, rather than unrelated machine
activity, to renew the session:

- https://pages.nist.gov/800-63-4/sp800-63b/session/

On Linux, the independently running supervisor—not the desktop renderer—owns operating-system lock
monitoring. It subscribes to `PrepareForSleep(true)` on systemd-logind's system bus and to
`ActiveChanged(true)` on the GNOME and freedesktop-compatible screen-saver interfaces on the user
bus. A `false` signal never unlocks Beacon. Each monitor reports `starting`, `active`, or
`unavailable`; an unavailable monitor leaves manual and inactivity locking active and creates a
visible warning rather than a false claim. `active` requires a live supported D-Bus owner and owner
loss changes the status to `unavailable`; signal matches are bound to that owner. The supervisor
also rechecks provider ownership and `GetActive` state every monitor cycle, so acquiring or
restarting a provider while the screen is already locked locks Beacon before that provider is
reported active. Losing the desktop window cannot disable the monitor.

Platform and library sources:

- systemd-logind sends `PrepareForSleep(true)` before suspend or hibernate:
  https://www.freedesktop.org/wiki/Software/systemd/logind/
- `dbus` 0.9.12 blocking connections register typed signal callbacks with `add_match` and dispatch
  them with bounded `process` calls:
  https://docs.rs/dbus/0.9.12/dbus/blocking/struct.Connection.html
- The supported Ubuntu GNOME host was also verified directly with `busctl` to expose
  `org.gnome.ScreenSaver.ActiveChanged(bool)` and logind `PrepareForSleep(bool)`.

## Deferred, release-blocking work

- Native passkey adapters: Windows WebAuthn, Apple AuthenticationServices, and a trustworthy Linux
  FIDO2/credentials portal path.
- Native OS screen-lock and suspend listeners on platforms other than Linux.
- The current Tauri Linux stack transitively uses the legacy GTK3 bindings. The 2026-08-27
  `cargo-audit` run found no vulnerability advisories but did report the GTK3 family as unmaintained
  and `glib` 0.18.5 under RUSTSEC-2024-0429. Packaging remains disabled; a release review must
  document the reachable surface or move to a maintained upstream path before public distribution.
- A renderer-hardening review before exposing Workspace content, including CSP and secret-entry
  handling.
- Compromised-password blocklist strategy that works offline without leaking the candidate.
- Remote pairing, MFA, Cloud Link, and any network authentication surface.

None of these deferrals may be silently replaced with a WebView-only passkey, remote password
fallback, or a Cloud Link identity dependency.
