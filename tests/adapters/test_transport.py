"""transport.py's two guarantees, tested from the outside."""

from __future__ import annotations

import pytest

from mosaic.adapters.transport import ReadOnlyCommand, ReadOnlyHTTP, Unreachable


def test_command_refuses_a_verb_it_was_not_given() -> None:
    """`janus decide` rules on a gate. No argv this class accepts can reach it.

    Note what is NOT passed in: the allowlist. It lives in `transport.READ_VERBS`
    keyed by module, so a caller cannot widen it -- the fix for the circularity
    claude-2f found in review.
    """

    cli = ReadOnlyCommand(module="janus", executable="janus")
    with pytest.raises(PermissionError) as caught:
        cli.run_json(["decide", "g123", "--option", "yes"])
    assert "decide" in str(caught.value)
    assert "list" in str(caught.value), "the error should say what IS allowed"


def test_unreachable_always_carries_the_address_tried() -> None:
    """ "Connection refused" without an address is the least useful error there is.

    Four modules, two bind addresses, and a default that is right for half of
    them -- the address is the first thing anyone debugging this needs.
    """

    http = ReadOnlyHTTP(module="athena", base_url="http://127.0.0.1:9", timeout=0.25)
    with pytest.raises(Unreachable) as caught:
        http.get_json("/version")
    assert "127.0.0.1:9" in str(caught.value)
    assert caught.value.module == "athena"
    assert caught.value.address.endswith("/version")


def test_missing_executable_is_unreachable_not_a_crash() -> None:
    cli = ReadOnlyCommand(module="janus", executable="mosaic-no-such-binary")
    with pytest.raises(Unreachable) as caught:
        cli.run_json(["list"])
    assert "not on PATH" in str(caught.value)
