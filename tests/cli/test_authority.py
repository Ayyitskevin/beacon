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
    binding_kind,
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


class TestBoundToNothing:
    """`binding.kind == "none"` is 4 of 13 live gates, not a hypothetical.

    A ruling bound to nothing is a permanently different thing from a ruling
    nobody has re-checked yet: the second can be settled by looking, the first
    can never be settled at all.
    """

    def _janus(self, **overrides):
        e = entry(source="janus", authority="janus-authorized")
        e.update(overrides)
        return e

    def test_bound_to_nothing_is_not_authority(self):
        verdict = resolve(self._janus(binding={"kind": "none", "locator": None, "sha256": None}))
        assert verdict.is_authority is False
        assert "bound to nothing" in verdict.display

    def test_it_beats_a_binding_holds_claim(self):
        """Nothing to check against outranks any claim of having checked."""
        verdict = resolve(self._janus(binding={"kind": "none"}, binding_holds=True))
        assert verdict.is_authority is False
        assert "bound to nothing" in verdict.display

    def test_distinguishable_from_merely_unchecked(self):
        nothing = resolve(self._janus(binding={"kind": "none"}))
        unchecked = resolve(self._janus(binding={"kind": "git", "sha256": "abc"}))
        assert nothing.display != unchecked.display
        assert nothing.badge != unchecked.badge
        # "can never be settled" vs "not settled yet" — the two must not share
        # a rechecked value, or a machine consumer cannot tell them apart.
        assert nothing.rechecked is False
        assert unchecked.rechecked is None

    def test_distinguishable_from_stale(self):
        nothing = resolve(self._janus(binding={"kind": "none"}))
        stale = resolve(self._janus(binding={"kind": "git"}, binding_holds=False))
        assert nothing.display != stale.display
        assert nothing.badge != stale.badge

    def test_real_bindings_still_reach_authority(self):
        """The live corpus is git: 8, none: 4, file: 1 — git must still work."""
        for kind in ("git", "file"):
            verdict = resolve(self._janus(binding={"kind": kind}, binding_holds=True))
            assert verdict.is_authority is True, kind

    def test_all_four_janus_states_render_distinctly(self):
        states = [
            self._janus(binding={"kind": "git"}, binding_holds=True),
            self._janus(binding={"kind": "git"}, binding_holds=False),
            self._janus(binding={"kind": "git"}),
            self._janus(binding={"kind": "none"}),
        ]
        displays = {resolve(e).display for e in states}
        assert len(displays) == 4
        assert sum(resolve(e).is_authority for e in states) == 1


class TestBindingKindHelper:
    def test_reads_the_nested_adapter_shape(self):
        assert binding_kind({"binding": {"kind": "git", "sha256": "x"}}) == "git"

    def test_accepts_a_flat_key(self):
        assert binding_kind({"binding_kind": "none"}) == "none"

    def test_nested_wins_over_flat(self):
        assert binding_kind({"binding": {"kind": "git"}, "binding_kind": "none"}) == "git"

    def test_not_stated_is_none(self):
        assert binding_kind({}) is None
        assert binding_kind({"binding": {}}) is None

    def test_normalises_case_and_space(self):
        assert binding_kind({"binding": {"kind": "  GIT "}}) == "git"


class TestFooterAdviceIsFollowable:
    """Advice that cannot be acted on trains people to ignore the footer."""

    def _janus(self, **overrides):
        e = entry(source="janus", authority="janus-authorized")
        e.update(overrides)
        return e

    def test_unbindable_is_not_told_to_recheck(self):
        doc = {"schema": "desk.v1", "entries": [self._janus(binding={"kind": "none"})]}
        out = text.render(doc)
        assert "can never be re-checked" in out
        assert "re-check the ruling in Janus" not in out

    def test_recheckable_is_told_to_recheck(self):
        doc = {"schema": "desk.v1", "entries": [self._janus(binding={"kind": "git"})]}
        out = text.render(doc)
        assert "re-check the ruling in Janus" in out
        assert "can never be re-checked" not in out

    def test_both_present_gets_both_lines(self):
        doc = {
            "schema": "desk.v1",
            "entries": [
                self._janus(binding={"kind": "none"}),
                self._janus(binding={"kind": "git"}),
            ],
        }
        out = text.render(doc)
        assert "re-check the ruling in Janus" in out
        assert "can never be re-checked" in out
        assert "0 entries render as authority" in out


class TestFailedReadIsNotAnEmptyRead:
    """The same collapse as the authority one, in a different field.

    A desk is composed FROM modules. A module that did not answer leaves a hole
    in the document; it does not mean there was nothing to find. "Nothing to
    report" and "we could not ask" are opposite facts, and exactly one of them
    needs an operator.
    """

    def test_empty_and_clean_says_so(self):
        out = text.render({"schema": "desk.v1", "entries": []})
        assert "genuinely empty desk" in out
        assert "INCOMPLETE" not in out

    def test_empty_with_unreachable_never_claims_it_composed_cleanly(self):
        out = text.render(
            {
                "schema": "desk.v1",
                "entries": [],
                "unreachable": [{"module": "buzz", "address": "ws://192.0.2.10:13000"}],
            }
        )
        assert "INCOMPLETE" in out
        assert "genuinely empty desk" not in out
        assert "buzz" in out
        # The address matters: "connection refused" without one is useless.
        assert "192.0.2.10:13000" in out

    def test_a_partial_read_is_flagged_alongside_the_counts(self):
        out = text.render(
            {
                "schema": "desk.v1",
                "entries": [entry(source="athena", authority="reviewed")],
                "unreachable": [{"module": "icarus", "reason": "not on PATH"}],
            }
        )
        assert "did not answer" in out
        assert "incomplete" in out.lower()
        assert "icarus" in out

    def test_accepts_bare_strings_as_well_as_dicts(self):
        out = text.render({"schema": "desk.v1", "entries": [], "unreachable": ["replaydeck"]})
        assert "replaydeck" in out
        assert "INCOMPLETE" in out

    def test_counts_are_never_presented_as_complete_when_a_read_failed(self):
        """Regression: the footer's counts describe what was seen, not what is."""
        doc = {
            "schema": "desk.v1",
            "entries": [entry(source="athena", authority="reviewed")],
            "unreachable": [{"module": "janus"}],
        }
        out = text.render(doc)
        counts_at = out.index("0 entries render as authority")
        warning_at = out.index("did not answer")
        assert warning_at > counts_at, (
            "the incompleteness warning must follow the counts it qualifies"
        )
