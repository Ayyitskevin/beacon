# C3-opus2 — Alpha criterion 2: clean-host first launch of the CI AppImage, observed

Seat: opus2 · Fable (w7:p2) · brief `brief-C3-opus2.md`. Nothing launched outside a container.
Nothing pushed; the repo was read only (plus one local `cargo build -p beacond`, explained below).
Working directory: `beacon-team/clean-host/`. Raw logs, stderr, and captures: `clean-host/out/<env>/`.

## Verdict

**Criterion 2 does not pass. The shipped AppImage cannot complete first launch on any host.**
Two independent blockers, both observed, neither of them a rendering problem:

1. **`beacond` is not in the bundle.** The desktop shell spawns it during Tauri's setup hook, the
   spawn fails with ENOENT, and the process panics and exits 101 — on every environment where it got
   far enough to try.
2. **The bundle has a glibc floor of 2.39.** It will not load on Debian 12, Ubuntu 22.04, RHEL 9, or
   Mint 21.

And the answer to the question Fable could not settle: **it renders perfectly.** With `beacond`
placed next to the shell, the same Xvfb, the same `import -window root`, and no GL environment
variable at all produces a full-colour first-run screen. The black capture was never a rendering
fault; there was nothing painting behind it.

## Artifact

`gh api repos/Ayyitskevin/beacon/actions/artifacts/9832520729/zip` (run 33591735092), 78,897,256
bytes, unzipped to `Beacon_0.1.0_amd64.AppImage` (79,522,296 bytes) and `Beacon.AppImage.sha256`.
sha256 `d479f4c4d20f905f79f665858e53b8d8b4d09e5d4bce0fb5b0b9fd7d2c7e8c2c` — recomputed on the host,
matches the shipped checksum file exactly.

## Environments

Base image **`ubuntu:24.04`** (glibc 2.39) for the primary runs, plus **`debian:12`** (glibc 2.36)
as a portability probe. Ubuntu 24.04 because it is what the CI runner builds on, so it is the most
favourable host the artifact can possibly meet — a failure there is unambiguous. Every container is
`docker run --rm` with a tmpfs `/work` and a tmpfs scratch `HOME`; the repo is never mounted.

Three packages appear in every run — `xvfb x11-utils imagemagick`. These are **harness**, not
application dependencies: Xvfb stands in for the user's display server, `xwininfo` and ImageMagick
are the instruments. **None of the nine dev packages from CONTRIBUTING was installed anywhere.**

| env | image | packages added beyond the harness | launch | outcome |
|---|---|---|---|---|
| **A** | ubuntu:24.04 | *(none)* | `--appimage-extract-and-run` | died <1 s, exit **127** — `libfribidi.so.0` missing |
| **B** | ubuntu:24.04 | `libfribidi0 libharfbuzz0b libegl1` | `--appimage-extract-and-run` | died <1 s, exit **127** — `libGLESv2.so.2` missing |
| **C** | ubuntu:24.04 | + `libgles2` | `--appimage-extract-and-run` | died <1 s, exit **101** — **setup-hook panic** |
| **E** | ubuntu:24.04 | *(none)* | direct, FUSE | died <1 s, exit **127** — no fusermount |
| **D** | ubuntu:24.04 | `libfuse2t64 fuse3` + the four above, `--device /dev/fuse --cap-add SYS_ADMIN` | direct, FUSE | mounted and ran; died <1 s, exit **101** — **same setup-hook panic** |
| **G2** | debian:12 | `xauth` + the four above | `--appimage-extract-and-run` | died <1 s, exit **1** — **GLIBC_2.39 / 2.38 not found** |
| **F** | ubuntu:24.04 | the four above · **repaired bundle** | `squashfs-root/AppRun` | **alive at 24 s, window mapped, non-black screenshot** |

Exact apt line for D (the widest): `apt-get install -y --no-install-recommends xvfb x11-utils
imagemagick libfuse2t64 fuse3 libfribidi0 libharfbuzz0b libegl1 libgles2`.
Launch command everywhere: `xvfb-run -a --server-args="-screen 0 1280x800x24" <command>`.

