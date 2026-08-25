"""The consult-seam, as tests.

Mutation discipline from Phase 1: a test only proves something if it fails
when you break what it guards, and there are two directions to break in.
Several tests here exist specifically to fail against the *plausible wrong
answer* (a seam that proceeds for everything, or refuses for everything),
because a naive assertion passes against both the fix and the mistake.
"""

from __future__ import annotations

import pytest

from mosaic.store.authority import (
    Authority,
    Check,
    Decider,
    DigestBinding,
    Enforcement,
    Force,
    Lifecycle,
    NoBinding,
)
from mosaic.store.envelope import Capability
from mosaic.store.seam import (
    Decision,
    Disposition,
    OperationSpec,
    Policy,
    Reversibility,
    SeamError,
    consult,
)


def _authority(**over) -> Authority:
    base = dict(
        decider=Decider.HUMAN,
        binding=DigestBinding("abc", "janus.sha256.v1", ("artifact",)),
        check=Check.VERIFIED,
        enforcement=Enforcement.REPORTED,
        lifecycle=Lifecycle.AUTHORIZED,
        force=Force.IN_FORCE,
        mechanism="janus.ruling",
        meaning="a human ruled on these exact bytes",
    )
    return Authority(**{**base, **over})


def _op(reversibility=Reversibility.IRREVERSIBLE, capability=Capability.EXECUTION):
    return OperationSpec("delete_thing", capability, reversibility)


class TestFailClosed:
    """The permissive value must never be the silent default."""

    def test_reversibility_has_no_default(self):
        """An operation that forgets to declare reversibility must fail at
        construction. If the permissive value were the default, "reversible"
        becomes the rule nobody decided -- the F1 failure inside a call graph,
        where there is no document to catch it."""
        with pytest.raises(TypeError):
            OperationSpec("delete_thing", Capability.EXECUTION)  # type: ignore[call-arg]

    def test_unlisted_capability_enforces(self):
        """A capability added without a policy entry gets the safe behaviour,
        not the convenient one."""
        policy = Policy(reporting_only=frozenset())
        assert policy.enforces(_op()) is True

    def test_missing_approval_refuses_when_enforced(self):
        d = consult(_op(), None, policy=Policy(), requires_authority=True)
        assert d.disposition is Disposition.REFUSE

    def test_requires_authority_is_explicit_not_inferred(self):
        """ "needs no approval" and "approval is missing" are different facts.
        Letting `authority is None` decide would make them identical -- the
        absent-versus-broken collapse, in the permission path."""
        with pytest.raises(SeamError):
            consult(_op(), _authority(), policy=Policy(), requires_authority=False)


class TestKevinsRuling:
    """report by default, enforce per-capability for irreversible ops."""

    def test_reversible_operations_never_enforce(self):
        d = consult(_op(Reversibility.REVERSIBLE), None, policy=Policy(), requires_authority=True)
        assert d.disposition is Disposition.PROCEED_REPORTING
        assert d.enforced is False

    def test_reporting_capability_proceeds_and_reports(self):
        """Both halves are required. Proceeding silently would make the report
        meaningless, and this is the state where a human must act."""
        policy = Policy(reporting_only=frozenset({Capability.EXECUTION}))
        d = consult(_op(), None, policy=policy, requires_authority=True)
        assert d.disposition is Disposition.PROCEED_REPORTING
        assert d.reason

    def test_enforcing_capability_refuses_the_same_case(self):
        """The guard against a seam that reports for everything: the identical
        input must produce REFUSE under an enforcing policy. A test asserting
        only the reporting case passes against a seam that never refuses."""
        d = consult(_op(), None, policy=Policy(), requires_authority=True)
        assert d.disposition is Disposition.REFUSE

    def test_healthy_approval_proceeds(self):
        """The guard against a seam that refuses everything."""
        d = consult(_op(), _authority(), policy=Policy(), requires_authority=True)
        assert d.disposition is Disposition.PROCEED


class TestForceIsConsultedFirst:
    """A spent decision is not an approval, whatever its check says."""

    @pytest.mark.parametrize("force", [Force.SPENT, Force.PENDING, Force.INDETERMINATE])
    def test_not_in_force_is_never_a_pass(self, force):
        """A superseded ruling that still verifies cleanly must not authorize
        anything. This is the shape that reported five superseded gates as
        live: reading only binding and check, never force."""
        d = consult(
            _op(),
            _authority(force=force, check=Check.VERIFIED),
            policy=Policy(),
            requires_authority=True,
        )
        assert d.disposition is not Disposition.PROCEED

    def test_in_force_and_verified_is_the_only_clean_pass(self):
        assert (
            consult(_op(), _authority(), policy=Policy(), requires_authority=True).disposition
            is Disposition.PROCEED
        )


class TestBindingAndCheck:
    @pytest.mark.parametrize("check", [Check.DRIFTED, Check.UNVERIFIABLE, Check.UNCHECKED])
    def test_unassuring_checks_do_not_pass(self, check):
        d = consult(_op(), _authority(check=check), policy=Policy(), requires_authority=True)
        assert d.disposition is not Disposition.PROCEED

    def test_reason_names_which_check_failed(self):
        """A refusal a human cannot explain is one they will route around."""
        d = consult(
            _op(), _authority(check=Check.DRIFTED), policy=Policy(), requires_authority=True
        )
        assert "binding" in d.reason


class TestContradictionIsRefusedNotResolved:
    def test_record_says_enforced_but_policy_says_report(self):
        """Proceeding either way would contradict one of them. Picking one
        silently is how a policy and a record drift apart while both look
        correct -- so the seam refuses to choose."""
        policy = Policy(reporting_only=frozenset({Capability.EXECUTION}))
        with pytest.raises(SeamError):
            consult(
                _op(),
                _authority(enforcement=Enforcement.ENFORCED),
                policy=policy,
                requires_authority=True,
            )


class TestDecisionIsNotABoolean:
    def test_decision_records_whether_enforcement_was_on(self):
        """A PROCEED under an enforcing policy and a PROCEED that only
        happened because enforcement was off are different facts."""
        strict = consult(_op(), _authority(), policy=Policy(), requires_authority=True)
        lax = consult(
            _op(),
            _authority(),
            policy=Policy(reporting_only=frozenset({Capability.EXECUTION})),
            requires_authority=True,
        )
        assert strict.enforced is True
        assert lax.enforced is False

    def test_no_boolean_reduction_exists(self):
        assert not hasattr(Decision, "allowed")
        assert not hasattr(Decision, "ok")


class TestNoBindingIsNotAnAbsence:
    def test_unbound_approval_does_not_assure(self):
        """3 of 15 live Janus gates are approved and bound to nothing."""
        assert not _authority(binding=NoBinding("raised before bindings existed")).assures()
