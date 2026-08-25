"""The authority rules, as tests.

Each test names the failure it prevents. A test here that cannot fail when the
doctrine changes is worthless -- these exist to break loudly if someone
reintroduces the collapse `docs/authority.md` forbids.
"""

from __future__ import annotations

import dataclasses

import pytest

from mosaic.contracts import (
    NOT_ASSURING,
    Attachment,
    Authority,
    Decider,
    Enforcement,
    Lifecycle,
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

    @pytest.mark.parametrize(
        "attachment",
        [
            Attachment.BYTES_DRIFTED,
            Attachment.BYTES_UNVERIFIABLE,
            Attachment.BYTES_UNCHECKED,
        ],
    )
    def test_unassuring_attachments_never_assure(self, attachment):
        """"Cannot tell" must never read as "fine" -- Janus's own
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
        assert _authority(
            attachment=Attachment.INTENT, enforcement=Enforcement.NONE
        ).assures()


class TestOperatorAction:
    """The axis that answers "what do I do differently?"."""

    def test_drift_without_enforcement_requires_a_human(self):
        """Janus reports drift and stops, to stay out of the permission path.
        So nothing in the system prevents acting on a void ruling -- only a
        person can close that gap."""
        a = _authority(
            attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.REPORTED
        )
        assert a.operator_must_act()

    def test_drift_with_enforcement_does_not(self):
        """Icarus re-derives planApprovalDigest on read and calls
        sourceInvalid() on mismatch. The system already refused; the operator
        is investigating, not containing."""
        a = _authority(
            attachment=Attachment.BYTES_DRIFTED, enforcement=Enforcement.ENFORCED
        )
        assert not a.operator_must_act()

    def test_healthy_binding_requires_nothing(self):
        assert not _authority(attachment=Attachment.BYTES_VERIFIED).operator_must_act()

    def test_enforcement_is_independent_of_attachment(self):
        """Binds-to-bytes and enforces-on-read are separate properties, and a
        mechanism can have either without the other. If these ever collapse
        into one field, this test should fail."""
        fields = {f.name for f in dataclasses.fields(Authority)}
        assert {"attachment", "enforcement"} <= fields


class TestMeaning:
    def test_meaning_is_required(self):
        """Every approval states what it means in the OWNING module's words.
        Mosaic is not entitled to paraphrase another module's semantics more
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
