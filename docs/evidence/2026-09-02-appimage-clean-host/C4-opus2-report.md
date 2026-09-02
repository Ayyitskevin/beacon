# C4-opus2 — the CI AppImage completes first launch on a clean host

Seat: opus2 · Fable (w7:p2) · brief `brief-C4-opus2.md`. **Nothing pushed.**
Branch `team/opus2-appimage-sidecar` in `/example/checkouts/beacon-opus2-c`, from `origin/main`
`4637716`. **HEAD `e8e3736`, `scripts/gate all` exit 0, all 11 checks ok, tree clean.**

Gate totals at `e8e3736`: pytest **424**, desktop vitest **4 files / 84 tests**, Rust **280 passed**,
plus fmt, clippy `-D warnings`, both builds and the Tauri `--no-bundle` build.

## 0. Cleanup (Kevin's note), done first

`beacon-team/clean-host/` went from **213 MB to 796 KB**. Deleted `artifact.zip` (78,897,256 B),
`Beacon_0.1.0_amd64.AppImage` (79,522,296 B), and the locally built `beacond-from-4637716`
(63,744,728 B). There was no `squashfs-root` to delete — every extraction ran inside a disposable
container, so nothing was left on disk. Kept every log, stderr capture, `xwininfo` dump and
screenshot, plus `Beacon.AppImage.sha256` and the probe scripts. `clean-host/REGENERATE.md` records
each deleted file's size, sha256 and the one command that brings it back.

Tonight's work added two more AppImages (82 MB and 84 MB); both were deleted the same way after the
evidence was extracted, and `REGENERATE.md` covers them too. The directory now holds **2.4 MB**: all
logs, six screenshots, and the harness.

## 1. The supervisor now ships inside the bundle — commit `4f0c92c`

`bundle.externalBin: ["../../../target/sidecar/beacond"]`. Sources read, not guessed:

- **`tauri-utils` 2.9.3 `src/config.rs:1660-1672`** — "Tauri will look for system-specific binaries
  following the pattern `binary-name{-target-triple}{.system-extension}` … `my-binary-x86_64-unknown-linux-gnu`
  for Linux". That fixes the staged filename.
- **`tauri-build` 2.6.3 `src/lib.rs:54-83` (`copy_binaries`)** — strips `-<target_triple>` from the
  filename and copies the file into the cargo target directory. `copy_file` at `:40-53` is where a
  missing sidecar becomes a hard build failure.

**Where it lands — measured in the bundle this branch produces, per the brief:**

```
Beacon.AppDir/usr/bin/beacon-desktop   16142096
Beacon.AppDir/usr/bin/beacond           6824552
```

The sidecar sits beside the shell, so `current_exe().with_file_name("beacond")` at
`lib.rs:1524-1526` already resolves it. **No Rust changed** — I measured the runtime path before
deciding to leave it alone, which is the outcome the brief asked me not to assume.

The staged copy lives under `target/`, already covered by `.gitignore`, so a 6 MB binary cannot be
committed by accident and no ignore rule was needed.

### Keeping `--no-bundle` working — and the one seam it does not cover

Declaring `externalBin` makes the sidecar a **compile-time** input: with the file absent, `build.rs`
fails even under `--no-bundle` — observed, verbatim:

```
thread 'main' panicked at apps/desktop/src-tauri/build.rs:56:6:
failed to build the Beacon Phase 0 Tauri shell: resource path
`../../target/sidecar/beacond-x86_64-unknown-linux-gnu` doesn't exist
```

(That first path was mine and wrong: `externalBin` resolves relative to the directory holding
`tauri.conf.json`, so it needs three `..`, not two. Found by running it, not by reading it.)

`beforeBuildCommand` and `beforeDevCommand` now build `beacond --release` and stage it before the
shell compiles. **`scripts/gate all` works from a clean checkout**, because its desktop stage runs
`npm run tauri build -- --no-bundle` before any bare cargo command — verified by deleting
`target/sidecar/` and running it: exit 0.

**What it does not cover:** `scripts/gate rust` or `cargo build --workspace` run *alone* on a fresh
checkout fail until one tauri build has staged the sidecar. The clean fix is one staging line in
`scripts/gate` — **that file belongs to another lane, so I did not touch it**; CONTRIBUTING documents
the one-command remedy instead. Flagging as a cross-lane item, not absorbing it.

### CI asserts the outcome

The package job needs no staging step (`npm run tauri build` runs `beforeBuildCommand`), but it now
fails if the bundle lacks a supervisor. Run against the wrong answer it kills — an AppDir with
`beacond` removed:

