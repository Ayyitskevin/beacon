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

    if not entries:
        out.append("(no entries — the desk composed cleanly and found nothing to show)")
        out.append("")
        out.append(
            "Run `mosaic doctor` to see whether that is an empty fleet or an unreachable one."
        )
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

    if contested:
        n = len(contested)
        subject = "entry carries" if n == 1 else "entries carry"
        out.append("")
        out.append(f"⚠ {n} {subject} a Janus label that does not render as authority.")
        out.append("  Follow the canonical link and re-check the ruling in Janus.")
    if unknown:
        n = len(unknown)
        subject = "entry carries" if n == 1 else "entries carry"
        out.append("")
        out.append(f"? {n} {subject} an unrecognised authority label.")
        out.append("  Reported, not guessed. Fix the adapter rather than the renderer.")

    return "\n".join(out)
