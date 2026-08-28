"""The never-collapse rules, as tests.

Three collapses are pinned here. Each was a real defect in this project, not a
hypothesis:

1. An approval's four axes reduced to one verdict word.
2. "No approval was ever sought" rendered as "an approval could not be
   verified" — which would fire on the majority of a real desk.
3. "Nothing to report" rendered as "we could not ask".

If any of these is ever deleted to make a change go through, the change is the
problem.
"""

from __future__ import annotations

import pytest

from beacon_legacy.contracts.authority import (
    Attachment,
    Decider,
    Enforcement,
    Lifecycle,
    OperatorAction,
)
from beacon_legacy.render import json_render, text
from beacon_legacy.render.authority import FORBIDDEN_RENDERINGS, read_authority, view


def authority(**overrides):
    base = {
        "decider": Decider.HUMAN,
        "attachment": Attachment.BYTES_VERIFIED,
        "enforcement": Enforcement.REPORTED,
        "lifecycle": Lifecycle.AUTHORIZED,
        "mechanism": "janus.ruling",
        "meaning": "a human ruled on specific bytes",
    }
    base.update(overrides)
    return base


def entry(**overrides):
    base = {
        "source": "janus",
        "owner": "kevin",
        "title": "a thing",
        "status": "approved",
        "timestamp": "2026-08-24T00:00:00Z",
        "schema_version": "janus.gate.v1",
        "ref": "g0001",
        "link": "janus://gates/g0001",
        "authority": authority(),
    }
    base.update(overrides)
    return base


def doc(*entries, **extra):
    return {"schema": "desk.v1", "entries": list(entries), **extra}


class TestPolicyIsImportedNeverReimplemented:
    """The renderer must ask the contract, not encode a second opinion."""

    def test_every_axis_value_round_trips(self):
        for att in Attachment:
            for enf in Enforcement:
                resolved = view(entry(authority=authority(attachment=att, enforcement=enf)))
                assert resolved.authority is not None
                assert resolved.authority.attachment is att
                assert resolved.authority.enforcement is enf
                # The contract owns the verdict; the view must not disagree.
                assert resolved.action is resolved.authority.operator_action()
                assert resolved.assures is resolved.authority.assures()

    def test_string_values_are_accepted_after_a_json_round_trip(self):
        """asdict gives StrEnum members; json.loads gives plain strings."""
        plain = view(
            entry(
                authority={
                    "decider": "human",
                    "attachment": "unbound",
                    "enforcement": "reported",
                    "lifecycle": "authorized",
                    "mechanism": "janus.ruling",
                    "meaning": "x",
                }
            )
        )
        assert plain.authority is not None
        assert plain.action is OperatorAction.DECIDE

    def test_the_live_unbound_gates_ask_for_a_decision(self):
        """4 of 13 live Janus gates are unbound and carry an approval word."""
        resolved = view(entry(authority=authority(attachment=Attachment.UNBOUND)))
        assert resolved.action is OperatorAction.DECIDE
        assert resolved.assures is False

    def test_the_one_state_that_demands_action(self):
        resolved = view(
            entry(
                authority=authority(
                    attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.REPORTED
                )
            )
        )
        assert resolved.action is OperatorAction.ACT

    def test_a_module_that_already_refused_does_not_demand_action(self):
        resolved = view(
            entry(
                authority=authority(
                    attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.ENFORCED
                )
            )
        )
        assert resolved.action is OperatorAction.NONE


