# Rendering a desk

How Mosaic turns a `desk.v1` document into output, and the one rule that
shapes every decision here.

**This page is subordinate to `docs/authority.md`.** That page decides *what
the authority labels mean and which mechanism decides over what*. This page
only describes how the renderer makes that visible. Where the two disagree,
`docs/authority.md` wins and this page is the bug.

## The seam

Renderers consume the desk **document as a plain mapping** — not the contract
classes, not adapter objects. That is deliberate. The display layer depends on
the document, so any shell that later renders a desk inherits the authority
rules without importing Mosaic's internals, and neither the contracts lane nor
the adapters lane is coupled to the display layer.

```
adapters  ->  desk.v1 document (plain dict)  ->  resolve()  ->  text | json
```

`resolve()` in `render/authority.py` is the single place the authority
question is answered. Both renderers call it, so the text and JSON paths
cannot drift apart on what counts as authority — a test asserts they agree.

### Fields read per entry

| Field | Meaning |
|---|---|
| `source` | Owning module. Only this decides whether an entry *can* render as authority. |
| `owner` | Who within that module holds it. |
| `title` | One-line human description. |
| `status` | The module's **own** status word, rendered verbatim and attributed. |
| `timestamp` | When the observation was taken. |
| `schema_version` | The owning module's schema for this entry. |
| `ref` / `id` | The module-local identifier. |
| `link` | Canonical link back to the owning module. |
| `authority` | The authority label. |
| `binding_holds` | Janus entries only. Tri-state — see below. |

`binding_holds` is tri-state and the three states are **not** interchangeable:

- `true` — re-checked, still binds
- `false` — re-checked, no longer binds
- absent / `null` — nobody checked

Only a literal `true` counts. A truthy string like `"yes"` does not, and a
test asserts it does not: the difference between "we checked" and "something
was there" is the entire value of the field.

## The rule

> The output must distinguish **proposed / reviewed / janus-authorized /
> executed / verified**, and must never collapse them into a generic
> "approved".

The failure this prevents is a *visual* one. AI Town has several legitimate
approval mechanisms that bind to different objects and are revoked
differently. The risk was never that someone wires a reaction to an approval;
it is that several legitimate approvals become indistinguishable on screen, so
an operator can no longer tell which one carries authority over what.

So the layout does real work:

| Rule | Meaning |
|---|---|
| `║` heavy | Renders as authority. Carries an explicit `AUTHORITY` tag. |
| `┆` dashed | Claims authority it cannot demonstrate — stale, unchecked, or asserted by a module that does not own rulings. |
| `│` light | A module-local fact wearing its owner's badge. |

An operator can count the heavy rules without reading a word. That is the
point: the summary line reports how many entries render as authority, because
**"0 entries render as authority" is the single most important thing this tool
can say**, and it must be legible without reading the list.

The module's own `status` is always rendered verbatim and attributed to its
module (`athena status: in_review`). It is never merged into the authority
line — "merged", "closed" and "ruled" mean different things in different
modules, and flattening them is how a module-local status starts reading like
a fleet-wide verdict. The sample desk contains two entries whose module status
is both literally `approved` and which render completely differently; that is
the rule working.

## Unrecognised labels

An unrecognised label is reported as unrecognised. It is never mapped onto the
nearest known label, because that mapping **is** the collapse this renderer
exists to prevent. `resolve()` does not raise on a bad label either — a
renderer that crashes on one malformed entry hides the other forty.

Fix the adapter, not the renderer.

## Refusals

Following the sibling convention in Janus: refusals are sentences, not
tracebacks. `mosaic desk` and `mosaic doctor` exit **69** (`EX_UNAVAILABLE`)
naming what is missing when their lane has not landed, and print nothing to
stdout. Malformed input exits **1**. A caller scripting around Mosaic can tell
"not built yet" from "you gave me garbage".

An empty desk renders as an explicit statement that it is empty. Blank output
is indistinguishable from a crash.

## Adding a renderer

Add it to `RENDERERS` in `render/__init__.py` and it appears as a `--format`
choice everywhere. Call `resolve()`; do not re-derive the authority question,
and do not add a code path that can present two different labels identically.
`tests/cli/test_authority.py::TestRenderersHoldTheLine` is the guard, and a
new renderer belongs under it.
