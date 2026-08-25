# Mosaic

A read-only cross-module desk for AI Town.

Mosaic composes what Buzz, Athena, Minerva, Vulcan, ReplayDeck, Janus and
Icarus already publish into one document, and prints it. That is the whole
product today.

**It composes; it does not compute.** Every entry points back at the module
that owns it, so the desk can never disagree with the page it links to.

## What Mosaic is not

- **Not a database.** No shared store, no filesystem reach-through into
  another module's state. Each module keeps its own.
- **Not an orchestrator.** It issues no work, holds no queue, and makes no
  decision. A second coordinator is explicitly out of scope.
- **Not a writer.** The adapter protocol has no write method, and a test
  asserts no adapter imports a write-capable client path. Read-only is a
  property of the code here, not a promise in this file.
- **Not an authority.** Mosaic displays who decided what. Reading Mosaic is
  never grounds to act.

## The rule that shapes the output

AI Town has several legitimate approval mechanisms — Buzz's approval kinds,
Buzz workflow approvals, Athena action approvals, Icarus's enforced dispatch
digest, and a Janus ruling. They mean different things, bind to different
objects, and are revoked differently.

> The desk must distinguish **proposed / reviewed / janus-authorized /
> executed / verified**, and must never collapse them into a generic
> "approved".

The danger was never that someone wires a reaction to an approval. It is that
several legitimate approvals become *visually interchangeable*, so an operator
cannot tell which one carries authority over what.

`src/mosaic/render/authority.py` is the executable form of that rule, and
`tests/cli/test_authority.py` is what keeps it true. `docs/authority.md` is
the page that says, for each mechanism, what it binds to, what it unblocks,
how it is revoked, and whether it renders as authority or as history.

## Install

Python 3.11+, zero runtime dependencies.

```sh
python3 -m venv .venv
.venv/bin/pip install -e '.[dev]'
```

## Use

```sh
mosaic render tests/cli/fixtures/desk_sample.json   # render a document you have
mosaic render - --format json < desk.json           # machine-readable, with verdicts
mosaic desk                                         # compose a live desk, then render
mosaic doctor                                       # what is reachable, which joins are missing
```

`mosaic render` depends on nothing outside this package, so it works before
any module is reachable. `desk` and `doctor` exit **69** (`EX_UNAVAILABLE`)
with a sentence naming what is missing, rather than printing empty output — a
missing adapter must never look like a quiet fleet.

A worked example lives at `tests/cli/fixtures/desk_sample.json`; it covers a
holding Janus binding, a broken one, and a module-local fact.

## Working on Mosaic

`AGENTS.md` holds the repository contract. `CONTEXT.md` holds the domain
model and is the definition of record — this README defers to it.

## Status

v0. The first slice is a read-only `desk.v1` document assembled through two
real adapters. A join that does not exist yet is a **result, not a bug**:
`mosaic doctor` names it rather than papering over it with a synthesized link.

## License

Apache-2.0.