## Blocker 1 — `beacond` is not in the AppImage

Verbatim stderr, env C and env D alike:

```
thread 'main' (1567) panicked at /home/runner/.cargo/registry/src/index.crates.io-.../tauri-2.11.5/src/app.rs:1425:11:
Failed to setup app: error encountered during setup hook: No such file or directory (os error 2)
```

The cause, from the bundle and the source rather than inference:

- `find squashfs-root -name '*beacond*'` inside the container returns **nothing but
  `beacon-desktop`**. The only executables in the bundle are `usr/bin/beacon-desktop`,
  `AppRun`, `AppRun.wrapped`, and WebKit's two helper processes.
- `apps/desktop/src-tauri/src/lib.rs:1524-1526` resolves the supervisor as
  `current_exe().with_file_name("beacond")` — inside the mounted AppImage that is
  `<mount>/usr/bin/beacond`, which does not exist.
- `lib.rs:1552` spawns it; `spawn()` returns ENOENT; `?` propagates out of the setup hook at
  `lib.rs:1591-1597`; Tauri panics at `app.rs:1425`. Exit 101 is the Rust panic exit.
- `apps/desktop/src-tauri/tauri.conf.json` declares **no `externalBin` and no `resources`**, so
  nothing ever placed `beacond` in the bundle. `.github/workflows/ci.yml:96` builds the AppImage and
  uploads it; nothing in the job builds or stages a supervisor binary.

This is packaging, not my slice — flagging with the exact seam, not fixing.

## Blocker 2 — the bundle's glibc floor is 2.39

Verbatim, env G2 (Debian 12, glibc 2.36):

```
beacon-desktop: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found (required by beacon-desktop)
beacon-desktop: /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.38' not found (required by .../libgdk-3.so.0)
... and the same for libcairo, libglib-2.0, libwebkit2gtk-4.1, libgtk-3, libsoup-3.0, libgio-2.0, libjavascriptcoregtk-4.1
```

The application binary needs GLIBC_2.39 and the **bundled** GTK/WebKit libraries need GLIBC_2.38,
because linuxdeploy copied them from the Ubuntu 24.04 runner. An AppImage is supposed to be the
portable format; this one is portable only to glibc ≥ 2.39, which excludes Debian 12 (2.36),
Ubuntu 22.04 (2.35), RHEL 9 (2.34), and Mint 21. Building the bundle on an older base is the usual
remedy; again, packaging's call.

## Not a blocker — the four libraries a bare container lacks

Envs A and B fail on `libfribidi.so.0`, `libharfbuzz.so.0`, `libEGL.so.1`, then `libGLESv2.so.2`
(the last is `dlopen`ed by libEGL, so `ldd` cannot see it — it only appears at runtime). A bare
container is **harsher than a real clean host**, and I checked rather than assumed:

```
apt-get install --simulate ubuntu-desktop-minimal   ->  Inst libegl1 · libfribidi0 · libgles2 · libharfbuzz0b
apt-cache rdepends: libgtk-3-0t64 -> libfribidi0, libharfbuzz0b ; xserver-xorg-core -> libegl1 ;
                    gnome-shell, mutter, libgtk-4-1, libwebkit2gtk-4.1-0 -> libgles2
```

All four arrive with any ordinary desktop. I report them because they say what the bundle assumes
about the host, not because a user would hit them.

## The FUSE question — both paths, and one that does bite a real user

- **`--appimage-extract-and-run` needs no FUSE** and worked in every environment (env A/B/C/G2 got
  as far as their respective real failures).
- **Direct execution needs libfuse2.** Env E, verbatim:

```
fuse: device not found, try 'modprobe fuse' first
Error: No suitable fusermount binary found on the $PATH
Cannot mount AppImage, please check your FUSE setup.
```

- With `libfuse2t64` plus `/dev/fuse` and `SYS_ADMIN` (env D) the AppImage mounted and executed
  normally, reaching the same setup-hook panic. So the FUSE path itself is sound.

