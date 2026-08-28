"""Beacon legacy desk — a read-only cross-module desk for AI Town.

Beacon legacy desk composes what each module already publishes into one document. It owns
no state, holds no opinions the modules have not published, and grants no
authority: a Janus ruling renders as authority because Janus binds to bytes,
and everything else renders as a module-local fact wearing its owner's badge.

It composes; it does not compute.
"""

__version__ = "0.1.0"
