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
| `binding` | Janus entries only. Passed through verbatim from Janus as `{kind, locator, sha256}`. |
| `binding_holds` | Janus entries only. Tri-state — see below. |

### The four Janus states

A Janus entry resolves into one of four renderings, and none of them is
interchangeable with another:

| State | Source | Renders |
|---|---|---|
| Re-checked, still binds | `binding_holds: true` | `✔` **authority** |
| Re-checked, no longer binds | `binding_holds: false` | `⚠` stale |
| Bound to something, nobody checked | `binding.kind` set, `binding_holds` absent | `?` unchecked |
| Bound to **nothing** | `binding.kind == "none"` | `⊘` never re-checkable |

Only a literal `true` for `binding_holds` counts. A truthy string like `"yes"`
does not, and a test asserts it does not: the difference between "we checked"
and "something was there" is the entire value of the field.

`binding.kind == "none"` is checked **before** `binding_holds`, deliberately.
A ruling bound to nothing cannot be re-checked against anything ever, so no
field claiming the binding "holds" can promote it to authority. The absence of
something to check beats any claim about having checked it.

This is not a hypothetical case: on the live Janus ledger the distribution is
`git: 8, none: 4, file: 1` — roughly a third of rulings are bound to nothing.
Collapsing that into "nobody checked yet" would tell an operator to go look at
an absence, so the footer splits its advice by what the reader can actually
do: re-check the ruling, versus raise a new gate against the artifact.

A flat `binding_kind` key is accepted as an alias so a hand-written document
need not nest one field; the nested form wins if both appear.

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
| `┆` dashed | Claims authority it cannot demonstrate — stale, unchecked, bound to nothing, or asserted by a module that does not own rulings. |
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