**This one reaches real users.** `apt-get install --simulate ubuntu-desktop-minimal` installs
`fuse3` and **not** `libfuse2t64`, so a default Ubuntu 24.04 desktop cannot double-click this
AppImage — it gets the message above. That is a documentation or packaging item independent of the
two blockers.

## The rendering question, settled

Env F is a **repaired bundle**: the shipped AppImage extracted, with `beacond` copied in beside
`beacon-desktop`. **It is not the shipped artifact** — it exists solely to separate "the bundle is
incomplete" from "WebKit cannot render under Xvfb". The `beacond` used was built locally from
`aa58dba`, whose only diff from the artifact's source `4637716` is three files under
`apps/desktop/src-tauri/`; `crates/beacond` is identical, so the supervisor is the artifact's own.

Observed:

```
beacon window at 6s: 0x200003 ; 12s: 0x200003 ; 20s: 0x200003
ALIVE_AT_24S
0x200003 "Beacon": ("beacon-desktop" "Beacon-desktop")  1280x800+0+0
root/win captures at 6s, 12s, 20s: mean=0.94598  colors=1416  bytes=55290 (byte-identical)
stderr in full: two benign "libEGL warning: DRI3 error: Could not get DRI3 device" lines
beacond.log: "beacon platform screen-lock monitor is unavailable" / "suspend monitor is unavailable"
```

Screenshot: `clean-host/screenshot-repaired-bundle-20s.png` (window) and
`clean-host/screenshot-repaired-bundle-root-20s.png` (root). I opened it. It shows the genuine
first-run screen: the application rail (Briefing, Projects, and the three disabled entries), the
**"Set up your operator account"** panel with its password and confirm fields and the
**Create recovery key** button, the passkey notice ("Passkeys are not available on this Linux
build"), and the Installation inspector reading **"Workspace access locked"** and **"External
network activity off"**, with the Appearance control on Light.

**No GL workaround was needed.** `WEBKIT_DISABLE_COMPOSITING_MODE=1` and `LIBGL_ALWAYS_SOFTWARE=1`
were never set; the probe has slots for them and they went unused. The `libEGL DRI3` warnings are
noise — WebKit fell back to software rendering on its own and painted correctly. The captures being
byte-identical at 6 s, 12 s and 20 s means the UI had fully painted within six seconds and was
stable, not mid-load.

Why Fable's capture came back black: on this host the process could not survive its setup hook, so
nothing painted. Fable's report of "stays alive 20 s" and mine of "died in under 1 s" disagree, and
I cannot reconcile them from here — the most likely explanation is that the liveness check watched
the `xvfb-run` wrapper rather than the application, since a wrapper outlives a child that exits 101.
Offered as the probable cause, not as a finding: I did not re-run Fable's exact command.

## What remains unobserved

- A real user's desktop session — a live X or Wayland compositor, a window manager, a GPU driver.
  Everything here is Xvfb with software rendering.
- **Wayland entirely.** Not probed. WebKitGTK behaves differently there.
- Any distro beyond Ubuntu 24.04 and Debian 12. Fedora, Arch, and openSUSE are untested.
- Double-click launch from a file manager, `.desktop` integration, the icon, and AppImage
  self-update. Only shell invocation was exercised.
- The application past first paint: nothing was typed, no account was created, no Workspace opened.
  "It renders" is not "it works".
- The two blockers were observed only in containers. They are properties of the artifact and the
  source, not of the container, but no real machine was used.

## Harness disposal (R12)

Every container ran `--rm`; `docker ps -a` shows no Beacon container. Two base images remain in the
local cache (`ubuntu:24.04` 119 MB, `debian:12` 185 MB). The scripts are filed rather than deleted —
`clean-host/probe.sh` (shipped-artifact probe), `clean-host/probe2.sh` (repaired-bundle probe),
`clean-host/run-env.sh` and `run-env2.sh` (drivers) — so any of these runs is one command to repeat.
`clean-host/` holds 213 MB, most of it the artifact zip, the AppImage, and the locally built
`beacond-from-4637716`; say the word and I will delete the three large files and keep the logs and
captures.