```
no supervisor in .../fakeappdir -- the AppImage cannot complete first launch     rc=1
the real bundle                                                                  rc=0
```

## 2. Criterion 2's first observed pass

Locally rebuilt AppImage (`npm run tauri build`), then the same clean-container harness as C3.
`ubuntu:24.04`, no development packages, only the harness (`xvfb x11-utils imagemagick`) and the
four ordinary desktop libraries.

| | env C′ `--appimage-extract-and-run` | env D′ direct, FUSE |
|---|---|---|
| missing libs (shell / supervisor) | 0 / 0 | 0 / 0 |
| liveness | **ALIVE_AT_24S** | **ALIVE_AT_24S** |
| window | `0x200003 "Beacon" 1280x800+0+0` | same |
| capture | mean **0.94598**, 1416 colours, 55,290 B | identical |
| supervisor | pid 1683, child of the shell, `beacond serve --store … --socket … --auth-token … --lock …` | pid 1729, same argv from `/tmp/.mount_Beacon…` |
| store | `/scratch-home/.local/share/dev.beacon.phase0/beacon.sqlite3`, **364,544 B, mode 0600** | identical |
| stderr, in full | two benign `libEGL warning: DRI3 error` lines | same |

Screenshots: `clean-host/screenshot-sidecar-extract-20s.png` and `…-fuse-20s.png`. I opened them —
the real first-run screen: the rail, **"Set up your operator account"** with both password fields and
**Create recovery key**, the passkey notice, and the Installation inspector reading **"Workspace
access locked"** and **"External network activity off"**.

**What this proves:** a downloaded AppImage, on an Ubuntu 24.04 host with nothing installed for it,
starts its supervisor, creates its store, and renders its first screen — through both AppImage entry
paths. **What it does not prove:** anything about a real desktop session, Wayland, a GPU, a
non-Debian-family distribution, launching from a file manager, or the application past first paint.
Nothing was typed; no account was created.

## 3. glibc floor — measured, and the one-line change made — commit `928197f`

The brief asked whether building on `ubuntu-22.04` yields a bundle that loads on `debian:12`. I built
the same tree inside an `ubuntu:22.04` container (glibc 2.35, same pinned Rust 1.97.1 and Node
22.23.2, same apt line — `libwebkit2gtk-4.1-dev` is available on jammy) and launched the result on
three clean hosts:

| host | glibc | 24.04-built bundle | 22.04-built bundle |
|---|---|---|---|
| ubuntu:22.04 | 2.35 | not tested | **alive 24 s, window, store, mean 0.9467** |
| debian:12 | 2.36 | **fails**: `GLIBC_2.38 not found` (× 13 bundled libs) | **alive 24 s, window, store, mean 0.9467** |
| ubuntu:24.04 | 2.39 | alive 24 s | **alive 24 s, window, store, mean 0.9467** |

So: yes, and it costs nothing observed. `runs-on: ubuntu-latest` → `runs-on: ubuntu-22.04` in the
**package job only**; the product job is untouched, as instructed. The comment on that line explains
why it is pinned, so nobody "modernises" it back.

Screenshots `clean-host/screenshot-jammy-on-debian12-20s.png` and `…-on-jammy-20s.png`.

Residue, stated rather than buried: **RHEL 9 is glibc 2.34 and is still excluded.** Going lower means
a manylinux-style container build instead of a runner label — a much larger change, not proposed.
And I **cannot verify from this machine that GitHub still offers the `ubuntu-22.04` label**; if it has
been retired the job fails at start, loudly, and the fallback is to revert one line and take the 2.39
floor.

One caveat about the Debian 12 rows: the process listing is empty there because `debian:12` has no
`ps` (procps is not installed) — verified separately. The supervisor did run: the store was created
and `beacond.log` carries its two startup lines.

## 4. Host requirements documented — commit `6a21990` (floor corrected in `928197f`)

CONTRIBUTING's AppImage section now states what was measured, as requirements rather than promises:
the glibc floor and which distributions it excludes; that the bundle does **not** carry
`libfribidi0`, `libharfbuzz0b`, `libegl1`, `libgles2`, with the evidence that every desktop supplies
them (`apt-get install --simulate ubuntu-desktop-minimal` pulls all four; `libgtk-3-0t64` depends on
the first two, `xserver-xorg-core` on `libegl1`, `gnome-shell`/`mutter`/`libwebkit2gtk-4.1-0` on
`libgles2`); and that `libfuse2` is needed only for direct execution — Ubuntu 24.04 ships `fuse3`
without it, so a default desktop **cannot double-click this file** and must use
`--appimage-extract-and-run` or install `libfuse2t64`.

