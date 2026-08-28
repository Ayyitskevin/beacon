"""The scrubber must catch real secrets, not merely secret-shaped strings.

No test in this file prints, asserts on, or persists a secret value. Assertions
are on booleans and on the redaction marker.
"""

from __future__ import annotations

import json

import pytest

from beacon_legacy.store.fixtures.scrub import (
    REDACTED,
    contains_known_secret,
    known_secret_values,
    scrub,
)

FAKE = {"FAKE_TOKEN_VALUE_abcdef123456", "another-fake-secret-value-xyz"}


class TestScrubsByKeyName:
    """A field called `token` is a secret whatever it contains."""

    @pytest.mark.parametrize(
        "key", ["token", "api_key", "Authorization", "PASSWORD", "session_id", "bearer_token"]
    )
    def test_a_secret_named_field_is_redacted(self, key):
        out = scrub({key: "anything at all"}, known=set())
        assert out[key] == REDACTED

    def test_the_key_survives_so_the_fixture_still_describes_the_shape(self):
        """Dropping the field would make the fixture describe a response the
        source never sends."""
        out = scrub({"token": "x", "id": 1}, known=set())
        assert set(out) == {"token", "id"}

    def test_a_null_stays_null_rather_than_becoming_a_fake_secret(self):
        assert scrub({"token": None}, known=set())["token"] is None


class TestScrubsByValueShape:
    @pytest.mark.parametrize(
        "value",
        [
            "Bearer abc123def456ghi",
            "sk-abcdefghijklmnopqrstuvwxyz",
            "ghp_abcdefghijklmnopqrstuvwxyz12",
            "xai-abcdefghijklmnopqrst",
            "sk-ant-abcdefghijklmnopqrst",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w",
            "postgres://user:hunter2@host/db",
        ],
    )
    def test_a_secret_shape_is_redacted_even_in_an_innocent_field(self, value):
        out = scrub({"description": value}, known=set())
        assert REDACTED in out["description"]

    def test_a_private_key_block_is_redacted(self):
        pem = "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----"
        assert scrub({"note": pem}, known=set())["note"] == REDACTED


class TestScrubsByKnownValue:
    """The case pattern-matching misses: a real secret in an innocent field."""

    def test_a_known_value_is_redacted_wherever_it_sits(self):
        payload = {"body": f"the key is {sorted(FAKE)[0]} ok", "nested": [{"x": sorted(FAKE)[1]}]}
        out = scrub(payload, known=FAKE)
        assert not contains_known_secret(out, known=FAKE)
        assert REDACTED in out["body"]

    def test_real_live_secrets_are_caught(self):
        """Uses this machine's actual secret values. Nothing is printed."""
        live = known_secret_values()
        if not live:
            pytest.skip("no secret sources readable on this host")
        payload = {"innocent_field": f"prefix {sorted(live)[0]} suffix"}
        assert contains_known_secret(payload, known=live) is True
        assert contains_known_secret(scrub(payload, known=live), known=live) is False


class TestDoesNotOverScrub:
    """Over-scrubbing has a real cost; it is not free safety."""

    def test_64_hex_survives_because_buzz_ids_and_digests_are_that_shape(self):
        digest = "a" * 64
        assert scrub({"binding_sha256": digest}, known=set())["binding_sha256"] == digest

    def test_ordinary_content_is_untouched(self):
        payload = {"title": "Wire the desk adapters", "status": "superseded", "count": 3}
        assert scrub(payload, known=set()) == payload

    def test_structure_and_types_are_preserved(self):
        payload = {"a": [1, 2, {"b": None, "c": True}], "d": 1.5}
        assert scrub(payload, known=set()) == payload

    def test_input_is_never_mutated(self):
        payload = {"token": "x", "list": [{"api_key": "y"}]}
        before = json.dumps(payload, sort_keys=True)
        scrub(payload, known=set())
        assert json.dumps(payload, sort_keys=True) == before


class TestTheVerifierIsHonest:
    def test_it_reports_a_survivor(self):
        assert contains_known_secret({"x": sorted(FAKE)[0]}, known=FAKE) is True

    def test_it_reports_clean_when_clean(self):
        assert contains_known_secret({"x": "nothing here"}, known=FAKE) is False

    def test_it_never_returns_the_secret_itself(self):
        assert contains_known_secret({"x": sorted(FAKE)[0]}, known=FAKE) is True
        assert isinstance(contains_known_secret({"x": "y"}, known=FAKE), bool)

    def test_an_empty_known_set_reports_clean_rather_than_erroring(self):
        """But see test_capture: an empty known set must BLOCK a capture, not
        pass it. 'Nothing to check against' is not 'nothing found'."""
        assert contains_known_secret({"x": "y"}, known=set()) is False


class TestOverScrubbingIsARealBugNotFreeSafety:
    """The first version matched key-name SUBSTRINGS and destroyed a fixture.

    `prompt_tokens` and `completion_tokens` contain "token", so Vulcan's usage
    corpus came back with 155 redactions where the token counts should be — a
    usage reader cannot be built from that. The failure is silent: the fixture
    looks scrubbed and careful and is useless. Nothing downstream complains
    about over-scrubbing the way a leak would be complained about, which is
    exactly why it needs its own tests.
    """

    @pytest.mark.parametrize(
        "key",
        [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "max_tokens",
            "sort_key",
            "api_version",
            "model",
            "seat",
            "ts",
            "provider",
            "title",
        ],
    )
    def test_a_count_or_ordinary_field_is_not_redacted(self, key):
        assert scrub({key: 1234}, known=set())[key] == 1234

    @pytest.mark.parametrize(
        "key",
        [
            "token",
            "api_token",
            "access_token",
            "ATHENA_API_TOKEN",
            "api_key",
            "password",
            "Authorization",
            "bearer_token",
            "session_id",
            "private_key",
            "cookie",
            "client_secret",
        ],
    )
    def test_a_real_secret_field_is_still_redacted(self, key):
        assert scrub({key: "value"}, known=set())[key] == REDACTED

    def test_the_vulcan_ledger_shape_survives_intact(self):
        """The exact record that was destroyed."""
        event = {
            "seat": "claude-code",
            "model": "simple",
            "provider": "local-ollama",
            "ts": 1786852163,
            "prompt_tokens": 412,
            "completion_tokens": 88,
        }
        assert scrub(event, known=set()) == event

    def test_singular_and_plural_are_different_words(self):
        """`token` is a credential; `tokens` is a count. Substring matching
        cannot tell them apart, which is the whole bug."""
        out = scrub({"token": "abc", "tokens": 500}, known=set())
        assert out["token"] == REDACTED
        assert out["tokens"] == 500
