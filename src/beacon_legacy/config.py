"""Per-module illustrative endpoints, overridden by explicit environment settings.

The defaults below are examples, not a working fleet map. Configure every module
for your installation before use. Doctor reports the exact selected endpoint.
Address/token naming-family binding remains enforced; an endpoint override does
not borrow a credential from another family.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from urllib.parse import urlsplit

MEASURED_ON = "example"
MEASURED_HOST = "node-a"


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
    enough time to earn an ORACLE troubleshooting page. Beacon legacy desk accepts both
    spellings and reports which one it used rather than reproducing the trap.
    """

    for name in names:
        value = os.environ.get(name)
        if value:
            return value, name
    return None, None


#: module -> (default address, ordered (address_env, token_env) PAIRS)
#:
#: The names are **paired**, not two independent lists, and that pairing is the
#: whole of the credential rule below. Each pair is one naming family: the
#: `BEACON_LEGACY_*` family, the deprecated pre-rename `MOSAIC_*` compatibility family,
#: the family most fleet tooling uses, and the family Athena's own token file exports.
_DEFAULTS: dict[str, tuple[str, tuple[tuple[str, str | None], ...]]] = {
    "athena": (
        "http://192.0.2.10:18300",
        (
            ("BEACON_LEGACY_ATHENA_URL", "BEACON_LEGACY_ATHENA_TOKEN"),
            ("MOSAIC_ATHENA_URL", "MOSAIC_ATHENA_TOKEN"),
            ("ATHENA_BASE_URL", "ATHENA_TOKEN"),
            ("ATHENA_URL", "ATHENA_API_TOKEN"),
        ),
    ),
    "minerva": (
        "http://127.0.0.1:18765",
        (
            ("BEACON_LEGACY_MINERVA_URL", None),
            ("MOSAIC_MINERVA_URL", None),
            ("MINERVA_BASE_URL", None),
            ("MINERVA_URL", None),
        ),
    ),
    "vulcan": (
        "http://127.0.0.1:18140",
        (
            ("BEACON_LEGACY_VULCAN_URL", None),
            ("MOSAIC_VULCAN_URL", None),
            ("VULCAN_BASE_URL", None),
            ("VULCAN_URL", None),
        ),
    ),
    "buzz": (
        "http://192.0.2.10:13000",
        (("BEACON_LEGACY_BUZZ_URL", None), ("MOSAIC_BUZZ_URL", None), ("BUZZ_RELAY_URL", None)),
    ),
}


def resolve(module: str) -> Endpoint:
    """Bind credentials only to an explicit, same-family non-example address.

    Defaults are illustrations, not trusted destinations. Naming a default
    explicitly does not convert it into an authorized credential recipient.
    """

    default, families = _DEFAULTS[module]

    for address_name, token_name in families:
        address = os.environ.get(address_name)
        if not address:
            continue
        token = os.environ.get(token_name) if token_name else None
        unbound = _token_set_in_another_family(families, address_name)
        if _illustrative_destination(address, default):
            token = None
            source = "none — illustrative destination; configure a real endpoint"
        elif token:
            source = token_name
        elif unbound:
            source = f"none — {unbound} is set but is not paired with {address_name}"
        else:
            source = None
        return Endpoint(
            module=module,
            address=address,
            source=address_name,
            token=token,
            token_source=source,
        )

    return Endpoint(
        module=module,
        address=default,
        source="default (example; configure explicitly)",
        token_source="none — no explicit endpoint/token binding",
    )


def _illustrative_destination(address: str, default: str) -> bool:
    """Do not send credentials to the example host, even with URL decoration."""

    try:
        return urlsplit(address).hostname == urlsplit(default).hostname
    except ValueError:
        return True


def _token_set_in_another_family(
    families: tuple[tuple[str, str | None], ...], chosen_address_name: str
) -> str | None:
    """Name the token variable that was set but deliberately not used."""

    for address_name, token_name in families:
        if address_name != chosen_address_name and token_name and os.environ.get(token_name):
            return token_name
    return None
