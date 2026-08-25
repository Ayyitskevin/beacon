# Adapters

An adapter answers two questions about one module and nothing else:

```
probe()   -- are you there, at the address I tried, and what are you running?
entries() -- what do you currently hold that belongs on the operator's desk?
```

There is no third method. Mosaic reads seven modules it does not own, and its
whole licence to do so rests on being unable to change them, so the absence of a
write verb is enforced by the shape of the interface rather than by everyone
remembering. `tests/adapters/test_read_only_by_construction.py` fails the build
if that stops being true.

## The three rules

**1. Read through the interface, never around it.** Per ADR 0001: a module's own
published interface is readable, and so is an immutable content-addressed artifact
it already treats as canonical. Its live database is not. `janus list --json` is
fine; opening `~/.janus/janus.db` is not, because that bypasses scopes,
authorization, the activity trail, and observes mid-transaction state. A test
asserts `sqlite3` is imported nowhere in the package.

**2. Describe, do not decide.** An adapter passes through the module's own status
word, the module's own identifiers, and the module's own binding facts. It does
not normalise them into a cross-module vocabulary — that is composition, and it
belongs in one reviewed place rather than at seven edges. Adapters emit
`authority_label = "unlabelled"` and let `docs/authority.md` decide.

**3. A missing thing is a reported thing.** An empty lane with a stated reason is
a result. A plausible lane that cannot be traced back is a lie.

## What was measured, and when

All of this was measured on mickey on 2026-08-24 with `ss -ltnp`, `curl`, and the
modules' own CLIs. It is perishable; `mosaic doctor` re-measures rather than
trusting this table.

| Module | Owns | Read via | Address |
|---|---|---|---|
| Buzz | live conversation, identity, mentions | relay HTTP + `buzz` CLI | `100.125.80.91:3000` — **tailnet only** |
| Athena | missions, work, operator documents | HTTP + Bearer token | `100.125.80.91:8300` — **tailnet only** |
| Minerva | evidence, claims, uncertainty | HTTP, unauthenticated | `127.0.0.1:8765` — **loopback only** |
| Vulcan | inference aliases, admission, usage | HTTP, unauthenticated | `127.0.0.1:8140` — **loopback only** |
| ReplayDeck | deterministic evaluation and replay | — | **does not exist** |
| Janus | pending human decisions | `janus` CLI, read verbs only | local |
| Icarus | bounded execution and receipts | committed git objects, pinned revision | repo-only, no daemon |

### There is no localhost default that works

Athena and Buzz refuse loopback. Minerva and Vulcan only accept it. A config that
defaults everything to one or the other silently loses half the fleet, and the
failure is indistinguishable from a dead service. So the address is a per-module
fact in `src/mosaic/config.py`, overridable per module, and `mosaic doctor` prints
the address it actually tried **including when it worked** — a wrong default
should be visible, not inferred from a refusal.

Related trap, already documented by another seat after it cost them time: Athena's
own token file exports `ATHENA_URL` / `ATHENA_API_TOKEN`, while most fleet tooling
expects `ATHENA_BASE_URL` / `ATHENA_TOKEN`. Mosaic accepts both spellings and
reports which one it used.

### Athena is identity-scoped

Athena authenticates per identity. A token minted for an agent seat sees that
seat's desk, not the operator's. Mosaic must never present Athena's contribution
as the whole of Athena.

### Janus binding kinds, from the live ledger

13 gates: **git 8, none 4, file 1**. There is no `text` kind in the live corpus.
Four approved gates are bound to nothing at all — a state weaker than a text
binding, because a ruling bound to nothing can never be re-checked against
anything. The adapter passes `{kind, locator, sha256}` through verbatim rather
than collapsing it to a boolean, because that distinction is exactly what a
reader needs and exactly what a boolean would erase.

The adapter does **not** re-check whether a binding still holds; that is a
per-gate `janus show` and should be opt-in, not a side effect of reading a desk.

### Capability discovery exists, but is not yet a convention

Two modules self-describe, and their documents disagree. Minerva serves
`minerva.capabilities.v3` (with `schema_version`, listener mode, identity
boundary, per-surface argv); Vulcan serves `/v1/capabilities` (`api_version` +
`callable_capabilities`, no `schema_version`). Mosaic normalises. It does not
declare a standard that its only two adopters already differ on.

**Known defect, re-measured by `mosaic doctor` on every run:** the fleet
entrypoint `~/.local/bin/minerva` injects `--db` into every verb outside a small
allowlist, and `capabilities` accepts none — so `minerva capabilities` fails on
PATH while `~/deploy/minerva/.venv/bin/minerva capabilities` succeeds. Minerva's
own manifest advertises that argv as its CLI discovery path, so the module
correctly declares how to discover it and the canonical entrypoint makes the
declaration false. Fleet infrastructure, not a Mosaic bug, and not worked around
here — reported, because Mosaic relies on the surface. It is a one-line fix in the
wrapper's allowlist and it is Kevin's call to make.

## Adding an adapter

1. Implement `probe()` and `entries()`. Take I/O from `transport.py`; do not
   import `subprocess`, `urllib`, or a database driver yourself — a test enforces this.
2. Build entries with `base.entry()`, never `Entry(...)` directly. That is the
   single seam where Lane A's contract plugs in.
3. Register it in `adapters/__init__.py` with what it owns and how it is read.
4. If you cannot build it, register it anyway with `factory=None` and a reason.
   The roadmap is explicit that knowing which joins are missing is worth more
   than a shell, and a registry that lists only what was built cannot say what
   was not.
