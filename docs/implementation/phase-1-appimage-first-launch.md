# Phase 1 AppImage first launch

This checkpoint makes the packaged application complete its own first launch on a host that has
nothing installed for it, and records what that does and does not prove.

## What was wrong

The bundle shipped `beacon-desktop` and no `beacond`. The desktop shell resolves its supervisor as
`current_exe().with_file_name("beacond")` and spawns it during Tauri's setup hook; inside the
AppImage that path did not exist, `spawn` returned `ENOENT`, and the process panicked at
`tauri-2.11.5/src/app.rs:1425` with `Failed to setup app: ... No such file or directory (os error 2)`
before anything could be typed. `tauri.conf.json` declared neither `externalBin` nor `resources`, so
nothing was ever going to place the supervisor in the bundle. The build was green throughout: the
only failure the packaging step can raise is a capability naming a permission that does not exist,
which is the opposite direction.

## The sidecar

`bundle.externalBin` now names `../../../target/sidecar/beacond`, relative to the directory holding
`tauri.conf.json`. `tauri-utils` 2.9.3 `src/config.rs:1660-1672` documents the naming rule — Tauri
looks for `binary-name{-target-triple}`, so the staged file is `beacond-x86_64-unknown-linux-gnu`.
`tauri-build` 2.6.3 `copy_binaries` (`src/lib.rs:54-83`) strips that suffix and copies the file into
the cargo target directory, beside the compiled shell, and the AppImage bundler carries both into
`usr/bin`.

Measured in the bundle this branch produces, rather than assumed:

```
Beacon.AppDir/usr/bin/beacon-desktop   16142096
Beacon.AppDir/usr/bin/beacond           6824552
```

The sidecar lands beside the shell, so `current_exe().with_file_name("beacond")` already resolves
it. **No Rust changed.** The runtime path was measured before it was left alone.

The staged copy lives under `target/`, which `.gitignore` already covers, so a 6 MB binary cannot be
committed by accident and no ignore rule was added.

## What declaring a bundle input costs

`tauri-build` runs on every compile of the desktop crate, and a declared `externalBin` that is
missing fails the build — including under `--no-bundle`, at `build.rs`, with
`resource path ... doesn't exist`. `beforeBuildCommand` and `beforeDevCommand` therefore build
`beacond` in release and stage it before the shell compiles.

There is one implementation of that: `scripts/stage-sidecar`, called by the gate's cargo-only stage
and by both Tauri hooks. It lived in three shapes before, and they had already drifted where it
mattered most — the gate's copy ran its commands independently and so reported only the last, which
meant a FAILED release build followed by a successful copy of a stale binary printed
`ok stage sidecar` and the gate exited 0. `tests/harness/test_gate_sidecar_staging.py` injects that
failure permanently and requires `FAIL stage sidecar` and a nonzero exit.

`set -e` in a subshell does not fix that class: `run` calls the helper as `if "$@"; then`, and `-e`
is suppressed for a command whose status is being tested. A separate script gets its own shell, so
its `set -euo pipefail` is its own.

`scripts/gate all` and `scripts/gate rust` therefore both work from a clean checkout with no
`target/sidecar`. A bare `cargo build --workspace` does not go through the gate and still needs one
`scripts/stage-sidecar` first; CONTRIBUTING records that.

CI needs no staging step for the same reason, but it now asserts the outcome: a step fails the
package job when `Beacon.AppDir/usr/bin/beacond` is missing. It was run against the wrong answer it
kills — an AppDir with the supervisor removed exits 1 with
`no supervisor in ... -- the AppImage cannot complete first launch`.

## The glibc floor

linuxdeploy bundles the build host's GTK and WebKit libraries, so the package job's base image sets
the floor. On `ubuntu-latest` the shell needs `GLIBC_2.39` and the bundled libraries need `2.38`,
which excludes Debian 12 and Ubuntu 22.04. The job is pinned to `ubuntu-22.04`, measured by
building the same tree in an `ubuntu:22.04` container and launching the result on three clean
hosts — Ubuntu 22.04, Debian 12, and Ubuntu 24.04 — all of which reached a rendered first-run screen
with the supervisor running. Those three are the whole of what was launched. Other hosts at glibc
2.35 or newer are loader-eligible and expected to work, which is not the same claim: the bundle also
needs four libraries it does not carry and, run directly, `libfuse2`. RHEL 9 is glibc 2.34 and is
below the floor; going lower means a manylinux-style container build rather than a runner label.

## Verification

Rendered-desktop evidence cannot come from a unit suite, so it comes from disposable containers.
From `apps/desktop`:

```sh
npm run tauri build                      # produces the AppImage and its AppDir
ls -l ../../target/release/bundle/appimage/Beacon.AppDir/usr/bin/
```

Then, with the produced or downloaded AppImage, the repository-owned harness drives first-run setup
on the three clean hosts:

The launch-only matrix ADR 0036 cites lives in `docs/evidence/2026-09-02-appimage-clean-host/`;
this harness supersedes it for first-run setup.

```sh
scripts/clean-host-first-run ./Beacon_0.1.0_amd64.AppImage ./first-run-evidence
```

The outer script runs `ubuntu:22.04`, `debian:12`, and `ubuntu:24.04` with `docker run --rm`, a
tmpfs home, and no development packages. The inner driver uses Xvfb and `xdotool` to enter the
password, advance through the Recovery Key confirmation, finish Account and Workspace creation,
and accept the default starter organization. It then requires authenticated Briefing to render.
The UI remains the writer; read-only daemon-socket queries verify the setup-required, unlocked,
starter-organization, and Residents states without substituting daemon calls for user actions.

Every asserted state has a screenshot or socket-response artifact. Recovery Key pixels never
leave tmpfs: the corresponding image band is covered before the screenshot is written, and requests
(including the local capability token) are never written. Raw AppImage and Xvfb logs also remain in
tmpfs until the export scan accepts them. The outer runner validates the closed five-transition
`result.json` schema and every named artifact; Docker's exit code alone cannot create an observed
host. Per-step `result.json` and whole-matrix `matrix.json` keep `absent`, `broken`, and
`not_evaluated` separate, including the transition-specific cause and an interrupted active step.
Every persisted byte is scanned for the fixture password, grouped Recovery Keys, and password- or
token-shaped values. Only a clean scan writes `.export-safe`, and CI's evidence upload requires that
marker even under `always()`.

Acceptance criterion row 2 remains **partial** until the round-2 harness fixes receive focused
cross-seat confirmation. A green local gate proves the implementation state, not that ruling.

This is part of the package job, with its evidence uploaded separately from the AppImage. The final
local matrix took 39 seconds on Ubuntu 22.04, 37 on Debian 12, and 61 on Ubuntu 24.04. The
immediately preceding main package job took 11 minutes 21 seconds without the harness, so the
measured addition fits its 30-minute budget with substantial room rather than converting a timeout
into a product verdict.

## Evidence limits

- Observed through complete first-run setup on `ubuntu:24.04`, `ubuntu:22.04`, and `debian:12`
  **containers under Xvfb**. Not on a
  real desktop session, not on Wayland, not on a non-Debian-family distribution, and not with a GPU.
- The real UI was driven, but only along the default password and starter-organization path. Native
  passkeys, Recovery Key account recovery, edited or discarded starter rosters, and relaunch are
  outside this harness.
- The AppImage's desktop integration — file-manager launch, `.desktop` registration, the icon — was
  not exercised. Only shell invocation was.
- CI's `ubuntu-22.04` package runner and its AppImage artifact were both observed on main run
  `33658544612`; the harness was run against those downloaded, checksum-verified bytes.
