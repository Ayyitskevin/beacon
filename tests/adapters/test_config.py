"""Endpoint resolution, including the trap this fleet already fell into once."""

from __future__ import annotations

import pytest

from mosaic import config


def test_defaults_are_not_uniformly_localhost() -> None:
    """The regression this whole module exists to prevent.

    Measured on node-a: Athena and Buzz answer only on the tailnet address and
    refuse loopback; Minerva and Vulcan are loopback-only. A config that defaults
    everything to one or the other silently loses half the fleet, and the failure
    looks exactly like a dead service.
    """

    athena = config.resolve("athena").address
    minerva = config.resolve("minerva").address
    assert "127.0.0.1" not in athena, "Athena does not listen on loopback"
    assert "127.0.0.1" in minerva, "Minerva listens only on loopback"


def test_default_source_says_it_is_a_default(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in ("MOSAIC_ATHENA_URL", "ATHENA_BASE_URL", "ATHENA_URL"):
        monkeypatch.delenv(name, raising=False)
    endpoint = config.resolve("athena")
    assert endpoint.source.startswith("default"), (
        "a defaulted address must announce itself as a default, or an operator "
        "reading doctor output cannot tell a configured address from a guess"
    )


@pytest.mark.parametrize("spelling", ["ATHENA_BASE_URL", "ATHENA_URL"])
def test_both_token_file_and_tooling_spellings_are_accepted(
    monkeypatch: pytest.MonkeyPatch, spelling: str
) -> None:
    """Athena's own token file and the fleet's tooling disagree on the names.

    The file exports ATHENA_URL / ATHENA_API_TOKEN; most tooling expects
    ATHENA_BASE_URL / ATHENA_TOKEN. That mismatch already cost another seat enough
    time to earn an ORACLE troubleshooting page. Accepting both is the fix;
    reporting which one was used is what stops it recurring silently.
    """

    for name in ("MOSAIC_ATHENA_URL", "ATHENA_BASE_URL", "ATHENA_URL"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv(spelling, "http://example.invalid:1234")
    endpoint = config.resolve("athena")
    assert endpoint.address == "http://example.invalid:1234"
    assert endpoint.source == spelling


def test_mosaic_prefixed_name_wins(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("ATHENA_URL", "http://loser.invalid")
    monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://winner.invalid")
    assert config.resolve("athena").address == "http://winner.invalid"


class TestCredentialsAreBoundToTheirEndpoint:
    """A credential is bound to the endpoint it was issued for.

    Changing the endpoint must invalidate the credential rather than send it
    somewhere new. Found in review by claude-2f: address and token were resolved
    from two independent env lists, so overriding only the address redirected a
    live Athena bearer to whatever host that variable named.

    A footgun rather than a vulnerability -- anyone who can set the environment
    already owns the machine. The realistic failure is a typo or a copied config,
    and a credential leaving the fleet silently.
    """

    NAMES = (
        "MOSAIC_ATHENA_URL",
        "ATHENA_BASE_URL",
        "ATHENA_URL",
        "MOSAIC_ATHENA_TOKEN",
        "ATHENA_TOKEN",
        "ATHENA_API_TOKEN",
    )

    def _clear(self, monkeypatch: pytest.MonkeyPatch) -> None:
        for name in self.NAMES:
            monkeypatch.delenv(name, raising=False)

    def test_a_token_does_not_follow_an_address_from_another_family(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        self._clear(monkeypatch)
        monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.address == "http://elsewhere.invalid:9999"
        assert endpoint.token is None, "a live bearer followed the address to another host"

    def test_it_says_why_the_token_was_withheld(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Silence here produces a 401 nobody can explain."""

        self._clear(monkeypatch)
        monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        source = config.resolve("athena").token_source or ""

        assert "ATHENA_API_TOKEN" in source
        assert "MOSAIC_ATHENA_URL" in source

    def test_a_paired_address_and_token_are_still_accepted(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The real token file's own pairing must keep working.

        `/example/checkouts/.secrets/athena-token` exports ATHENA_URL and
        ATHENA_API_TOKEN together. Breaking that would have traded a footgun for
        an outage.
        """

        self._clear(monkeypatch)
        monkeypatch.setenv("ATHENA_URL", "http://athena.invalid:18300")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.address == "http://athena.invalid:18300"
        assert endpoint.token == "live-token-value"
        assert endpoint.token_source == "ATHENA_API_TOKEN"

    def test_a_token_alone_never_reaches_the_example_address(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """No address means no explicit credential binding."""

        self._clear(monkeypatch)
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.token is None
        assert endpoint.source.startswith("default")

    def test_the_endpoint_never_carries_the_token_value_in_its_source(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        self._clear(monkeypatch)
        monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert "live-token-value" not in (endpoint.token_source or "")
        assert "live-token-value" not in endpoint.source


@pytest.mark.parametrize("suffix", ["", "/", "/api", "?query=1", "#fragment"])
def test_example_destination_never_receives_an_explicitly_paired_token(
    monkeypatch: pytest.MonkeyPatch, suffix: str
) -> None:
    for name in (
        "BEACON_LEGACY_ATHENA_URL",
        "BEACON_LEGACY_ATHENA_TOKEN",
        "MOSAIC_ATHENA_URL",
        "MOSAIC_ATHENA_TOKEN",
        "ATHENA_BASE_URL",
        "ATHENA_TOKEN",
        "ATHENA_URL",
        "ATHENA_API_TOKEN",
    ):
        monkeypatch.delenv(name, raising=False)
    default = config.resolve("athena").address
    monkeypatch.setenv("ATHENA_URL", default + suffix)
    monkeypatch.setenv("ATHENA_API_TOKEN", "synthetic-credential-not-secret")
    endpoint = config.resolve("athena")
    assert endpoint.token is None
    assert "illustrative" in (endpoint.token_source or "")
    assert "synthetic-credential-not-secret" not in (endpoint.token_source or "")


@pytest.mark.parametrize(
    "token_name",
    [
        "BEACON_LEGACY_ATHENA_TOKEN",
        "MOSAIC_ATHENA_TOKEN",
        "ATHENA_TOKEN",
        "ATHENA_API_TOKEN",
    ],
)
def test_every_token_only_family_is_withheld(
    monkeypatch: pytest.MonkeyPatch, token_name: str
) -> None:
    for name in (
        "BEACON_LEGACY_ATHENA_URL",
        "BEACON_LEGACY_ATHENA_TOKEN",
        "MOSAIC_ATHENA_URL",
        "MOSAIC_ATHENA_TOKEN",
        "ATHENA_BASE_URL",
        "ATHENA_TOKEN",
        "ATHENA_URL",
        "ATHENA_API_TOKEN",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv(token_name, "synthetic-credential-not-secret")
    endpoint = config.resolve("athena")
    assert endpoint.token is None
    assert endpoint.source.startswith("default")
