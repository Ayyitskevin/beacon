"""`mosaic doctor` -- what is reachable, and precisely which joins do not exist.

This is not a health check. The roadmap is explicit that the first slice's job is
to find out whether the modules can produce one coherent cross-module document at
all, and that **knowing exactly which joins are missing is worth more than a
shell**. So a missing join is a first-class result here, reported with a reason,
not an empty section.

Three rules this module holds to:

1. **Every address is printed, including the ones that worked.** The fleet's
   modules disagree about whether they live on loopback or the tailnet, and the
   most likely way Mosaic ships broken is a plausible default that silently
   misses two of them. A wrong address should be visible, not inferred from a
   refusal.
2. **Defects are measured, not remembered.** The known-broken Minerva discovery
   path below is re-run every time rather than asserted from a note, because a
   note is a claim about the past and this is a diagnostic about the present.
3. **Absence is reported with a reason.** A module with no adapter gets a row
   saying why. Silence would read as a clean fleet.
"""

from __future__ import annotations

import subprocess
from typing import Any

from . import config
from .adapters import REGISTRY
from .adapters.transport import DEFAULT_TIMEOUT

#: The product trace from the roadmap. Mosaic exists to answer one question that
#: walks this whole chain -- "why does this claim hold, and who let it ship?" --
#: so the hops are enumerated here and each one is reported as joinable or not.
PRODUCT_TRACE: tuple[tuple[str, str], ...] = (
    ("buzz", "athena"),
    ("athena", "minerva"),
    ("minerva", "vulcan"),
    ("vulcan", "replaydeck"),
    ("replaydeck", "janus"),
    ("janus", "icarus"),
)


def _probe_all() -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for registration in REGISTRY:
        row: dict[str, Any] = {
            "module": registration.module,
            "owns": registration.owns,
            "access": registration.access,
            "adapter": registration.factory is not None,
        }
        if registration.factory is None:
            # Unknown, not false. Nobody looked, because there is nothing to look
            # at -- reporting a repo that was never built as a service that is
            # down sends someone to restart it.
            row["reachable"] = None
            row["detail"] = registration.absent_reason
            rows.append(row)
            continue

        try:
            adapter = registration.factory()
            capability = adapter.capabilities()
            entries, unreachable = adapter.read()
        except Exception as exc:  # an adapter must never take doctor down with it
            row["reachable"] = False
            row["detail"] = f"adapter raised {type(exc).__name__}: {exc}"
            rows.append(row)
            continue

        raw = capability.raw if isinstance(capability.raw, dict) else {}
        row.update(
            {
                "reachable": capability.listener is not None and not unreachable,
                "address": capability.listener
                or _describe_endpoint(registration.module).get("address"),
                "address_source": raw.get("address_source")
                or _describe_endpoint(registration.module).get("source"),
                "version": capability.version,
                "surfaces": list(capability.surfaces),
                "entries": len(entries),
                "commit": raw.get("commit"),
                "tree_state": raw.get("tree_state"),
                "detail": (
                    "; ".join(f"{u.reason}" for u in unreachable)
                    if unreachable
                    else _read_detail(len(entries), capability)
                ),
            }
        )
        rows.append(row)
    return rows


def _read_detail(count: int, capability: Any) -> str:
    """Say which of the three read outcomes this was.

    Reachable-with-nothing and unreachable are different facts, and a desk that
    renders them alike is lying by omission. Doctor says which one happened.
    """

    if count:
        return f"{count} entries"
    return "reachable, and contributes nothing to the desk"


def _check_minerva_cli_discovery() -> dict[str, Any]:
    """Re-measure the broken Minerva CLI discovery path.

    Minerva's own capability manifest advertises `capabilities_argv: ["capabilities"]`
    as its CLI discovery entry point. On node-a the fleet wrapper ~/.local/bin/minerva
    injects `--db` into every verb outside a small allowlist, and `capabilities`
    accepts no `--db` -- so the module correctly declares how to discover it and the
    canonical entrypoint makes that declaration false.

    Capability discovery is the mechanism Mosaic wants to build cross-module
    discovery on, so a broken discovery path is a Mosaic problem, not a Minerva
    footnote. Measured live so that the day someone fixes the wrapper, this stops
    reporting on its own.
    """

    try:
        done = subprocess.run(
            ["minerva", "capabilities"],
            capture_output=True,
            text=True,
            timeout=DEFAULT_TIMEOUT,
            check=False,
            shell=False,
        )
    except FileNotFoundError:
        return {"ok": None, "detail": "`minerva` is not on PATH -- not checked"}
    except subprocess.TimeoutExpired:
        return {"ok": False, "detail": "`minerva capabilities` timed out"}

    if done.returncode == 0:
        return {"ok": True, "detail": "`minerva capabilities` answers on PATH"}
    return {
        "ok": False,
        "detail": (
            "`minerva capabilities` fails on PATH while the venv binary succeeds -- "
            "the fleet wrapper injects --db into a verb that takes none. Minerva's "
            "own manifest advertises this argv as its discovery path. Fleet infra, "
            "not a Mosaic bug; reported because Mosaic relies on the surface."
        ),
        "exit_code": done.returncode,
    }