class TestCollapse1_TheAxesAreNeverSummarised:
    def test_no_forbidden_word_is_rendered_anywhere(self):
        """Global, not scoped to entry lines. The renderer's own prose has to
        hold the same vocabulary line it enforces on the modules it renders."""
        for att in Attachment:
            for lifecycle in Lifecycle:
                out = text.render(
                    doc(entry(status="", authority=authority(attachment=att, lifecycle=lifecycle)))
                )
                for word in FORBIDDEN_RENDERINGS:
                    assert word not in out.lower(), f"{att}/{lifecycle} rendered {word!r}"

    def test_the_module_status_word_is_rendered_verbatim_and_attributed(self):
        """status is open-ended module vocabulary; never mapped, never switched on."""
        for word in ("superseded", "withdrawn", "in_progress", "done", "wat"):
            out = text.render(doc(entry(status=word)))
            assert f"janus says: {word}" in out

    def test_all_four_axes_appear(self):
        out = text.render(doc(entry(authority=authority(attachment=Attachment.UNBOUND))))
        assert "unbound" in out
        assert "reported" in out
        assert "decided by human" in out
        assert "janus.ruling" in out

    def test_the_modules_own_meaning_is_quoted_not_paraphrased(self):
        out = text.render(doc(entry(authority=authority(meaning="a very specific promise"))))
        assert "a very specific promise" in out

    def test_no_authority_count_is_emitted(self):
        """There is deliberately no 'N entries are approved' number anywhere."""
        out = text.render(doc(entry(), entry()))
        assert "renders as authority" not in out
        assert "need you to act" in out

    def test_json_emits_axes_not_a_verdict(self):
        rendered = json_render.to_dict(doc(entry()))
        v = rendered["entries"][0]["authority_view"]
        assert "is_authority" not in v
        assert "authority_summary" not in rendered
        assert set(rendered["operator_actions"]) == {a.value for a in OperatorAction}
        assert v["attachment"] == "bytes-verified"

    def test_json_does_not_mutate_or_overwrite(self):
        d = doc(entry())
        before = repr(d)
        rendered = json_render.to_dict(d)
        assert repr(d) == before
        for key, value in d["entries"][0].items():
            assert rendered["entries"][0][key] == value


class TestCollapse2_NoApprovalIsNotAFailedCheck:
    """The common case: most desk entries are work, not approvals."""

    def test_an_entry_without_an_approval_needs_nothing(self):
        resolved = view(entry(source="athena", authority=None))
        assert resolved.authority is None
        assert resolved.problem is None
        assert resolved.action is OperatorAction.NONE

    def test_a_missing_authority_key_behaves_the_same(self):
        bare = {"source": "athena", "owner": "x", "title": "an issue", "status": "open"}
        assert view(bare).action is OperatorAction.NONE
        assert view(bare).problem is None

    def test_it_does_not_render_as_a_problem(self):
        out = text.render(doc(entry(source="athena", title="an issue", authority=None)))
        assert "unreadable" not in out.lower()
        assert "0 need you to act" in out
        assert "an issue" in out

    def test_a_malformed_block_is_distinguishable_from_an_absent_one(self):
        absent = view(entry(authority=None))
        broken = view(entry(authority={"decider": "human"}))
        assert absent.problem is None
        assert broken.problem is not None
        assert absent.action is not broken.action

    @pytest.mark.parametrize(
        "bad",
        [
            {"decider": "human"},
            "a string",
            {**authority(), "attachment": "made-up"},
            {**authority(), "decider": "nobody"},
        ],
    )
    def test_a_bad_value_is_reported_never_guessed(self, bad):
        resolved = view(entry(authority=bad))
        assert resolved.problem is not None
        assert resolved.glyph == "?"
        assert resolved.action is OperatorAction.DECIDE

    def test_read_authority_returns_none_rather_than_raising_for_absent(self):
        assert read_authority({"source": "athena"}) is None

    def test_the_counts_separate_approvals_from_everything_else(self):
        out = text.render(doc(entry(), entry(authority=None), entry(authority=None)))
        assert "1 of 3 entries record a decision" in out


class TestCollapse3_FailedReadIsNotAnEmptyRead:
    def test_empty_and_clean_says_so(self):
        out = text.render(doc())
        assert "genuinely empty desk" in out
        assert "INCOMPLETE" not in out

    def test_empty_with_unreachable_never_claims_a_clean_compose(self):
        out = text.render(doc(unreachable=[{"module": "buzz", "address": "ws://100.x:13000"}]))
        assert "INCOMPLETE" in out
        assert "genuinely empty desk" not in out
        assert "100.x:13000" in out

    def test_a_partial_read_qualifies_the_counts(self):
        out = text.render(doc(entry(), unreachable=[{"module": "icarus", "reason": "no PATH"}]))
        assert "did not answer" in out
        assert "incomplete" in out.lower()
        counts_at = out.index("need you to act")
        assert out.index("did not answer") > counts_at


