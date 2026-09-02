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
python3.12 -m venv .venv && .venv/bin/pip install -e ".[dev]"   # the legacy Python tree
cd apps/desktop && npm ci                                       # the desktop app
```

The Rust toolchain is pinned in `rust-toolchain.toml` and installs itself via rustup. The gate
refuses to run without `.venv`, on purpose: importing another checkout's copy is a false green this
repository has actually hit.

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

The package job proves that the AppImage builds and its downloaded bytes can be checked. First run
on a clean Linux host is still unverified, so the downloaded-AppImage launch criterion is partial,
not met. See the official upload action's [permission-loss
note](https://github.com/actions/upload-artifact/blob/v4.6.2/README.md#permission-loss).

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
