# Scoped offline consolidation-core verification

This entry point checks one admitted pure-library slice. It is not `scripts/gate`, full CI,
a working native command journey, M2 success, M3 completion, or permission to run held effects.
Authoring baseline: `c853c5e7719166a5c34dd039006b072cdcaf429c`; use the later exact reviewed
integration commit supplied by the coordinator, not this historical base as an acceptance claim.

## Commands and ownership

After independent source/effects review and a serialized Cargo slot from the coordinator, the
caller chooses an existing owned parent and an absent absolute receipt leaf outside the checkout.
The source must be clean at an explicitly supplied full commit. No default branch/head selection.
Only these Cargo test commands exist in [the script](../../scripts/verify-consolidation-core):

```text
cargo test --offline --locked --jobs 2 --target-dir RECEIPT/target -p beacond --test evidence_span
cargo test --offline --locked --jobs 2 --target-dir RECEIPT/target -p beacond --test local_transport --no-run
```

The second command requires `--compile-local-transport`. It never executes that integration target,
and default operation cannot select it. The wrapper accepts no arbitrary target, command, additional
Cargo options, broad suite, sidecar stage or `scripts/gate` stage. Compilation still executes ordinary
dependency build scripts/proc macros; it is not a no-execution or process-containment guarantee.
For verification invocations, each valued option may occur once, including when its first value is
empty; missing or malformed values and repeated options are refused before any output is created.
Help remains usage-only and exits immediately when parsed.

The direct installed Linux x86-64 Rust/Cargo/Rustdoc 1.97.1 binaries under the caller's
`$HOME/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin` are mandatory. Cargo and rustc version
outputs are recorded and checked; rustup is not invoked, so it cannot install/fallback automatically.
Source `rust-toolchain.toml`, both manifests and `Cargo.lock` remain unchanged. Linux C/linker/system
library availability is not established by metadata or version checks; compiler logs report failures.

Before Cargo, the script copies existing public crates.io registry index/archive/unpacked-source
directories into `RECEIPT/cargo-home`, using GNU cp's reflink optimization when available, otherwise
a real copy. This is retained local cache data, not a download/install. Expect potentially substantial
disk/time cost; cache sizes and free space are caller preflight responsibilities, not measured here.
There must be exactly one `index.crates.io-*` directory in each source index/cache/src partition.
Alternate registries/git sources are refused. Missing cache data is not fetched. A private Cargo HOME
keeps cache administration/extraction/locks out of the shared original cache. No original Cargo
configuration, credentials, git cache or installed binaries are copied. Ancestor Cargo config files
are refused by existence without reading their contents; no config is disabled or edited.

Cargo runs in a cleared child environment containing only explicit tool paths, private HOME,
private Cargo HOME, private TMPDIR, LANG, RUSTC/RUSTDOC and offline/incremental controls. No inherited
tokens, provider endpoints, compiler wrappers, Cargo overrides or environment dump. `--offline`
prevents Cargo downloads; this wrapper is not an OS network sandbox for arbitrary build code.
The reviewed source and installed dependencies remain trusted build inputs. Do not run it on an
unreviewed commit or credential-bearing cache. No live model/service/credential path is selected.

## Local clean-checkout reproduction

The following is proposed reproduction text, not a command executed by this author. Replace all
uppercase placeholders with reviewed exact local paths and a full commit. Checkout creation is a
separate admitted coordinator step: its Git hooks/config/attributes/template effects must already be
reviewed. The wrapper itself creates no checkout, fetches nothing, and does not change Git refs.

```sh
# Only after the separate local-clone creation effects are admitted:
git clone --no-hardlinks --no-checkout -- LOCAL_PINNED_SOURCE FRESH_PRIVATE_CLONE
git -C FRESH_PRIVATE_CLONE checkout --detach EXACT_REVIEWED_COMMIT
(
    cd FRESH_PRIVATE_CLONE || exit
    env -i HOME="$HOME" PATH=/usr/bin:/bin /bin/bash --noprofile --norc \
        scripts/verify-consolidation-core --expected-head EXACT_REVIEWED_COMMIT \
        --output /absolute/already-owned-parent/fresh-receipt
)
```

The clone commands are not an implicit grant and are not part of the verification script. Preserve
the resulting checkout and receipt. For compile-only journey evidence, append exactly
`--compile-local-transport` to the wrapper call after the coordinator admits that compiler slot.
Never run `local_transport` without `--no-run` under this recipe. The compiler slot is coordination,
not a new runtime/authority approval mechanism installed by the script.

## Receipt and failure meanings

