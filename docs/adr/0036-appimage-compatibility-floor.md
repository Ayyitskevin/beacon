---
status: proposed
---

# Set the distributed AppImage's compatibility floor at glibc 2.35

This proposal is written against exact Beacon base
`ececa6f4ce384babd6736caa5e8d12355f31282c`. Pinning the package job's runner
changes which machines can run a downloaded Beacon, which `GOVERNANCE.md` makes
a high-stakes compatibility change: it needs this record, one non-author review,
and explicit Product Steward sign-off. Merging the pull request that carries this
file is the acceptance. Nothing here grants authority or changes a status.

## Context

The AppImage is assembled by linuxdeploy, which copies the **build host's** GTK
and WebKit libraries into the bundle. The base image of the CI `package` job
therefore sets the floor for every machine that can run the artifact. Nothing
in the crate, the toolchain, or `tauri.conf.json` moves it; only the runner does.

That coupling was invisible until first launch was attempted on a clean host.
Built on `ubuntu-latest` (24.04, glibc 2.39), the shell binary requires
`GLIBC_2.39` and the bundled libraries require `GLIBC_2.38`. On Debian 12 the
loader refuses before any Beacon code runs:

```
beacon-desktop: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found (required by beacon-desktop)
... and the same for libgdk-3, libcairo, libglib-2.0, libwebkit2gtk-4.1, libgtk-3,
    libsoup-3.0, libgio-2.0, libjavascriptcoregtk-4.1
```

## Measured matrix

Each row is a clean container with no development packages, launching the
AppImage under Xvfb and checking that the process survives 24 seconds, maps its
window, starts `beacond`, and creates its store. Recorded in
`C3-opus2-clean-host-report.md` and `C4-opus2-report.md`.

| Host | glibc | Built on 24.04 | Built on 22.04 |
|---|---|---|---|
| `ubuntu:22.04` | 2.35 | not run | **launches** — window, supervisor, store, mean 0.9467 |
| `debian:12` | 2.36 | **fails** — `GLIBC_2.38 not found`, 13 bundled libraries | **launches** — window, supervisor, store, mean 0.9467 |
| `ubuntu:24.04` | 2.39 | **launches** | **launches** — window, supervisor, store, mean 0.9467 |

Building on the older base cost nothing observed: the same tree, the same pinned
Rust 1.97.1 and Node 22.23.2, the same apt line (`libwebkit2gtk-4.1-dev` is
available on jammy), and an AppImage that launches on all three.

## Options

**A — floor 2.35 via `runs-on: ubuntu-22.04` on the package job (recommended).**
One line. **Measured:** launches on Ubuntu 22.04, Debian 12, and Ubuntu 24.04 —
the three rows above, and the only hosts anything was launched on.
**Expected but not measured:** any other host at glibc 2.35 or newer, such as
Mint 21 and 22, is loader-*eligible*. That is necessary, not sufficient: the
bundle also needs the four libraries it does not carry and, for direct
execution, `libfuse2` (`CONTRIBUTING.md`). No non-Debian-family host and no real
desktop session has been launched at all.
**Excluded** below the floor: RHEL 9 and CentOS Stream 9 (2.34), SLES 15 (2.31),
and Debian 11 (2.31).
Cost if wrong: GitHub retires the `ubuntu-22.04` label and the package job fails
at start — loudly, and revertible in one line. That label's continued
availability is the one thing in this proposal not verified locally.

**B — keep the floor at 2.39 on `ubuntu-latest`.** No change, no risk of a
retired label, and the floor rises on its own each time the runner moves. It
excludes every distribution above **plus** Debian 12 and Ubuntu 22.04 — an
LTS still in standard support and the most common Debian stable at the time of
writing. Cost if wrong: Alpha testers on ordinary machines cannot run the
artifact at all, and the failure is a loader error that reads like a broken
download.

**C — a manylinux-style container build.** Build inside a pinned old-glibc image
rather than on a runner label, decoupling the floor from GitHub's fleet
entirely. It is the only option that could make hosts below 2.35 loader-eligible
at all; RHEL 9 and older are its **intended target, not a measured result** —
nothing has been built or launched this way, and the same unmeasured
runtime-library and desktop boundary would still apply on top of the loader. Cost: a container image to build and maintain, WebKit and
GTK development packages sourced for that base, and a slower package job — a
project rather than a line, and not justified by any Alpha requirement now.

## Recommendation

**Option A.** It is the only change measured end to end, it is one line, it is
revertible in one line, and it widens the reachable fleet at no observed cost.
Option C stays on the table if a target host below 2.35 ever appears; the
measured matrix here is what a later comparison should be made against.

## Consequences if accepted

The package job pins `runs-on: ubuntu-22.04` while the product job stays on
`ubuntu-latest`: this is a packaging constraint, not a toolchain one. The floor
is documented in `CONTRIBUTING.md` as a requirement rather than a promise, and
`docs/implementation/phase-1-appimage-first-launch.md` records how it was
measured. RHEL 9 and older remain unreachable and are not claimed. The pinned
line carries a comment saying why, so a later modernisation of the runner label
is a deliberate decision rather than a tidy-up.
