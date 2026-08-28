"""Doctor's job is to make a missing join legible, not to hide one."""

from __future__ import annotations

from typing import Any

from beacon_legacy import doctor
from beacon_legacy.adapters import REGISTRY


def test_all_seven_capabilities_are_listed() -> None:
    """AI Town is seven modules, and every handoff about this work says six.

    ReplayDeck is named in the roadmap's product trace at the evaluation step and
    is ranked first in the portfolio. It has never been built. A registry that
    listed only what we adapted could not report that.
    """

    modules = {r.module for r in REGISTRY}
    assert modules == {
        "buzz",
        "athena",
        "minerva",
        "vulcan",
        "replaydeck",
        "janus",
        "icarus",
    }


def test_a_module_without_an_adapter_reports_a_reason() -> None:
    """Absence with no explanation reads as a clean fleet."""

    for registration in REGISTRY:
        if registration.factory is None:
            assert registration.absent_reason.strip(), (
                f"{registration.module} has no adapter and no stated reason, which "
                f"is indistinguishable from an oversight"
            )


def test_unbuilt_modules_are_unknown_not_unreachable() -> None:
    """`reachable: None` and `reachable: False` are different claims.

    False means we asked and got nothing. None means nobody asked. Collapsing
    them would report ReplayDeck -- which does not exist -- as a service that is
    down, and send someone to restart it.
    """

    rows = doctor._probe_all()
    replaydeck = next(r for r in rows if r["module"] == "replaydeck")
    assert replaydeck["reachable"] is None
    assert replaydeck["adapter"] is False


def test_a_blocked_hop_names_the_end_that_blocks_it() -> None:
    """ "Join unavailable" sends someone to check seven modules."""

    rows: list[dict[str, Any]] = [
        {"module": "buzz", "reachable": None, "adapter": False},
        {"module": "athena", "reachable": True, "adapter": True},
        {"module": "minerva", "reachable": True, "adapter": True},
        {"module": "vulcan", "reachable": True, "adapter": True},
        {"module": "replaydeck", "reachable": None, "adapter": False},
        {"module": "janus", "reachable": True, "adapter": True},
        {"module": "icarus", "reachable": None, "adapter": False},
    ]
    joins = {j["hop"]: j for j in doctor._trace_joins(rows)}
    assert joins["athena -> minerva"]["both_ends_readable"] is True
    assert joins["athena -> minerva"]["blocked_by"] == []
    assert joins["vulcan -> replaydeck"]["blocked_by"] == ["replaydeck"]
    assert joins["janus -> icarus"]["blocked_by"] == ["icarus"]


def test_report_never_contains_a_token() -> None:
    """Which env var supplied the token is debugging information. The value is not.

    Doctor output gets pasted into handoffs and Buzz threads. A report that can
    leak a bearer token is a report that will.
    """

    report = doctor.run_doctor()
    for module, described in report["config"].items():
        assert set(described) <= {"address", "source", "token", "token_source"}, module
        assert isinstance(described.get("token", False), bool), (
            f"{module}: the token field must be a boolean presence flag, never the value"
        )


def test_text_render_shows_the_address_even_when_it_worked() -> None:
    """A wrong default should be visible, not inferred from a refusal."""

    report = doctor.run_doctor()
    text = doctor.render_text(report)
    for module in ("athena", "minerva", "vulcan"):
        assert module in text
    assert "address from:" in text
    assert "PRODUCT TRACE" in text