The atomic new-directory claim refuses an existing file, directory or dangling symlink. Its parent
must be canonical, owned and writable; the output cannot be within the source checkout. No overwrite
or deletion of earlier evidence. The script sets creation umask `077` only in its own process and
children, atomically creates the new receipt as `0700`, and verifies it is a nonsymlink directory
owned by the running user with that exact mode before writing receipt files. Ordinary script-created
files/directories therefore start owner-only even with a permissive caller umask. No existing parent,
artifact, account default or permission policy is modified. A failed fresh-leaf verification retains
the leaf but refuses before writing inside it; it has no complete receipt. The caller must preserve
ownership/exclusivity while running;
preflight is not a filesystem race-containment proof. No cleanup/finalizer or background process.

Each fixed command gets `.argv`, `.stdout` and `.stderr`; `exits.tsv` records its actual exit. Head,
dirty states, tool versions, UTC start/end when reached and complete input hashes are retained.
This folder is local verification evidence, not a native Beacon Receipt/Run or authority record.
`source-pins.stdout` is the sha256sum-format input manifest, checked again after Cargo; full source
identity is the supplied Git commit, not a claim that only those representative inputs matter.
The private `target/`, `cargo-home/`, `home/`, `tmp/` remain on success/failure/interruption.

`STATUS.tsv` separately labels preflight, evidence target, optional compile, always-UNRUN transport
execution/full gate/M2/M3 completion and script exit. A command failure stops later commands; they
remain UNRUN. A failing or interrupted attempted target is FAILED, never PASS. Early usage/path
failure exits 2 on stderr without claiming a receipt directory; later prerequisites retain a FAILED
preflight receipt with `failure.txt` for the refusal reason. Exit 1 means an invoked command failed; exit 0 means only the selected commands
passed and the final head/source/status rechecks succeeded. SIGINT/TERM attempt exit accounting;
SIGKILL or storage failure may leave an incomplete receipt. No physical-exit/deadline guarantee.
Read every recorded step/exit/log: a last successful command never erases an earlier failure.

## Connections and deliberately missing proof

The six inherited repository identities are recorded in the existing workspace
`reports/beacon-consolidation-20261004/SOURCES.json` (`source_repositories`), not fetched or copied
by this recipe. [ADR 0002](../adr/0002-unified-agent-office-suite.md) requires native modules in
one codebase/store, not six live services behind a shell. The table is bounded source/documentary
context, not a claim all inherited deltas are integrated or runtime accepted:

| Origin / native boundary | Connection and remaining limit |
| --- | --- |
| Athena / [Work](../briefing.md) | Native Project/Team/Mission/completion owners feed existing Outcome collectors. This pure gate does not exercise that workflow or prove Assignment/Attention producers. |
| Buzz / [Conversation](../conversations.md) | Thread/attachment integration remains a separately owned contract/implementation dependency; no Thread claim follows from this test. |
| Minerva / [Evidence](../knowledge.md) | `verify_source_span` verifies complete raw digest, UTF-8 byte range and exact quote. No native Claim/Finding storage, resolver, access or truth/admission decision is established. |
| Vulcan / [Inference](../models.md) | Existing model inspection/eligibility and stored observations are distinct from invocation authority. No discovery/provider/model call occurs in this target. |
| Janus / [Authority](../decisions.md) | Proposal/Ruling bindings are existing owners; digest equality confers no Ruling or permission. This selected test does not exercise auth/policy. |
| Icarus / [Execution](../execution.md) and [Evaluation](../evaluations.md) | Receipt/check/evaluation foundations remain scoped. No dispatch, containment, real evaluation or publication is demonstrated. ReplayDeck is native design, not a seventh imported tree. |

The existing `local_transport` command-journey witness connects synthetic native setup/completion
to canonical Outcome Candidate Markdown/digest and restart read-back. Its optional compilation here
does not establish retained fixture behavior or runtime success. This author's base does not include
Astra's round-2 fixture edits; final integration must pin their reviewed successor explicitly.

Full [contributor gates](../../CONTRIBUTING.md#the-gate) remain required for full-gate claims.
Workspace Rust tests reach the held PDF/cleanup families; desktop stages require npm/Tauri/sidecar
effects and existing log teardown. None is silently waived. Delivery Item2 HOLD/NOT_ADMITTED/UNRUN/
zero attempts, B-ON3 execution/cleanup, B1/M1, B7, G3/capacity, P1-Muse-first, Q2A3/C1 and dormant
Par remain. Selected core PASS never closes those holds, independent review or Muse exact-head
landing. No push, merge, deployment, hosted Actions or M1/M2/M3 completion is authorized here.
