"""Renderers for the ``desk.v1`` document.

Renderers consume the document as a plain mapping rather than the contract
classes, so that whatever shell eventually renders a desk inherits the
authority rules without importing Mosaic's internals.
"""

from __future__ import annotations

from . import json_render, text
from .authority import AUTHORITY_LABELS, AuthorityVerdict, resolve

#: Renderer name -> callable taking a ``desk.v1`` document and returning str.
RENDERERS = {
    "text": text.render,
    "json": json_render.render,
}

DEFAULT_RENDERER = "text"

__all__ = [
    "AUTHORITY_LABELS",
    "DEFAULT_RENDERER",
    "RENDERERS",
    "AuthorityVerdict",
    "json_render",
    "resolve",
    "text",
]