The old paragraph claiming clean-host first run was unverified is replaced with a narrower true
claim: observed on `ubuntu:24.04` through both entry paths, unobserved everywhere else.

## 5. Commits, gate, evidence

| sha | item |
|---|---|
| `4f0c92c` | 1 — sidecar in `tauri.conf.json`, CI verification step |
| `6a21990` | 4 — CONTRIBUTING host requirements |
| `928197f` | 3 — package job on `ubuntu-22.04`, floor corrected |
| `e8e3736` | 5 — `docs/implementation/phase-1-appimage-first-launch.md` |

`scripts/gate all` at `e8e3736`: **exit 0**, 11/11 ok. Files touched: `tauri.conf.json`,
`.github/workflows/ci.yml` (package job only), `CONTRIBUTING.md`, one new
`docs/implementation/` note. **No Rust, no daemon code, no ADR, no `scripts/gate`.**

Evidence, all under `beacon-team/clean-host/`:

- `out-sidecar/C2-extract/`, `out-sidecar/D2-fuse/` — criterion 2 pass, both entry paths
- `out-sidecar/J-jammy/`, `J-debian12/`, `J-noble/`, `N-debian12/` — the glibc matrix
- `screenshot-sidecar-extract-20s.png`, `-fuse-20s.png`, `screenshot-jammy-on-debian12-20s.png`,
  `-on-jammy-20s.png`
- `jammy-build.log` — the 22.04 container build
- `probe3.sh`, `run-sidecar.sh`, `run-any.sh` — the harness, filed so any run is one command

All containers ran `--rm`; `docker ps -a` shows no Beacon container. Three base images remain cached
(`ubuntu:24.04`, `ubuntu:22.04`, `debian:12`).

## Open for Fable

- **`scripts/gate rust` alone on a fresh checkout now fails** until a tauri build stages the sidecar.
  One line in `scripts/gate` closes it; that file is not mine. Either hand it to gpt2 or tell me to
  take it.
- **The `ubuntu-22.04` runner label** is the one thing in this branch I could not verify locally.
- **RHEL 9 and older** remain unreachable; the decision if that matters is manylinux-style container
  builds, which is a project, not a line.
- This branch is independent of #35 and #36 and of #32; it touches none of their files.

---

## Addendum — the `scripts/gate` seam, taken (Fable, after report acceptance)

Fable asked me to take the one-line gate fix myself rather than hand it to gpt2 mid-review.

| | |
|---|---|
| New HEAD | **`c2cedd5`** — `fix: stage the sidecar before the gate's cargo-only stage` |
| `scripts/gate rust` (alone, `target/sidecar` deleted first) | **exit 0** |
| `scripts/gate all` (`target/sidecar` deleted first) | **exit 0**, 12/12 ok |
| Working tree | clean |

`stage_sidecar()` builds `beacond --release` and copies it to
`target/sidecar/beacond-$(rustc -vV | awk '/^host:/ {print $2}')`. It is invoked through the same
`run` helper as every other check, so it prints `ok`/`FAIL` in the existing shape and cannot fail
silently — the property that script's header comment exists to protect. It is listed in both the
`rust` and `all` cases, matching how `all` already spells out each rust step; the cost is about a
tenth of a second once `beacond` is built.

Run against the wrong answer before the fix landed. With `target/sidecar` deleted and the staging
absent, `scripts/gate rust` exits **1** with three of its four checks failing:

```
  ok    cargo fmt
  failed to build the Beacon Phase 0 Tauri shell: resource path
  `../../../target/sidecar/beacond-x86_64-unknown-linux-gnu` doesn't exist
  FAIL  cargo clippy
  FAIL  cargo test
  FAIL  cargo build
```

From the same empty state with the fix, all five report `ok`.

The gate now reports **12** checks rather than 11; the added line is `stage sidecar`, and nothing
was removed. The commit message states plainly that this is a seam fix for the sidecar stage and
that `scripts/gate` is gpt2's file — flagged, not quietly annexed. The open item in the previous
section ("`scripts/gate rust` alone on a fresh checkout now fails … hand it to gpt2 or tell me to
take it") is closed by this commit; gpt2 should still be told the file moved under them.

Branch now carries five commits: `4f0c92c` · `6a21990` · `928197f` · `e8e3736` · `c2cedd5`.
Still not pushed.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
