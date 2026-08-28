"""The authority rules, as tests.

Each test names the failure it prevents. A test here that cannot fail when the
doctrine changes is worthless -- these exist to break loudly if someone
reintroduces the collapse `docs/authority.md` forbids.
"""

from __future__ import annotations

import dataclasses

import pytest

from beacon_legacy.contracts import (
    NOT_ASSURING,
    Attachment,
    Authority,
    Decider,
    Enforcement,
    Lifecycle,
    OperatorAction,
)


def _authority(**overrides) -> Authority:
    base = dict(
        decider=Decider.HUMAN,
        attachment=Attachment.BYTES_VERIFIED,
        enforcement=Enforcement.REPORTED,
        lifecycle=Lifecycle.AUTHORIZED,
        mechanism="janus.ruling",
        meaning="a human ruled on these exact bytes",
    )
    return Authority(**{**base, **overrides})


class TestNoCollapse:
    """The generic-"approved" collapse must stay unrepresentable."""

    def test_no_boolean_reduction_exists(self):
        """No `is_approved`. Reducing four axes to one boolean IS the
        collapse; that reduction is Kevin's call, not an API convenience."""
        a = _authority()
        assert not hasattr(a, "is_approved")
        assert not hasattr(a, "approved")

    def test_authority_is_not_orderable(self):
        """Mechanisms must not be rankable. The prior review ranked them on
        "binds to bytes" and got it backwards -- Icarus binds wider than Janus
        and enforces, while Janus deliberately does not."""
        with pytest.raises(TypeError):
            _ = _authority() < _authority(mechanism="icarus.plan_approval")

    def test_lifecycle_has_no_module_name(self):
        """`janus-authorized` hardcodes the disproved claim that Janus is the
        sole authorizing mechanism. Adapters emit AUTHORIZED + decider."""
        assert Lifecycle.AUTHORIZED.value == "authorized"
        assert not any("janus" in v.value for v in Lifecycle)

    def test_no_lifecycle_value_reads_as_generic_approval(self):
        assert not any(v.value in {"approved", "approval"} for v in Lifecycle)


class TestAttachment:
    """The four bytes-* states are four different facts about the world."""

    def test_unverifiable_and_unchecked_are_distinct(self):
        """The refinement Lane C's question forced. "Can never be checked by
        anyone" (Janus text-kind) and "we did not check this run" (module
        unreachable) lead to different operator actions: nothing vs. go look.
        Rendering them alike rebuilds the collapse in miniature."""
        assert Attachment.BYTES_UNVERIFIABLE is not Attachment.BYTES_UNCHECKED
        assert Attachment.BYTES_UNVERIFIABLE.value != Attachment.BYTES_UNCHECKED.value

    def test_unbound_is_not_unverifiable(self):
        """Fitted to real data, not to the code path. Of 13 live Janus gates:
        8 git, 1 file, 4 bound to NOTHING, and 0 text. So the state that
        actually occurs is UNBOUND -- a ruling attached to no artifact at all
        -- while the `text` path the tri-state analysis was built around has
        never been exercised.

        UNBOUND binds nothing; BYTES_UNVERIFIABLE binds bytes it cannot
        re-derive but can still show a reader. Collapsing them hides the more
        dangerous one, and all four live UNBOUND gates are `approved`."""
        assert Attachment.UNBOUND is not Attachment.BYTES_UNVERIFIABLE
        assert not _authority(attachment=Attachment.UNBOUND).assures()

    @pytest.mark.parametrize(
        "attachment",
        [
            Attachment.BYTES_DRIFTED,
            Attachment.BYTES_UNVERIFIABLE,
            Attachment.BYTES_UNCHECKED,
            Attachment.UNBOUND,
        ],
    )
    def test_unassuring_attachments_never_assure(self, attachment):
        """ "Cannot tell" must never read as "fine" -- Janus's own
        verify_binding docstring says exactly this about its None return."""
        assert not _authority(attachment=attachment).assures()
        assert attachment in NOT_ASSURING

    def test_verified_assures(self):
        assert _authority(attachment=Attachment.BYTES_VERIFIED).assures()

    def test_intent_assures_because_it_is_not_a_failed_check(self):
        """Athena binds an intent rather than a payload ON PURPOSE: replaying
        a stored mutation would re-execute a stale payload under authorization
        evaluated at a different moment. That is a different promise, not a
        weaker one, and must not render as a warning."""
        assert _authority(attachment=Attachment.INTENT, enforcement=Enforcement.NONE).assures()


