---
status: accepted
---

# Rely on operating-system encryption for live Workspace data in Alpha

Alpha protects the open SQLite store and managed Artifact directory through operating-system disk
encryption instead of adding application-level database encryption and its separate key lifecycle.
Beacon-managed encryption remains mandatory for portable Safety Snapshots before transfer or
off-device storage.

## Consequences

Beacon reports operating-system disk-encryption status as enabled, disabled, or unknown when the
platform exposes it. Disabled and unknown states produce a persistent but unobtrusive warning until
the condition changes; they do not prevent local use in Alpha. The warning opens a one-click,
operating-system-specific setup guide and status recheck but never changes system settings on its
own. Beacon never labels the open live store as Beacon-encrypted. Disk encryption, portable
snapshot encryption, and secret-store protection remain distinct controls.
