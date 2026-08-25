# Adapters

An adapter answers two questions about one module and nothing else:

```
capabilities() -- what do you advertise, and are you listening at all?
read()         -- what do you currently hold, and what could I not reach?
```

There is no third method. `read()` returns `(entries, unreachable)` and **never
raises**: an empty read and a failed read are different facts about the world, and
a desk that renders them alike is lying by omission. There is a third state too —
reachable and contributing nothing — which Minerva and Vulcan both occupy today. Mosaic reads seven modules it does not own, and its
whole licence to do so rests on being unable to change them, so the absence of a
write verb is enforced by the shape of the interface rather than by everyone
remembering. `tests/adapters/test_read_only_by_construction.py` fails the build
if that stops being true.

## The three rules

**0. The allowlist is not the caller's to choose.** `transport.READ_VERBS` is a
literal table keyed by module, and `ReadOnlyCommand` looks itself up in it. The
first version took the verb set as a constructor argument, which made the
constraint circular — the adapter being restricted chose its own restrictions,
and claude-2f broke it in review by writing an adapter that declared
`{"decide", "withdraw"}` and reached `subprocess.run`. A module with no entry in
the table may run nothing at all: it fails closed.

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

All of this was measured on node-a on 2026-08-24 with `ss -ltnp`, `curl`, and the
modules' own CLIs. It is perishable; `mosaic doctor` re-measures rather than
trusting this table.

| Module | Owns | Read via | Address |
|---|---|---|---|
| Buzz | live conversation, identity, mentions | relay HTTP + `buzz` CLI | `192.0.2.10:13000` — **tailnet only** |
| Athena | missions, work, operator documents | HTTP + Bearer token | `192.0.2.10:18300` — **tailnet only** |
| Minerva | evidence, claims, uncertainty | HTTP, unauthenticated | `127.0.0.1:18765` — **loopback only** |
| Vulcan | inference aliases, admission, usage | HTTP, unauthenticated | `127.0.0.1:18140` — **loopback only** |
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
PATH while `./deployment-example/minerva/.venv/bin/minerva capabilities` succeeds. Minerva's
own manifest advertises that argv as its CLI discovery path, so the module
correctly declares how to discover it and the canonical entrypoint makes the
declaration false. Fleet infrastructure, not a Mosaic bug, and not worked around
here — reported, because Mosaic relies on the surface. It is a one-line fix in the
wrapper's allowlist and it is Kevin's call to make.

## Adding an adapter

1. Implement `capabilities()` and `read()`. Take I/O from `transport.py`; do not
   import `subprocess`, `os`, `pathlib`, `importlib`, `urllib`, or a database
   driver yourself — a test enforces this.
2. Declare your read verbs in `transport.READ_VERBS` if you read through a CLI.
   Omitting it means your adapter may run nothing, which is the intended failure.
3. Use the contract enums for every label. If a module has a state the contract
   cannot express, bring the case to the authority page — do not invent a value.
3. Register it in `adapters/__init__.py` with what it owns and how it is read.
4. If you cannot build it, register it anyway with `factory=None` and a reason.
   The roadmap is explicit that knowing which joins are missing is worth more
   than a shell, and a registry that lists only what was built cannot say what
   was not.

## Where the contract did not fit, and what happened

`Entry.status` used to be typed as `Lifecycle`, the roadmap's five-value approval
vocabulary. Two of the four live modules have statuses that were not drawn from
one, and writing the mapping tables was what made the defect visible:

- **Janus** has no terminal-negative value to land on. `withdrawn`, `expired` and
  `superseded` all mapped to `proposed` — so **4 of 13 live gates rendered exactly
  like a live open gate.** A question the world moved past and a question nobody
  has answered are different facts, and that is this project's own failure mode
  one level up: the same shape as `unbound` vs `bytes-unchecked`.
- **Athena** issues are *work*. `done` mapped to `executed` rather than `verified`,
  because Athena's own desk protocol says "complete does not close the issue" and
  `verified` would assert a check nobody ran.

Raised to Lane A rather than patched per-adapter, on the grounds that a mapping
table in six adapter files is a doctrine decision living in the wrong place. Lane A
ruled the field back to `str` and found a second defect underneath: `Entry.status`
duplicated `Authority.lifecycle`, so two fields carried one concept and had room
to disagree.

**Now:** `status` is the owning module's own word, verbatim, mapped by nobody. The
approval vocabulary lives only on `Authority.lifecycle`, populated only when there
is an approval — so an entry with no approval borrows no approval word.

## Mosaic's own guarantee

*Written for `docs/authority.md`; lift it whole rather than paraphrasing.*

Mosaic argues that other systems' guarantees should be stated precisely — that
Icarus enforcing a digest on read and Janus reporting drift without enforcing are
different obligations, and that collapsing them is the failure to avoid. A tool
making that argument has no business being vague about its own guarantee.

**What is guaranteed.** Mosaic reads seven modules it does not own and cannot
write to any of them through its own interfaces. The adapter Protocol has no write
method, no dispatch, and no generic `call`. All I/O goes through `transport.py`,
which exposes exactly two entry points: an HTTP client whose method is the literal
constant `GET`, and a subprocess runner restricted by `transport.READ_VERBS`, a
literal table keyed by module. A module absent from that table may run nothing at
all — it fails closed. Tests read both the method and the table out of the AST, so
the guarantee is checked rather than reviewed.

**How that was established.** The first version was broken, and a non-author
reviewer broke it. `ReadOnlyCommand` took its verb allowlist as a constructor
argument, which meant the adapter being restricted chose its own restrictions: an
adapter declaring `{"decide", "withdraw"}` reached `subprocess.run` and passed
every read-only test in the suite. With `janus` as the executable, that rules on a
gate. The control was taking its bounds from the thing it bounded.

The fix moved the allowlist onto the transport. It was then verified by
re-creating the exploit and re-running it against the fix — `decide` raises
`PermissionError`, and the AST scan now fails on the proof adapter — rather than
by accepting the fix on trust.

**What it does not close.** The import scan proves no adapter has an I/O path *of
its own*. It does not prove no path exists, and the difference matters:

- `open()` is a builtin. No import scan can see it, and it writes.
- Any future adapter is one unreviewed commit away from a path nobody scanned for.
- The guarantee covers Mosaic's own code. It says nothing about what a module does
  with a read request it receives.

`os`, `importlib`, `pathlib`, `shutil` and `tempfile` are banned by name, and the
`__import__` builtin is caught separately, because banning `importlib` alone would
have left dynamic import as a one-line bypass. Bare `open()` remains reachable and
is deliberately not chased: the honest boundary is the transport seam, not the
language.

The earlier version of this claim said an AST scan "proves no path exists." That
overstatement was the real hazard, more than the hole it described — someone who
believes the strong version stops looking.


> Privacy rewrite: machine names, paths and endpoint examples in this document are illustrative, not a live infrastructure map. Historical measurements and prior byte-hash/review claims refer to retained original evidence, not these edited public bytes; no authority transfers through this rewrite.
