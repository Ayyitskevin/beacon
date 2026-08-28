"""Machine-readable rendering of a `desk.v1` document.

The document passes through unchanged; this adds one computed block per entry.

That addition exists because a JSON consumer holds a plain mapping and cannot
call `Authority.operator_action()`. Without it, every consumer re-implements
"what does this approval require of me?", and the first one to get it slightly
wrong reintroduces the collapse. One resolver, shared with the text renderer,
means a downstream shell can read `operator_action` and be right.

Note what is NOT emitted: there is no `renders_as_authority` count and no
`is_authority` flag. An earlier version had both, and they were precisely the
binary axis `docs/authority.md` refuses — a summary that reduced four
orthogonal facts to one number, computed into the document where every
consumer would inherit it.
"""

from __future__ import annotations

import json

from beacon_legacy.contracts.authority import OperatorAction

from .authority import view


def to_dict(document: dict) -> dict:
    """Return the document with a resolved authority view on every entry.

    The input is not mutated and no original key is overwritten — the desk
    document stays the source of truth and this is strictly an annotation.
    """
    entries = []
    for entry in document.get("entries") or []:
        resolved = view(entry)
        annotated = dict(entry)
        annotated["authority_view"] = {
            # Three distinguishable states, never two: an entry with no
            # approval is not an entry whose approval failed to read.
            "has_approval": resolved.authority is not None,
            "unreadable": resolved.problem,
            "operator_action": resolved.action.value,
            "assures": resolved.assures,
            "decider": resolved.authority.decider.value if resolved.authority else None,
            "attachment": resolved.authority.attachment.value if resolved.authority else None,
            "enforcement": resolved.authority.enforcement.value if resolved.authority else None,
            "lifecycle": resolved.authority.lifecycle.value if resolved.authority else None,
            "mechanism": resolved.authority.mechanism if resolved.authority else None,
            "spent": resolved.authority.spent if resolved.authority else None,
        }
        entries.append(annotated)

    out = dict(document)
    out["entries"] = entries
    # Counts of WORK, not of authority. "How many are approved?" is not a
    # question this document will answer, by design.
    out["operator_actions"] = {
        action.value: sum(1 for e in entries if e["authority_view"]["operator_action"] == action)
        for action in OperatorAction
    }
    return out


def render(document: dict) -> str:
    """Render a `desk.v1` document as indented JSON with stable key order."""
    return json.dumps(to_dict(document), indent=2, sort_keys=True, ensure_ascii=False, default=str)