class TestOperatorAction:
    """The axis that answers "what do I do differently?"."""

    def test_drift_without_enforcement_must_be_contained(self):
        """Janus reports drift and stops, to stay out of the permission path.
        So nothing in the system prevents acting on a void ruling -- only a
        person can close that gap."""
        a = _authority(attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.REPORTED)
        assert a.operator_action() is OperatorAction.ACT

    def test_drift_with_enforcement_needs_nothing(self):
        """Icarus re-derives planApprovalDigest on read and calls
        sourceInvalid() on mismatch. The system already refused; the operator
        is investigating, not containing."""
        a = _authority(attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.ENFORCED)
        assert a.operator_action() is OperatorAction.NONE

    def test_healthy_binding_needs_nothing(self):
        assert (
            _authority(attachment=Attachment.BYTES_VERIFIED).operator_action()
            is OperatorAction.NONE
        )

    @pytest.mark.parametrize("attachment", [Attachment.UNBOUND, Attachment.BYTES_UNVERIFIABLE])
    def test_never_checkable_needs_a_decision(self, attachment):
        """The regression this pins: an earlier boolean `operator_must_act()`
        answered False for UNBOUND, so the four live unbound-and-approved
        gates the doctrine page is built around asked nobody to look. The
        weakest state in the ledger was also the quietest.

        These are not breaches and not urgent, but they never resolve on their
        own -- accept the ruling as resting on the raiser's word, or re-raise
        with a checkable binding. That is a decision, not a containment."""
        a = _authority(attachment=attachment)
        assert a.operator_action() is OperatorAction.DECIDE

    def test_unchecked_is_deliberately_quiet(self):
        """A stated choice, not an oversight. BYTES_UNCHECKED is a fact about
        this run rather than about the decision, and `beacon-legacy-desk doctor` owns that
        surface. Most product-trace hops are legitimately unreachable today
        (2 of 7 joinable), so flagging every unchecked entry would drown ACT
        and DECIDE. `assures()` is still False, so it keeps its warning."""
        a = _authority(attachment=Attachment.BYTES_UNCHECKED)
        assert a.operator_action() is OperatorAction.NONE
        assert not a.assures()

    def test_spent_decision_needs_nothing(self):
        """Gate g1a603e736cb shipped `superseded` and still returned DECIDE.
        Supersession IS the resolution, so the advice it produced ("re-raise
        with a checkable binding") was wrong, not merely superfluous."""
        a = _authority(attachment=Attachment.UNBOUND, spent=True)
        assert a.operator_action() is OperatorAction.NONE

    @pytest.mark.parametrize("attachment", list(Attachment))
    @pytest.mark.parametrize(
        "enforcement", [Enforcement.ENFORCED, Enforcement.REPORTED, Enforcement.NONE]
    )
    def test_spent_beats_every_other_axis(self, attachment, enforcement):
        """Whatever it is attached to and whoever enforces it, a closed
        decision asks nothing. Includes spent+drifted, which would otherwise
        be the one ACT case."""
        a = _authority(attachment=attachment, enforcement=enforcement, spent=True)
        assert a.operator_action() is OperatorAction.NONE

    def test_unspent_still_asks_the_same_questions(self):
        """The guard against the mutation that makes the test above pass for
        the wrong reason: code returning NONE unconditionally would satisfy
        every `spent` assertion. These pin that the OTHER answers survive."""
        assert (
            _authority(
                attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.REPORTED
            ).operator_action()
            is OperatorAction.ACT
        )
        assert _authority(attachment=Attachment.UNBOUND).operator_action() is OperatorAction.DECIDE

    def test_spent_is_not_derivable_from_lifecycle(self):
        """Why `spent` is a field rather than a lifecycle check.

        The mandated vocabulary has no terminal-negative value, so an adapter
        that refuses to default upward lands refused/withdrawn/expired/
        superseded ALL on REVIEWED. A live in-review decision carries the same
        lifecycle as a superseded one, so reading `lifecycle` cannot separate
        them -- it would silence genuine in-review entries or change nothing.
        If this test ever fails, the vocabulary gained a spent state and
        `spent` should be deleted rather than kept alongside it.
        """
        live = _authority(lifecycle=Lifecycle.REVIEWED, attachment=Attachment.UNBOUND)
        dead = _authority(lifecycle=Lifecycle.REVIEWED, attachment=Attachment.UNBOUND, spent=True)
        assert live.lifecycle is dead.lifecycle
        assert live.operator_action() is not dead.operator_action()
        assert not any(
            v.value in {"superseded", "withdrawn", "expired", "refused", "spent"} for v in Lifecycle
        )

    def test_action_is_three_valued(self):
        """A boolean cannot carry both "something broke" and "nothing was ever
        checkable" without silencing one of them."""
        assert len(set(OperatorAction)) == 3


class TestAgentNeverApproves:
    """Doctrine that must be a property of the code, not of a docstring."""

    def test_agent_never_assures_even_when_bytes_verify(self):
        """The highest-consequence case in the file: an agent self-attesting
        over content it computed itself. The attachment says a digest matched;
        it says nothing about whether that party may approve it.

        This shipped wrong for one commit -- `assures()` never consulted
        `decider`, so agent + bytes-verified returned True while the docstring
        promised it never would."""
        a = _authority(decider=Decider.AGENT, attachment=Attachment.BYTES_VERIFIED)
        assert not a.assures()

    @pytest.mark.parametrize("attachment", list(Attachment))
    def test_agent_never_assures_for_any_attachment(self, attachment):
        assert not _authority(decider=Decider.AGENT, attachment=attachment).assures()

    def test_human_and_policy_still_assure_on_a_good_binding(self):
        for decider in (Decider.HUMAN, Decider.POLICY):
            assert _authority(decider=decider, attachment=Attachment.BYTES_VERIFIED).assures()

    def test_enforcement_is_independent_of_attachment(self):
        """Binds-to-bytes and enforces-on-read are separate properties, and a
        mechanism can have either without the other. If these ever collapse
        into one field, this test should fail."""
        fields = {f.name for f in dataclasses.fields(Authority)}
        assert {"attachment", "enforcement"} <= fields


class TestMeaning:
    def test_meaning_is_required(self):
        """Every approval states what it means in the OWNING module's words.
        Beacon legacy desk is not entitled to paraphrase another module's semantics more
        briefly than the module does."""
        with pytest.raises(TypeError):
            Authority(
                decider=Decider.HUMAN,
                attachment=Attachment.INTENT,
                enforcement=Enforcement.NONE,
                lifecycle=Lifecycle.AUTHORIZED,
                mechanism="athena.approval_request",
            )

    def test_mechanism_records_buzz_kind_numbers_literally(self):
        """Buzz is upstream OSS (github.com/block/buzz.git) and can renumber
        without consulting AI Town. Recording the observed number makes a
        future divergence surface as a change, not a silent relabel."""
        a = _authority(mechanism="buzz.kind.46030", attachment=Attachment.HANDLE)
        assert "46030" in a.mechanism