class TestWorklistOrderIsNotAnAuthorityRanking:
    """docs/authority.md permits ordering by operator action, and only that."""

    def _mixed(self):
        act = entry(
            ref="MARK-ACT",
            authority=authority(
                attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.REPORTED
            ),
        )
        decide = entry(ref="MARK-DECIDE", authority=authority(attachment=Attachment.UNBOUND))
        nothing = entry(
            ref="MARK-NONE",
            authority=authority(
                attachment=Attachment.BYTES_VERIFIED, enforcement=Enforcement.ENFORCED
            ),
        )
        return doc(nothing, decide, act)

    def test_worklist_orders_act_then_decide_then_none(self):
        out = text.render(self._mixed(), worklist=True)
        assert out.index("MARK-ACT") < out.index("MARK-DECIDE") < out.index("MARK-NONE")

    def test_the_ordering_is_anti_correlated_with_authority_strength(self):
        """The proof it is not the banned ranking wearing a sort key.

        A Janus ruling that reports drift sorts FIRST; an Icarus-style approval
        that enforced and refused sorts LAST. An authority ranking inverts both.
        """
        out = text.render(self._mixed(), worklist=True)
        drifted_reported = out.index("MARK-ACT")
        drifted_enforced = out.index("MARK-NONE")
        assert drifted_reported < drifted_enforced

    def test_the_heading_never_says_importance_or_priority(self):
        out = text.render(self._mixed(), worklist=True)
        heading = out.splitlines()[1].lower()
        assert "needs you first" in heading
        for banned in ("important", "priority", "critical", "severity", "ranked"):
            assert banned not in heading

    def test_the_document_keeps_its_own_order_by_default(self):
        d = self._mixed()
        default = text.render(d)
        assert default.index("MARK-NONE") < default.index("MARK-DECIDE") < default.index("MARK-ACT")

    def test_ties_break_on_recency(self):
        older = entry(ref="MARK-OLD", timestamp="2026-08-01T00:00:00Z", authority=None)
        newer = entry(ref="MARK-NEW", timestamp="2026-08-24T00:00:00Z", authority=None)
        out = text.render(doc(older, newer), worklist=True)
        assert out.index("MARK-NEW") < out.index("MARK-OLD")


class TestTheReaderNeverSilentlyDropsAField:
    """The renderer rebuilds `Authority` field by field, so a field added to the
    contract is dropped silently and takes its dataclass default.

    That happened: `spent` was added to close B1, the adapter set it, the
    contract honoured it, 198 tests passed — and the renderer reconstructed
    every entry with `spent=False`, so five superseded gates kept asking for a
    decision on screen. Two lanes each verified their own half correctly and
    the composed result was still wrong.

    This is the guard for the general case, not for `spent`.
    """

    def test_every_contract_field_survives_the_round_trip(self):
        """Fails the day the contract gains a field `read_authority` ignores."""
        import dataclasses

        from beacon_legacy.contracts.authority import Authority

        original = Authority(
            decider=Decider.HUMAN,
            attachment=Attachment.UNBOUND,
            enforcement=Enforcement.REPORTED,
            lifecycle=Lifecycle.AUTHORIZED,
            mechanism="janus.ruling",
            meaning="a human ruled",
            spent=True,
        )
        serialized = dataclasses.asdict(original)
        rebuilt = read_authority({"authority": serialized})

        assert rebuilt is not None
        for field in dataclasses.fields(Authority):
            assert getattr(rebuilt, field.name) == getattr(original, field.name), (
                f"read_authority dropped {field.name!r}. Every field the contract "
                f"defines must survive reconstruction, or the renderer silently "
                f"substitutes a default for what the module actually said."
            )

    def test_a_spent_decision_asks_nothing(self):
        e = entry(authority=authority(attachment=Attachment.UNBOUND, spent=True))
        assert view(e).action is OperatorAction.NONE

    def test_an_unspent_one_still_does(self):
        e = entry(authority=authority(attachment=Attachment.UNBOUND, spent=False))
        assert view(e).action is OperatorAction.DECIDE

    def test_only_a_literal_true_marks_a_ruling_spent(self):
        """A truthy string must not silence a live ruling."""
        for truthy in ["yes", 1, "true", [1]]:
            e = entry(authority=authority(attachment=Attachment.UNBOUND, spent=truthy))
            assert view(e).action is OperatorAction.DECIDE, truthy

    def test_the_screen_says_spent_not_just_the_json(self):
        """The bug was invisible on screen; the fix must be visible there."""
        out = text.render(doc(entry(authority=authority(spent=True))))
        assert "SPENT" in out
