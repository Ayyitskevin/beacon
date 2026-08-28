# Beacon Legacy Desk rendering

> Historical research originally built under the Mosaic name. The maintained compatibility
> package is now `beacon_legacy` and its command is `beacon-legacy-desk`.

How Mosaic turns a `desk.v1` document into output, and the one rule that
shapes every decision here.

**This page is subordinate to `docs/authority.md`.** That page decides *what
the authority labels mean and which mechanism decides over what*. This page
only describes how the renderer makes that visible. Where the two disagree,
`docs/authority.md` wins and this page is the bug.

## The seam

Renderers consume the desk **document as a plain mapping** and rebuild the
contract's `Authority` from it via `render.authority.read_authority`. The
policy is never re-implemented here — it is imported from
`beacon_legacy.contracts.authority` and *asked*, so the renderer and the contract
cannot drift apart. A test walks every `Attachment` x `Enforcement` pair and
asserts the view agrees with `Authority.operator_action()`.

```
adapters -> desk.v1 document -> read_authority() -> Authority -> text | json
```

## The four axes

An approval is rendered across four orthogonal axes and never summarised:

| Axis | Question |
|---|---|
| `decider` | who made the decision — `human` / `policy` / `agent` |
| `attachment` | what it is pinned to, and what checking it just produced |
| `enforcement` | who acts when the binding stops holding |
| `lifecycle` | where the thing is in its journey |

The operationally important pair is `enforcement` x `attachment`, and the
contract answers it with `operator_action()`:

| | Meaning | Rendered |
|---|---|---|
| `ACT` | `reported` + `bytes-drifted`. Nothing is stopping anyone. | `║ ⚑` loud |
| `DECIDE` | `unbound` or `bytes-unverifiable`. Never resolvable. | `┆ ◈` |
| `NONE` | including `enforced` + `bytes-drifted` — the module already refused. | `│ ·` quiet |

**Visual weight tracks what a human must do, never how strong an approval
is.** It is deliberately anti-correlated with authority strength: a Janus
ruling with a broken binding is the loudest thing on the page, and an Icarus
plan approval that correctly refused is quiet. Styling that made the strongest
mechanism the loudest would be the banned ranking wearing a stylesheet.

There is no `is_authority`, no score, and no "N entries are approved" count
anywhere. An earlier version of this renderer had all three, and they were the
binary axis `docs/authority.md` refuses.

## Three collapses, each a real defect

Every one of these was found in this renderer, not hypothesised.

**1. Four axes into one verdict word.** The original renderer resolved a
single string label and answered "is this authority?" That premise was wrong:
Icarus binds a wider digest than Janus *and enforces it*, while Janus
deliberately does not enforce so it stays out of the permission path.

**2. "No approval" rendered as "approval could not be verified."** `authority`
is `None` for most entries — work, evidence, conversation — and that is the
common case, not a failed check. `AuthorityView.problem` is what separates
*absent* from *unreadable*; callers must check it rather than treating
`authority is None` as one thing.

**3. "Nothing to report" rendered as "we could not ask."** The renderer reads
`unreachable` and never describes a desk as cleanly composed while it is
populated. Each module is named with the address actually tried, because the
fleet disagrees about loopback versus tailnet.

An unreadable authority value is **reported, never guessed**. Mapping an
unknown value onto a known one is the collapse this file exists to prevent.
`view()` never raises: a renderer that crashes on one bad entry hides the
other forty.

## The worklist view

`--worklist` orders by `ACT` -> `DECIDE` -> `NONE`, ties broken by recency so
no secondary sort smuggles in an authority ordering. `docs/authority.md`
permits this: the ban is on ordering that *correlates with authority
strength*, and this is anti-correlated with it.

Three conditions, all pinned by tests:

1. The document keeps its own order — the worklist is a **view**, not a
   reordering. Default rendering preserves compose's recency order.
2. Ties break on recency.
3. **The heading says "needs you first", never "most important" or
   "priority".** A reader who learns top-means-important will conclude
   top-means-trustworthy, rebuilding the binary in the one place nobody thinks
   to look. This is the condition most likely to be lost in a future edit.

## Refusals

Refusals are sentences, not tracebacks. `beacon-legacy-desk desk` / `beacon-legacy-desk doctor` exit
**69** (`EX_UNAVAILABLE`) naming what is missing; malformed input exits **1**.
`beacon-legacy-desk desk` exits **0** even when the desk is incomplete: Buzz, ReplayDeck
and Icarus are expected unreachable in normal conditions, and a command that
almost always fails trains people to ignore it. Incompleteness is carried in
the document and the footer instead.

## Adding a renderer

Add it to `RENDERERS` in `render/__init__.py`. Call `view()`; never re-derive
the operator action, and never add a path that can present two different axes
identically. `tests/cli/test_authority.py` is the guard.
