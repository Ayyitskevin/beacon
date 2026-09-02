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

Then, with the produced AppImage, on a host with no development packages
(`beacon-team/clean-host/run-any.sh` and `probe3.sh` drive this):

```sh
docker run --rm ubuntu:24.04   # + xvfb x11-utils imagemagick libfribidi0 libharfbuzz0b libegl1 libgles2
xvfb-run -a --server-args="-screen 0 1280x800x24" ./Beacon_0.1.0_amd64.AppImage --appimage-extract-and-run
```

Observed: alive at 24 s, an X window named `Beacon` at 1280x800, a screenshot with mean luminance
0.946 across 1416 colours showing the operator-account setup screen, a `beacond` child process
serving the socket, and a 364,544-byte store at
`$HOME/.local/share/dev.beacon.phase0/beacon.sqlite3`. The same holds for the direct FUSE path with
`libfuse2t64` installed.

## Evidence limits

- Observed on `ubuntu:24.04`, `ubuntu:22.04`, and `debian:12` **containers under Xvfb**. Not on a
  real desktop session, not on Wayland, not on a non-Debian-family distribution, and not with a GPU.
- Nothing was typed. The application was observed reaching its first screen with its supervisor
  running; no account was created and no Workspace was opened. "It launches" is not "it works".
- The AppImage's desktop integration — file-manager launch, `.desktop` registration, the icon — was
  not exercised. Only shell invocation was.
- The `ubuntu-22.04` runner label was not verified to still exist on GitHub; that is checkable only
  by running the workflow.
