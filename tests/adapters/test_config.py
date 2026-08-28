"""Endpoint resolution, including the trap this fleet already fell into once."""

from __future__ import annotations

import pytest

from beacon_legacy import config


def test_defaults_are_not_uniformly_localhost() -> None:
    """The regression this whole module exists to prevent.

    Measured on mickey: Athena and Buzz answer only on the tailnet address and
    refuse loopback; Minerva and Vulcan are loopback-only. A config that defaults
    everything to one or the other silently loses half the fleet, and the failure
    looks exactly like a dead service.
    """

    athena = config.resolve("athena").address
    minerva = config.resolve("minerva").address
    assert "127.0.0.1" not in athena, "Athena does not listen on loopback"
    assert "127.0.0.1" in minerva, "Minerva listens only on loopback"


def test_default_source_says_it_is_a_default(monkeypatch: pytest.MonkeyPatch) -> None:
    for name in (
        "BEACON_LEGACY_ATHENA_URL",
        "MOSAIC_ATHENA_URL",
        "ATHENA_BASE_URL",
        "ATHENA_URL",
    ):
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

    for name in (
        "BEACON_LEGACY_ATHENA_URL",
        "MOSAIC_ATHENA_URL",
        "ATHENA_BASE_URL",
        "ATHENA_URL",
    ):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv(spelling, "http://example.invalid:1234")
    endpoint = config.resolve("athena")
    assert endpoint.address == "http://example.invalid:1234"
    assert endpoint.source == spelling


def test_beacon_legacy_prefixed_name_wins(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://deprecated.invalid")
    monkeypatch.setenv("ATHENA_URL", "http://loser.invalid")
    monkeypatch.setenv("BEACON_LEGACY_ATHENA_URL", "http://winner.invalid")
    assert config.resolve("athena").address == "http://winner.invalid"


def test_pre_rename_name_remains_an_explicit_compatibility_alias(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    for name in ("BEACON_LEGACY_ATHENA_URL", "ATHENA_BASE_URL", "ATHENA_URL"):
        monkeypatch.delenv(name, raising=False)
    monkeypatch.setenv("MOSAIC_ATHENA_URL", "http://legacy.invalid")

    endpoint = config.resolve("athena")

    assert endpoint.address == "http://legacy.invalid"
    assert endpoint.source == "MOSAIC_ATHENA_URL"


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
        "BEACON_LEGACY_ATHENA_URL",
        "ATHENA_BASE_URL",
        "ATHENA_URL",
        "BEACON_LEGACY_ATHENA_TOKEN",
        "MOSAIC_ATHENA_URL",
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
        monkeypatch.setenv("BEACON_LEGACY_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.address == "http://elsewhere.invalid:9999"
        assert endpoint.token is None, "a live bearer followed the address to another host"

    def test_it_says_why_the_token_was_withheld(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """Silence here produces a 401 nobody can explain."""

        self._clear(monkeypatch)
        monkeypatch.setenv("BEACON_LEGACY_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        source = config.resolve("athena").token_source or ""

        assert "ATHENA_API_TOKEN" in source
        assert "BEACON_LEGACY_ATHENA_URL" in source

    def test_a_paired_address_and_token_are_still_accepted(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The real token file's own pairing must keep working.

        `~/ai-workspace/claude/.secrets/athena-token` exports ATHENA_URL and
        ATHENA_API_TOKEN together. Breaking that would have traded a footgun for
        an outage.
        """

        self._clear(monkeypatch)
        monkeypatch.setenv("ATHENA_URL", "http://athena.invalid:8300")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.address == "http://athena.invalid:8300"
        assert endpoint.token == "live-token-value"
        assert endpoint.token_source == "ATHENA_API_TOKEN"

    def test_a_token_alone_still_reaches_the_default_address(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """No address override means no chosen endpoint to contradict."""

        self._clear(monkeypatch)
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.token == "live-token-value"
        assert endpoint.source.startswith("default")

    def test_the_endpoint_never_carries_the_token_value_in_its_source(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        self._clear(monkeypatch)
        monkeypatch.setenv("BEACON_LEGACY_ATHENA_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert "live-token-value" not in (endpoint.token_source or "")
        assert "live-token-value" not in endpoint.source

    def test_the_correct_host_named_by_another_family_keeps_its_token(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The most likely real mixture must not 401.

        `ATHENA_BASE_URL` is what most fleet tooling expects; `ATHENA_API_TOKEN` is
        what Athena's own token file exports. Someone sourcing the token file and
        setting the fleet-standard URL var names the *correct* host through a
        different family. Nothing was redirected, so nothing needs protecting.
        """

        self._clear(monkeypatch)
        default, _ = config._DEFAULTS["athena"]
        monkeypatch.setenv("ATHENA_BASE_URL", default)
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        endpoint = config.resolve("athena")

        assert endpoint.address == default
        assert endpoint.token == "live-token-value"

    def test_that_relaxation_cannot_rescue_a_redirect(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The guard above keys on equality with the default, and only that.

        An address that is not the default is a redirect regardless of which
        family named it, so the credential is still withheld.
        """

        self._clear(monkeypatch)
        monkeypatch.setenv("ATHENA_BASE_URL", "http://elsewhere.invalid:9999")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        assert config.resolve("athena").token is None

    def test_a_host_that_merely_starts_with_the_default_is_still_a_redirect(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """Prefix confusion. This test is the guard; the comment above it is not.

        `http://100.125.80.91:8300.evil.invalid` starts with the default address
        and is a completely different host. The relaxation that lets a correct
        host keep its token compares for EXACT equality, and that exactness is
        the only thing standing between this case and a live bearer.

        Written after claude-2f demonstrated that swapping `==` for `.startswith()`
        left the entire suite green while sending the token here -- so the
        property had a comment behind it and no test. The realistic path to that
        mutation is benign: someone hits the trailing-slash wrinkle, reaches for a
        looser comparison to be helpful, sees green, and ships it.
        """

        self._clear(monkeypatch)
        default, _ = config._DEFAULTS["athena"]
        monkeypatch.setenv("ATHENA_BASE_URL", default + ".evil.invalid")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        assert config.resolve("athena").token is None, (
            "a host that merely shares a prefix with the default received the token"
        )

    def test_a_default_prefixed_path_is_also_a_redirect(
        self, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The same trap one character further along: an appended path, not a host."""

        self._clear(monkeypatch)
        default, _ = config._DEFAULTS["athena"]
        monkeypatch.setenv("ATHENA_BASE_URL", default + "@evil.invalid")
        monkeypatch.setenv("ATHENA_API_TOKEN", "live-token-value")

        assert config.resolve("athena").token is None
