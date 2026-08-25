"""The never-collapse rule, as tests.

If these pass, Mosaic cannot present five different approval mechanisms as one
generic affordance. If any of them is ever deleted to make a change go
through, the change is the problem.
"""

from __future__ import annotations

import pytest

from mosaic.render import json_render, text
from mosaic.render.authority import (
    AUTHORITY_LABELS,
    FORBIDDEN_RENDERINGS,
    resolve,
)


def entry(**overrides):
    base = {
        "source": "athena",
        "owner": "codex",
        "status": "open",
        "authority": "proposed",
        "timestamp": "2026-08-24T00:00:00Z",
        "schema_version": "issue.v1",
        "link": "http://example.invalid/1",
    }
    base.update(overrides)
    return base


class TestTheFiveLabels:
    def test_all_five_are_recognised(self):
        for label in AUTHORITY_LABELS:
            verdict = resolve(entry(authority=label, source="janus"))
            assert verdict.label == label
            assert verdict.badge != "?" or label == "janus-authorized"

    def test_the_five_never_render_identically(self):
        """The whole rule in one assertion: five labels, five distinct displays."""
        displays = {
            resolve(entry(authority=label, source="janus", binding_holds=True)).display
            for label in AUTHORITY_LABELS
        }
        assert len(displays) == len(AUTHORITY_LABELS)

    @pytest.mark.parametrize("label", AUTHORITY_LABELS)
    def test_no_label_renders_a_forbidden_word(self, label):
        verdict = resolve(entry(authority=label, source="janus", binding_holds=True))
        lowered = verdict.display.lower()
        for word in FORBIDDEN_RENDERINGS:
            assert word not in lowered, f"{label!r} rendered as {verdict.display!r}"


class TestOnlyJanusIsAuthority:
    def test_janus_with_holding_binding_is_the_only_authority(self):
        verdict = resolve(entry(source="janus", authority="janus-authorized", binding_holds=True))
        assert verdict.is_authority is True
        assert verdict.rechecked is True

    @pytest.mark.parametrize("label", [x for x in AUTHORITY_LABELS if x != "janus-authorized"])
    def test_no_other_label_is_authority(self, label):
        assert resolve(entry(source="janus", authority=label)).is_authority is False

    @pytest.mark.parametrize("source", ["athena", "buzz", "icarus", "minerva", "vulcan", ""])
    def test_another_module_cannot_claim_janus_authority(self, source):
        """A module asserting a Janus ruling renders as a claim, not a ruling."""
        verdict = resolve(entry(source=source, authority="janus-authorized", binding_holds=True))
        assert verdict.is_authority is False
        assert "claims" in verdict.display
        assert verdict.note is not None


class TestBindingIsRecheckable:
    def test_broken_binding_is_loud_and_not_authority(self):
        verdict = resolve(entry(source="janus", authority="janus-authorized", binding_holds=False))
        assert verdict.is_authority is False
        assert verdict.rechecked is True
        assert "STALE" in verdict.display

    def test_unchecked_binding_is_not_authority(self):
        verdict = resolve(entry(source="janus", authority="janus-authorized"))
        assert verdict.is_authority is False
        assert verdict.rechecked is None

    def test_unchecked_is_distinguishable_from_broken(self):
        unchecked = resolve(entry(source="janus", authority="janus-authorized"))
        broken = resolve(entry(source="janus", authority="janus-authorized", binding_holds=False))
        assert unchecked.display != broken.display
        assert unchecked.rechecked != broken.rechecked

    def test_truthy_is_not_true(self):
        """`binding_holds: "yes"` must not sneak past as authority."""
        for truthy in ["yes", 1, "true", [1]]:
            verdict = resolve(
                entry(source="janus", authority="janus-authorized", binding_holds=truthy)
            )
            assert verdict.is_authority is False


class TestUnknownLabels:
    @pytest.mark.parametrize("label", ["approved", "signed-off", "", None, "JANUS_AUTHORIZED"])
    def test_unknown_label_is_reported_not_guessed(self, label):
        verdict = resolve(entry(source="janus", authority=label))
        assert verdict.is_authority is False
        assert verdict.badge == "?"
        assert "unrecognised" in verdict.display

    def test_missing_authority_key_does_not_raise(self):
        bare = {"source": "athena", "owner": "codex"}
        verdict = resolve(bare)
        assert verdict.is_authority is False
        assert "(missing)" in verdict.display


class TestRenderersHoldTheLine:
    """The rule must survive the trip through both renderers, not just the resolver."""

    def _doc(self, **overrides):
        return {"schema": "desk.v1", "entries": [entry(**overrides)]}

    @pytest.mark.parametrize("label", AUTHORITY_LABELS)
    def test_text_render_never_emits_a_forbidden_word_as_status(self, label):
        out = text.render(self._doc(authority=label, source="janus", binding_holds=True))
        # The module's own status field is rendered verbatim and attributed, so
        # only the authority lines are checked for the collapse vocabulary.
        authority_lines = [
            ln for ln in out.splitlines() if "status:" not in ln and not ln.startswith("─")
        ]
        for word in FORBIDDEN_RENDERINGS:
            assert word not in "\n".join(authority_lines).lower()

    def test_text_marks_authority_and_only_authority(self):
        authorised = text.render(
            self._doc(source="janus", authority="janus-authorized", binding_holds=True)
        )
        assert "AUTHORITY" in authorised
        assert "1 entry renders as authority" in authorised

        for label in [x for x in AUTHORITY_LABELS if x != "janus-authorized"]:
            out = text.render(self._doc(source="janus", authority=label))
            assert "0 entries render as authority" in out

    def test_json_annotates_without_overwriting(self):
        doc = self._doc(source="janus", authority="janus-authorized", binding_holds=True)
        rendered = json_render.to_dict(doc)
        original = doc["entries"][0]
        annotated = rendered["entries"][0]
        for key, value in original.items():
            assert annotated[key] == value
        assert annotated["authority_verdict"]["is_authority"] is True
        assert rendered["authority_summary"]["renders_as_authority"] == 1

    def test_json_does_not_mutate_the_input(self):
        doc = self._doc()
        before = repr(doc)
        json_render.to_dict(doc)
        assert repr(doc) == before

    def test_both_renderers_agree_on_authority(self):
        """Two renderers, one resolver — they cannot drift apart."""
        cases = [
            ("janus", "janus-authorized", True, True),
            ("janus", "janus-authorized", False, False),
            ("athena", "janus-authorized", True, False),
            ("minerva", "verified", None, False),
        ]
        for source, label, binding, expected in cases:
            e = entry(source=source, authority=label)
            if binding is not None:
                e["binding_holds"] = binding
            doc = {"schema": "desk.v1", "entries": [e]}
            as_json = json_render.to_dict(doc)["entries"][0]["authority_verdict"]
            as_text = text.render(doc)
            assert as_json["is_authority"] is expected
            assert ("AUTHORITY" in as_text) is expected
