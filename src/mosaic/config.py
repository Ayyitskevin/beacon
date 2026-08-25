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


#: module -> (default address, ordered (address_env, token_env) PAIRS)
#:
#: The names are **paired**, not two independent lists, and that pairing is the
#: whole of the credential rule below. Each pair is one naming family: the
#: `MOSAIC_*` family, the family most fleet tooling uses, and the family Athena's
#: own token file exports.
_DEFAULTS: dict[str, tuple[str, tuple[tuple[str, str | None], ...]]] = {
    "athena": (
        "http://100.125.80.91:8300",
        (
            ("MOSAIC_ATHENA_URL", "MOSAIC_ATHENA_TOKEN"),
            ("ATHENA_BASE_URL", "ATHENA_TOKEN"),
            ("ATHENA_URL", "ATHENA_API_TOKEN"),
        ),
    ),
    "minerva": (
        "http://127.0.0.1:8765",
        (("MOSAIC_MINERVA_URL", None), ("MINERVA_BASE_URL", None), ("MINERVA_URL", None)),
    ),
    "vulcan": (
        "http://127.0.0.1:8140",
        (("MOSAIC_VULCAN_URL", None), ("VULCAN_BASE_URL", None), ("VULCAN_URL", None)),
    ),
    "buzz": (
        "http://100.125.80.91:3000",
        (("MOSAIC_BUZZ_URL", None), ("BUZZ_RELAY_URL", None)),
    ),
}


def resolve(module: str) -> Endpoint:
    """Resolve one module's endpoint, and its credential only if they are bound.

    **A credential is bound to the endpoint it was issued for. Changing the
    endpoint invalidates the credential rather than sending it somewhere new.**

    Address and token used to be resolved from two independent lists, so setting
    `MOSAIC_ATHENA_URL` alone redirected the address while a token from
    `ATHENA_API_TOKEN` followed it — a live bearer sent to whatever host the
    address variable happened to name. Anyone who can set the environment already
    owns the machine, so this is a footgun rather than a vulnerability; the
    realistic failure is a typo or a copied config pointing somewhere unintended
    and the credential travelling silently. Found by claude-2f in review.

    Now the address and the token must come from the **same naming family**. A
    token from another family is not sent, and `token_source` says why, so the
    result is a visible 401 instead of a credential quietly leaving the fleet.
    """

    default, families = _DEFAULTS[module]

    for address_name, token_name in families:
        address = os.environ.get(address_name)
        if not address:
            continue
        token = os.environ.get(token_name) if token_name else None
        unbound = _token_set_in_another_family(families, address_name)
        if token is None and unbound and address == default:
            # The address was named explicitly but is byte-identical to the
            # measured default, so nothing was redirected and there is nothing to
            # protect the credential from. Blocking here would punish the single
            # most likely real setup -- ATHENA_BASE_URL is what most fleet tooling
            # expects, ATHENA_API_TOKEN is what Athena's own token file exports --
            # with a 401 that reads like an auth problem. Refinement from
            # claude-2f. It cannot weaken the redirect case: an attacker-supplied
            # host is not the default, so it still loses the token.
            token = os.environ[unbound]
            token_name = unbound
            unbound = None
        return Endpoint(
            module=module,
            address=address,
            source=address_name,
            token=token,
            token_source=(
                token_name
                if token
                else f"none — {unbound} is set but is not paired with {address_name}"
                if unbound
                else None
            ),
        )

    # No address override: fall back to the measured default, and accept a token
    # from any family, because none of them is contradicting a chosen address.
    for _, token_name in families:
        if token_name and os.environ.get(token_name):
            return Endpoint(
                module=module,
                address=default,
                source=f"default (measured on {MEASURED_HOST} {MEASURED_ON})",
                token=os.environ[token_name],
                token_source=token_name,
            )
    return Endpoint(
        module=module,
        address=default,
        source=f"default (measured on {MEASURED_HOST} {MEASURED_ON})",
    )


def _token_set_in_another_family(
    families: tuple[tuple[str, str | None], ...], chosen_address_name: str
) -> str | None:
    """Name the token variable that was set but deliberately not used."""

    for address_name, token_name in families:
        if address_name != chosen_address_name and token_name and os.environ.get(token_name):
            return token_name
    return None
