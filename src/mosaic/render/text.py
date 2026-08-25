"""Human-readable rendering of a ``desk.v1`` document.

The layout is doing real work, not decoration. Authority is set apart with a
heavy left rule and an explicit ``AUTHORITY`` tag; every other entry gets a
light rule and its owner's name. An operator scanning the column can see how
many things are actually authorized without reading a word — which is the
failure this renderer exists to prevent, since the collapse it guards against
is a *visual* one.
"""

from __future__ import annotations

from .authority import resolve

_RULE_AUTHORITY = "║"  # heavy double rule: a re-checkable human ruling
_RULE_PLAIN = "│"  # light rule: a module-local fact
_RULE_ATTENTION = "┆"  # dashed rule: something claims authority it cannot show


def _entry_rule(verdict) -> str:
    if verdict.is_authority:
        return _RULE_AUTHORITY
    if verdict.label == "janus-authorized" or verdict.badge in {"?", "⚠"}:
        return _RULE_ATTENTION
    return _RULE_PLAIN


def render_entry(entry: dict) -> list[str]:
    """Render one desk entry as a list of output lines."""
    verdict = resolve(entry)
    rule = _entry_rule(verdict)

    owner = entry.get("owner") or "unknown owner"
    source = entry.get("source") or "unknown source"
    tag = "  AUTHORITY" if verdict.is_authority else ""

    lines = [f"{rule} {verdict.badge} {verdict.display}{tag}"]

    title = entry.get("title")
    if title:
        lines.append(f"{rule}   {title}")

    # The module's OWN status word, kept verbatim and clearly attributed. It is
    # never merged with the authority label above: "merged", "closed" and
    # "ruled" mean different things in different modules, and flattening them
    # is how a module-local status starts reading like a fleet-wide verdict.
    facts = [f"{source} · owner {owner}"]
    status = entry.get("status")
    if status:
        facts.append(f"{source} status: {status}")
    ref = entry.get("ref") or entry.get("id")
    if ref:
        facts.append(str(ref))
    lines.append(f"{rule}   {' · '.join(facts)}")

    trail = []
    timestamp = entry.get("timestamp")
    if timestamp:
        trail.append(str(timestamp))
    schema = entry.get("schema_version")
    if schema:
        trail.append(f"schema {schema}")
    link = entry.get("link")
    if link:
        trail.append(f"→ {link}")
    if trail:
        lines.append(f"{rule}   {' · '.join(trail)}")

    if verdict.note:
        lines.append(f"{rule}   ! {verdict.note}")

    return lines


def _unreachable_lines(document: dict) -> list[str]:
    """Describe the modules that did not answer, or return an empty list.

    A desk is composed FROM modules, so a module that did not answer is a hole
    in the document, not an absence of records. Rendering the two the same way
    is the same collapse this file exists to prevent — "nothing to report" and
    "we could not ask" are opposite facts and one of them needs an operator.
    """
    raw = document.get("unreachable") or []
    if not raw:
        return []

    lines = []
    for item in raw:
        if isinstance(item, dict):
            name = item.get("module") or item.get("name") or "unknown module"
            address = item.get("address")
            reason = item.get("reason") or item.get("detail")
            trail = " · ".join(str(x) for x in (address, reason) if x)
            lines.append(f"  {name}" + (f" — {trail}" if trail else ""))
        else:
            lines.append(f"  {item}")
    return lines


def render(document: dict) -> str:
    """Render a full ``desk.v1`` document as text.

    Args:
        document: A mapping with ``schema``, optional ``generated_at``, and an
            ``entries`` list.

    Returns:
        The rendered document. An empty desk renders as an explicit statement
        that it is empty, never as blank output — blank output is
        indistinguishable from a crash.
    """
    entries = list(document.get("entries") or [])
    schema = document.get("schema", "desk.v1")

    header = f"DESK · {schema} · {len(entries)} "
    header += "entry" if len(entries) == 1 else "entries"
    generated = document.get("generated_at")
    if generated:
        header += f" · composed {generated}"

    out = [header, ""]
    unreachable = _unreachable_lines(document)

    if not entries:
        if unreachable:
            # The bug this replaces claimed "composed cleanly" while the
            # document it was rendering named modules that never answered.
            n = len(unreachable)
            subject = "module" if n == 1 else "modules"
            out.append(f"(no entries — but {n} {subject} did not answer, so this desk is")
            out.append(" INCOMPLETE, not empty. What is missing is unknown, not nothing.)")
            out.append("")
            out.extend(unreachable)
            out.append("")
            out.append("Run `mosaic doctor` for the address each one was tried at.")
        else:
            out.append("(no entries — every module answered and none had anything to show)")
            out.append("")
            out.append("This is a genuinely empty desk: nothing was hidden by a failed read.")
        return "\n".join(out)

    verdicts = [resolve(e) for e in entries]
    for entry in entries:
        out.extend(render_entry(entry))
        out.append("")

    authorized = sum(1 for v in verdicts if v.is_authority)
    contested = [v for v in verdicts if v.label == "janus-authorized" and not v.is_authority]
    unknown = [v for v in verdicts if v.badge == "?" and v.label not in {"janus-authorized"}]

    # The counts are a guard, not a summary. "0 entries render as authority" is
    # the single most important thing this tool can tell an operator, and it
    # must be legible without reading the list above.
    out.append("─" * 72)
    plural = "entry renders" if authorized == 1 else "entries render"
    out.append(f"{authorized} {plural} as authority (a Janus ruling whose binding still holds).")
    others = len(entries) - authorized
    plural_other = "entry is" if others == 1 else "entries are"
    out.append(f"{others} {plural_other} a module-local fact wearing its owner's badge.")

    # Split by whether the reader can actually do anything about it. Telling
    # someone to "re-check the ruling" on a gate bound to nothing sends them
    # to look at an absence, and advice that cannot be followed trains people
    # to ignore the footer.
    unbindable = [v for v in contested if v.badge == "⊘"]
    recheckable = [v for v in contested if v.badge != "⊘"]

    if recheckable:
        n = len(recheckable)
        subject = "entry carries" if n == 1 else "entries carry"
        out.append("")
        out.append(f"⚠ {n} {subject} a Janus label that does not render as authority.")
        out.append("  Follow the canonical link and re-check the ruling in Janus.")
    if unbindable:
        n = len(unbindable)
        subject = "ruling is" if n == 1 else "rulings are"
        out.append("")
        out.append(f"⊘ {n} Janus {subject} bound to nothing and can never be re-checked.")
        out.append("  Nothing to look at — raise a new gate against the artifact instead.")
    if unknown:
        n = len(unknown)
        subject = "entry carries" if n == 1 else "entries carry"
        out.append("")
        out.append(f"? {n} {subject} an unrecognised authority label.")
        out.append("  Reported, not guessed. Fix the adapter rather than the renderer.")

    # A partial read misleads exactly as much as an empty one: the counts above
    # describe what was seen, and this is what was not.
    if unreachable:
        n = len(unreachable)
        subject = "module" if n == 1 else "modules"
        out.append("")
        out.append(f"✖ {n} {subject} did not answer. The counts above are incomplete —")
        out.append("  entries these modules hold are missing, not absent.")
        out.extend(unreachable)

    return "\n".join(out)
