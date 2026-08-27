---
status: accepted
---

# Require an authenticated Operator Account

Alpha requires the human to create and sign in to a local Operator Account before entering a
Workspace or issuing an Operator Command. Sign-in is passkey-first with password fallback, and
local workflows remain available offline after sign-in. Password fallback is same-machine only;
remote human access requires a passkey. An optional Cloud Link synchronizes only an explicit
allowlist of non-secret interface preferences without replacing local identity, moving work
records, or granting authority.

## Consequences

Authentication success never authorizes an external effect or bypasses Policy and Gates. Account
credentials and sessions are security-sensitive and require non-author review plus human approval
before implementation. Local recovery uses an Operator-held offline Recovery Key and does not
depend on Cloud Link. Setup cannot complete until the Operator confirms that the key was saved;
this records an assertion, not proof of separate storage. A successful Recovery Key rotation
immediately invalidates the old key. The human session locks after 30 minutes of inactivity by
default, with an Operator-configurable duration, and immediately on operating-system screen lock
or suspend. Locking blocks human access but does not pause authorized Agent work.

Multi-factor authentication is optional for same-machine access. Human remote access requires local
approval of the exact revocable Paired Device through a QR code and human-compared matching code,
plus a passkey and MFA. The device list shows last use and provides one-action revocation. Remote
pairings expire after 90 days without use, and Beacon creates an Attention Item seven days before
expiry. Worker leases use machine identity and satisfy none of these requirements. Cloud Link's
Alpha allowlist is theme, keyboard shortcuts, accessibility preferences, and notification
preferences. It excludes notification contents, events, and history; Workspace data; Projects;
Missions; Agents; Policy; secrets; credentials; and every form of authority.

For broad adoption, the official hosted Cloud Link is the default setup path. Operators may instead
use a self-hosted endpoint implementing the same documented protocol and data boundaries. Official
hosting costs $5 per month and removes the need to run sync infrastructure. It requires the Operator
to choose United States or European Union data residency at launch. Linked Devices receive only the
allowed preference synchronization, not remote access or authority. The local product and
protocol-compatible self-hosted Cloud Link remain free without a time limit. Self-hosting uses a
one-command Docker installer for Linux x86-64 and ARM64. Preference version history remains
available locally and when self-hosted; official hosting manages its remote copy. Changing regions
requires an explicit migration and never silently relocates data. The local Account and complete
local product depend on neither service.
