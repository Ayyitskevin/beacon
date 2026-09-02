# Contributing to Beacon

Beacon is one local-first office suite for AI agent teams supervised by one human operator. It is
**pre-Alpha**: there is no published release, and [AGENTS.md](AGENTS.md) is explicit that this
checkpoint must not be described as a usable Alpha. See its *Current state* section for what exists
— and treat that section as the answer rather than this page, which would go stale naming features.
`Beacon` is a working title, not a cleared name.

Contributions are welcome. This page is how to work in the repository; [GOVERNANCE.md](GOVERNANCE.md)
is who decides what.

## Read this first

1. [README.md](README.md) — what Beacon is for.
2. [CONTEXT.md](CONTEXT.md) — the domain language. Terms like Mission, Receipt, Ruling, and Attention
   Item mean specific things, and using them loosely is the most common review comment.
3. [docs/BLUEPRINT.md](docs/BLUEPRINT.md) — the Alpha 1 design.
4. [docs/adr/](docs/adr/) — accepted decisions. If your change contradicts one, that is a proposal,
   not a patch.
5. [AGENTS.md](AGENTS.md) — the repository contract: product invariants, module design, persistence
   rules, and the working agreement. It is written for AI agents and is equally the rules for people.

## Setting up

```sh
sudo apt-get update && \
  sudo apt-get install --no-install-recommends --yes \
    libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev
python3.12 -m venv .venv && .venv/bin/pip install -e ".[dev]"   # the legacy Python tree
npm --prefix apps/desktop ci                                    # the desktop app
```

The Rust toolchain is pinned in `rust-toolchain.toml` and installs itself via rustup. The gate
refuses to run without `.venv`, on purpose: importing another checkout's copy is a false green this
repository has actually hit. Without the Linux development libraries and build tools above, the
native Tauri compile and link steps in the desktop and Rust gates fail before Beacon can be built.

## The gate

One command runs everything:

```sh
scripts/gate all
```

Stages run individually as `scripts/gate lint | format | test | desktop | rust`.

**Read its exit status, not its tail.** The script is deliberately built with no pipes into anything
that consumes an exit status, because five times in one session a check failed and the work
proceeded anyway. Its header comment lists each shell construct that caused that. When capturing
output:

```sh
scripts/gate all > gate.log 2>&1; echo "exit=$?" >> gate.log; grep '^exit=' gate.log
```

`cargo clippy` runs with `-D warnings`, so speculative helpers written for a future change will fail
the build. Write what this change uses.

## Downloading the CI AppImage

After the product job passes for a pull request or a push to `main`, the `package` job attaches a
`beacon-linux-appimage` artifact to the CI workflow run for 14 days. Download it from the run's
**Artifacts** section and extract the ZIP archive. GitHub's artifact service does not retain the
executable bit, so verify the bundled checksum and restore that bit before trying the application:

```sh
sha256sum --check Beacon.AppImage.sha256
chmod +x ./*.AppImage
./Beacon_*.AppImage
```

