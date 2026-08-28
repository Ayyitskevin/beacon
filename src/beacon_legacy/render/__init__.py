"""Renderers for the ``desk.v1`` document.

Renderers consume the document as a plain mapping and rebuild the contract's
`Authority` from it, so any shell that later renders a desk inherits the
authority rules without depending on how Beacon legacy desk assembled the document. The
policy itself is never re-implemented here — it is imported from
`beacon_legacy.contracts.authority` and asked.
"""

from __future__ import annotations

from . import json_render, text
from .authority import AuthorityView, has_approval, read_authority, view, worklist_key

#: Renderer name -> callable taking a ``desk.v1`` document and returning str.
RENDERERS = {
    "text": text.render,
    "json": json_render.render,
}

DEFAULT_RENDERER = "text"

__all__ = [
    "DEFAULT_RENDERER",
    "RENDERERS",
    "AuthorityView",
    "has_approval",
    "json_render",
    "read_authority",
    "text",
    "view",
    "worklist_key",
]
