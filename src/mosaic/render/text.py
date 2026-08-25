"""Human-readable rendering of a `desk.v1` document.

The layout does real work. Visual weight tracks **what a human has to do**,
never how strong an approval is — a Janus ruling with a broken binding is the
loudest thing on the page, and an Icarus plan approval that correctly refused
is quiet. Any styling that made the strongest mechanism the loudest would be
the ranking `docs/authority.md` bans, wearing a stylesheet.

Three things are never collapsed here, because each collapse was a real bug
caught in this project rather than a hypothesis:

* an approval's four axes into one verdict word,
* "no approval was ever sought" into "an approval could not be verified",
* "nothing to report" into "we could not ask".
"""

from __future__ import annotations

from mosaic.contracts.authority import OperatorAction

from .authority import view, worklist_key

_ACTION_CALL: dict[OperatorAction, str] = {
    OperatorAction.ACT: (
        "Nothing in the system is stopping anyone from relying on this. "
        "Contain it: raise a new gate."
    ),
    OperatorAction.DECIDE: (
        "This will never resolve itself. Accept it as resting on the raiser's "
        "word, or re-raise with a checkable binding."
    ),
}


def render_entry(entry: dict) -> list[str]:
    """Render one desk entry as a list of output lines."""
    resolved = view(entry)
    rule = resolved.rule

    head = f"{rule} {resolved.glyph} "
    head += resolved.headline if resolved.headline else str(entry.get("title") or "(untitled)")
    lines = [head]

    if resolved.headline:
        lines.append(f"{rule}   {entry.get('title') or '(untitled)'}")

    # The module's OWN status word, verbatim and attributed. Never merged with
    # the lifecycle above: `superseded`, `done` and `authorized` come from
    # different vocabularies, and flattening them is how a module-local status
    # starts reading like a fleet-wide verdict.
    source = entry.get("source") or "unknown source"
    facts = [f"{source} · owner {entry.get('owner') or 'unknown'}"]
    if entry.get("status"):
        facts.append(f"{source} says: {entry['status']}")
    if entry.get("ref") or entry.get("id"):
        facts.append(str(entry.get("ref") or entry.get("id")))
    lines.append(f"{rule}   {' · '.join(facts)}")

    for axis in resolved.axes:
        lines.append(f"{rule}   {axis}")

    # The owning module's own words about what its approval means. Mosaic is
    # not entitled to restate another module's semantics more briefly.
    if resolved.authority and resolved.authority.meaning:
        lines.append(f'{rule}   "{resolved.authority.meaning}"')

    call = _ACTION_CALL.get(resolved.action)
    if call and resolved.authority:
        lines.append(f"{rule}   → {call}")

    trail = [str(x) for x in (entry.get("timestamp"), entry.get("schema_version")) if x]
    if entry.get("link"):
        trail.append(f"→ {entry['link']}")
    if trail:
        lines.append(f"{rule}   {' · '.join(trail)}")

    return lines


def _unreachable_lines(document: dict) -> list[str]:
    """Describe the modules that did not answer, or return an empty list.

    A desk is composed FROM modules, so a module that did not answer is a hole
    in the document, not an absence of records. "Nothing to report" and "we
    could not ask" are opposite facts and one of them needs an operator.
    """
    raw = document.get("unreachable") or []
    lines = []
    for item in raw:
        if isinstance(item, dict):
            name = item.get("module") or item.get("name") or "unknown module"
            trail = " · ".join(
                str(x) for x in (item.get("address"), item.get("reason") or item.get("detail")) if x
            )
            lines.append(f"  {name}" + (f" — {trail}" if trail else ""))
        else:
            lines.append(f"  {item}")
    return lines


def render(document: dict, worklist: bool = False) -> str:
    """Render a `desk.v1` document as text.

    Args:
        document: the desk document.
        worklist: order by what a human must do rather than by recency. The
            document keeps its own order; this is a *view* of it. The heading
            says "needs you first" and never "most important" — a reader who
            learns that top-means-important concludes that top-means-
            trustworthy, which rebuilds the banned ranking in the one place
            nobody thinks to look for it.
    """
    entries = list(document.get("entries") or [])
    schema = document.get("schema", "desk.v1")
    unreachable = _unreachable_lines(document)

    header = f"DESK · {schema} · {len(entries)} "
    header += "entry" if len(entries) == 1 else "entries"
    if document.get("generated_at"):
        header += f" · composed {document['generated_at']}"
    out = [header, ""]

    if not entries:
        if unreachable:
            n = len(unreachable)
            out.append(f"(no entries — but {n} module{'' if n == 1 else 's'} did not answer,")
            out.append(" so this desk is INCOMPLETE, not empty. What is missing is unknown.)")
            out.extend(["", *unreachable, ""])
            out.append("Run `mosaic doctor` for the address each one was tried at.")
        else:
            out.append("(no entries — every module answered and none had anything to show)")
            out.append("")
            out.append("This is a genuinely empty desk: nothing was hidden by a failed read.")
        return "\n".join(out)

    if worklist:
        entries = sorted(entries, key=worklist_key)
        out.insert(1, "Needs you first — ordered by what you have to do, not by importance.")

    for entry in entries:
        out.extend(render_entry(entry))
        out.append("")

    views = [view(e) for e in entries]
    act = [v for v in views if v.action is OperatorAction.ACT]
    decide = [v for v in views if v.action is OperatorAction.DECIDE]
    unreadable = [v for v in views if v.problem]
    approvals = sum(1 for v in views if v.authority)

    # The counts are a guard, not a summary. They count WORK, never authority:
    # there is deliberately no "N entries are approved" line, because that
    # number is the collapse this whole project exists to prevent.
    out.append("─" * 72)
    out.append(f"{len(act)} need you to act · {len(decide)} need you to decide · ")
    out[-1] += f"{len(views) - len(act) - len(decide)} need nothing from you."
    # Worded without the collapse vocabulary on purpose. The global test that
    # no forbidden word is rendered anywhere is more valuable than the one
    # sentence it costs, and "records a decision" says the same thing.
    out.append(
        f"{approvals} of {len(views)} entries record a decision; "
        "the rest are work, evidence or conversation."
    )

    if act:
        out.append("")
        out.append("⚑ Act: a binding that was verified is now broken, and the owning module")
        out.append("  reports drift without stopping anyone. Only a person can close that.")
    if decide:
        out.append("")
        out.append("◈ Decide: pinned to nothing re-checkable, now or ever. Not urgent, but it")
        out.append("  will not resolve itself.")
    if unreadable:
        n = len(unreadable)
        out.append("")
        out.append(f"? {n} entr{'y' if n == 1 else 'ies'} carry an authority block Mosaic could")
        out.append("  not read. Reported, not guessed. Fix the adapter, not the renderer.")

    if unreachable:
        n = len(unreachable)
        out.append("")
        out.append(f"✖ {n} module{'' if n == 1 else 's'} did not answer. The counts above are")
        out.append("  incomplete — entries these modules hold are missing, not absent.")
        out.extend(unreachable)

    return "\n".join(out)
