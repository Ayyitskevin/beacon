"""Where each module actually listens, and how to override it.

Mosaic's configuration surface. Adapters resolve their endpoint through it and
`mosaic doctor` prints what it resolved, so a wrong address is visible in one
command instead of being mistaken for a dead service.

There is no "localhost" default that works. Measured on mickey 2026-08-24 with
`ss -ltnp` and curl in both directions:

    Athena   100.125.80.91:8300   tailnet only -- loopback is connection-refused
    Buzz     100.125.80.91:3000   tailnet only -- loopback is connection-refused
    Minerva  127.0.0.1:8765       loopback only
    Vulcan   127.0.0.1:8140       loopback only

A config that defaults everything to loopback silently loses Athena and Buzz; one
that defaults everything to the tailnet address silently loses Minerva and Vulcan.
So the bind address is a per-module fact, recorded here with the date it was
measured, and overridable by environment for every module.

These defaults are a convenience, not a source of truth. `mosaic doctor` reports
the address it actually tried so that a wrong default is visible in one command
instead of being mistaken for a dead service.
"""

from __future__ import annotations

import os
from dataclasses import dataclass

MEASURED_ON = "2026-08-24"
MEASURED_HOST = "mickey"


@dataclass(frozen=True)
class Endpoint:
    module: str
    address: str
    source: str  # which env var supplied it, or "default (measured <date>)"
    token: str | None = None
    token_source: str | None = None


def _first_env(names: tuple[str, ...]) -> tuple[str | None, str | None]:
    """Return the first set environment variable and which name supplied it.

    Several names per module is not indecision. Athena's own token file exports
    ATHENA_URL / ATHENA_API_TOKEN while most fleet tooling expects
    ATHENA_BASE_URL / ATHENA_TOKEN -- a mismatch that already cost another seat
    enough time to earn an ORACLE troubleshooting page. Mosaic accepts both
    spellings and reports which one it used rather than reproducing the trap.
    """

    for name in names:
        value = os.environ.get(name)
        if value:
            return value, name
    return None, None


# module -> (default address, address env names, token env names)
_DEFAULTS: dict[str, tuple[str, tuple[str, ...], tuple[str, ...]]] = {
    "athena": (
        "http://100.125.80.91:8300",
        ("MOSAIC_ATHENA_URL", "ATHENA_BASE_URL", "ATHENA_URL"),
        ("MOSAIC_ATHENA_TOKEN", "ATHENA_TOKEN", "ATHENA_API_TOKEN"),
    ),
    "minerva": (
        "http://127.0.0.1:8765",
        ("MOSAIC_MINERVA_URL", "MINERVA_BASE_URL", "MINERVA_URL"),
        (),
    ),
    "vulcan": (
        "http://127.0.0.1:8140",
        ("MOSAIC_VULCAN_URL", "VULCAN_BASE_URL", "VULCAN_URL"),
        (),
    ),
    "buzz": (
        "http://100.125.80.91:3000",
        ("MOSAIC_BUZZ_URL", "BUZZ_RELAY_URL"),
        (),
    ),
}


def resolve(module: str) -> Endpoint:
    default, address_names, token_names = _DEFAULTS[module]
    address, address_from = _first_env(address_names)
    token, token_from = _first_env(token_names)
    return Endpoint(
        module=module,
        address=address or default,
        source=address_from or f"default (measured on {MEASURED_HOST} {MEASURED_ON})",
        token=token,
        token_source=token_from,
    )
