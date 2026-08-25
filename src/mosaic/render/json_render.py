"""Machine-readable rendering of a ``desk.v1`` document.

The JSON form carries the document through unchanged and *adds* a resolved
``authority_verdict`` to each entry. That addition is the point: every machine
consumer would otherwise re-implement the "is this actually authorized?"
question for itself, and the first one to get it slightly wrong reintroduces
the collapse. One resolver, shared by the text and JSON paths, means a
downstream shell can read ``authority_verdict.is_authority`` and be right.
"""

from __future__ import annotations

import json
from dataclasses import asdict

from .authority import resolve


def to_dict(document: dict) -> dict:
    """Return the document with a resolved authority verdict on every entry.

    The input is not mutated, and no original key is overwritten — the desk
    document remains the source of truth and this is strictly an annotation.
    """
    entries = []
    for entry in document.get("entries") or []:
        annotated = dict(entry)
        annotated["authority_verdict"] = asdict(resolve(entry))
        entries.append(annotated)

    out = dict(document)
    out["entries"] = entries
    out["authority_summary"] = {
        "total": len(entries),
        "renders_as_authority": sum(1 for e in entries if e["authority_verdict"]["is_authority"]),
        # Named so a consumer cannot read it as "everything else is fine".
        "module_local_facts": sum(1 for e in entries if not e["authority_verdict"]["is_authority"]),
    }
    return out


def render(document: dict) -> str:
    """Render a ``desk.v1`` document as indented JSON with stable key order."""
    return json.dumps(to_dict(document), indent=2, sort_keys=True, ensure_ascii=False)
