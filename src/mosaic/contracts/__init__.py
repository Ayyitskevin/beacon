"""Mosaic's contracts: the desk envelope, authority labels, adapter protocol.

Import from here. The module split beneath is an implementation detail.
"""

from mosaic.contracts.adapter import Adapter, Capability
from mosaic.contracts.authority import (
    NOT_ASSURING,
    Attachment,
    Authority,
    Decider,
    Enforcement,
    Lifecycle,
    OperatorAction,
)
from mosaic.contracts.desk import SCHEMA, Desk, Entry, Provenance, Unreachable

__all__ = [
    "NOT_ASSURING",
    "SCHEMA",
    "Adapter",
    "Attachment",
    "Authority",
    "Capability",
    "Decider",
    "Desk",
    "Enforcement",
    "Entry",
    "Lifecycle",
    "OperatorAction",
    "Provenance",
    "Unreachable",
]