The package job proves that the AppImage builds, that its downloaded bytes can be checked, and that
the bundle contains both `beacon-desktop` and the `beacond` supervisor it spawns. See the official
upload action's [permission-loss
note](https://github.com/actions/upload-artifact/blob/v4.6.2/README.md#permission-loss).

First launch has been observed on a clean `ubuntu:24.04` container — no development packages
installed — through both AppImage entry paths. It has not been observed on a real desktop session,
on Wayland, or on any distribution other than Ubuntu 24.04, so treat the criterion as met for that
one host shape and open everywhere else.

### What the AppImage requires of the host

This is what was measured, not what we would like to promise.

- **glibc 2.35 or newer.** linuxdeploy bundles the build host's GTK and WebKit libraries, so the
  package job's base image sets this floor; it is pinned to `ubuntu-22.04` for that reason. The
  bundle was observed launching on Ubuntu 22.04, Debian 12, and Ubuntu 24.04. Anything older than
  glibc 2.35 — RHEL 9 is 2.34 — fails with a loader error naming the missing version, not a crash.
  Built on `ubuntu-latest` instead, the floor is 2.39 and Debian 12 and Ubuntu 22.04 both fail.
- **A desktop's ordinary graphics and text libraries.** The bundle does not carry `libfribidi0`,
  `libharfbuzz0b`, `libegl1`, or `libgles2`. Every desktop install has them —
  `apt-get install --simulate ubuntu-desktop-minimal` pulls all four, `libgtk-3-0t64` depends on the
  first two, `xserver-xorg-core` on `libegl1`, and `gnome-shell`, `mutter`, and `libwebkit2gtk-4.1-0`
  on `libgles2` — but a bare container does not, and it fails there with a missing-library error.
- **`libfuse2` only if you run the AppImage directly.** Ubuntu 24.04 ships `fuse3` and not
  `libfuse2t64`, so a default desktop cannot double-click this file; it prints "Cannot mount
  AppImage, please check your FUSE setup." Either install `libfuse2t64` or run
  `./Beacon_*.AppImage --appimage-extract-and-run`, which needs no FUSE at all.

### Building the bundle locally

`npm run tauri build` from `apps/desktop` is the whole command. Its `beforeBuildCommand` builds
`beacond` in release and stages it as the sidecar `tauri.conf.json` names, so there is no separate
step to forget.

Because that sidecar is a declared bundle input, `apps/desktop/src-tauri/build.rs` requires it to
exist before the desktop crate compiles — including under `--no-bundle`. `scripts/gate all` is fine:
its desktop stage runs `npm run tauri build -- --no-bundle` before any bare cargo command, and that
stages the sidecar. But **`scripts/gate rust` or `cargo build --workspace` run alone on a fresh
checkout will fail** with `resource path ... doesn't exist`. Run this once first:

```sh
cd apps/desktop && npm run tauri build -- --no-bundle
```

## What good looks like here

From the working agreement in AGENTS.md, and worth repeating because reviews turn on them:

- **Read before you write.** Immediate callers, exports, shared types, and tests.
- **Smallest vertical slice that crosses the real interface.** Prefer a thin slice that works end to
  end over a wide one that does not.
- **Test observable behavior** through Command, Query, and Event interfaces — not internals.
- **For a bug guard, prove the test fails against the plausible wrong answer.** A guard whose test
  passes without the guard is not a guard. Deleting the mechanism and watching the test go red is
  cheap and is expected for anything security-relevant.
- **Distinguish absent, dead, broken, and not evaluated.** Collapsing "the check errored", "the check
  never finished", and "the check said no" into one state is the defect this codebase most often
  finds in itself.
- **Do not claim operability from a green unit suite.** Run the path you are claiming works.

## Fixtures name nothing on your machine

A test fixture must be constructed by the test, not borrowed from the machine that wrote it. No
absolute path inside your home directory, no host name, no address from your network. Where a test
needs a repository, build one in a temporary directory; where it needs only a declaration, a
plausible fabricated path is correct and a real one is not. This is a consequence of
[ADR 0028](docs/adr/0028-beacon-is-a-standalone-product.md) and it is enforced by review.

## Proposing a change

Small, reversible changes take normal review. Changes to public contracts, domain language,
security, authority, persistence, compatibility, or governance need a written proposal and a
decision record — see the ADRs in `docs/adr/` for the form, and GOVERNANCE.md for who signs off.

Pull requests should say what the change does **and what it deliberately does not do**. Naming the
scope boundary is how a reviewer knows an omission was a decision.

Evidence settles technical disagreements. Where it cannot, the Product Steward decides.

## Security

Do not open a public issue for a security problem. See [SECURITY.md](SECURITY.md).

## Licence

Contributions are accepted under the [Apache License 2.0](LICENSE).