def _trace_joins(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    reachable = {row["module"]: bool(row.get("reachable")) and row.get("adapter") for row in rows}
    joins: list[dict[str, Any]] = []
    for left, right in PRODUCT_TRACE:
        both = reachable.get(left, False) and reachable.get(right, False)
        missing = [m for m in (left, right) if not reachable.get(m, False)]
        joins.append(
            {
                "hop": f"{left} -> {right}",
                "both_ends_readable": both,
                # Naming the end that fails is the whole point. "Join unavailable"
                # sends someone to check six modules; "replaydeck has no repo"
                # sends them to the one that matters.
                "blocked_by": missing,
            }
        )
    return joins


def run_doctor() -> dict[str, Any]:
    rows = _probe_all()
    joins = _trace_joins(rows)
    with_adapters = [r for r in rows if r["adapter"]]
    return {
        "schema": "mosaic.doctor.v1",
        "summary": {
            "modules_in_trace": len(REGISTRY),
            "adapters_built": len(with_adapters),
            "reachable_now": sum(1 for r in with_adapters if r.get("reachable")),
            "trace_hops_joinable": sum(1 for j in joins if j["both_ends_readable"]),
            "trace_hops_total": len(joins),
        },
        "modules": rows,
        "trace": joins,
        "discovery": {
            "note": (
                "Two modules self-describe and their documents disagree: Minerva "
                "serves minerva.capabilities.v3 with a schema_version field, Vulcan "
                "serves /v1/capabilities without one. Mosaic normalises; it does not "
                "declare a standard that its only two adopters already differ on."
            ),
            "minerva_cli": _check_minerva_cli_discovery(),
        },
        "config": {
            registration.module: _describe_endpoint(registration.module)
            for registration in REGISTRY
        },
    }


def _describe_endpoint(module: str) -> dict[str, Any]:
    try:
        endpoint = config.resolve(module)
    except KeyError:
        return {"address": None, "source": "not an HTTP module"}
    return {
        "address": endpoint.address,
        "source": endpoint.source,
        # Never the token itself. Which env var supplied it is the debugging
        # information; the value is a secret and secrets do not go in reports.
        "token": bool(endpoint.token),
        "token_source": endpoint.token_source,
    }


def render_text(report: dict[str, Any]) -> str:
    """Human-readable doctor output.

    Kept here rather than in render/ because it describes adapter state, which is
    this lane's subject; the desk renderers stay Lane C's.
    """

    summary = report["summary"]
    lines = [
        "mosaic doctor",
        "",
        f"  {summary['reachable_now']}/{summary['adapters_built']} adapters reachable"
        f"  ·  {summary['adapters_built']}/{summary['modules_in_trace']} modules have an adapter"
        f"  ·  {summary['trace_hops_joinable']}/{summary['trace_hops_total']} trace hops joinable",
        "",
        "MODULES",
    ]
    for row in report["modules"]:
        if row["reachable"] is None:
            mark = "  -"
        elif row["reachable"]:
            mark = "  +"
        else:
            mark = "  !"
        lines.append(f"{mark} {row['module']:<11} {row.get('address', row['access'])}")
        if row.get("address_source"):
            lines.append(f"      address from: {row['address_source']}")
        for field in ("version", "commit", "tree_state", "schema_version"):
            if row.get(field):
                lines.append(f"      {field}: {row[field]}")
        if row.get("detail"):
            lines.append(f"      {row['detail']}")
    lines += ["", "PRODUCT TRACE"]
    for join in report["trace"]:
        mark = "  +" if join["both_ends_readable"] else "  !"
        blocked = ""
        if join["blocked_by"]:
            blocked = f"   blocked by: {', '.join(join['blocked_by'])}"
        lines.append(f"{mark} {join['hop']:<24}{blocked}")
    discovery = report["discovery"]["minerva_cli"]
    lines += ["", "DISCOVERY", f"  {report['discovery']['note']}", f"  {discovery['detail']}"]
    return "\n".join(lines)
