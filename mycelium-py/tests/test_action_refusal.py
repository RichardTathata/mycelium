"""The three action refusals stay three things on the way to a caller.

This is the distinction the whole authorisation slice exists to keep, and an SDK that folded them
into one exception would undo it at the last step — the boundary an adopter actually touches.

Before this, `A2aClient.send` raised a bare `KeyError` with the code inside the message, so telling
a denial from an unestablished authority meant parsing English.
"""

from __future__ import annotations

import pytest

from mycelium.a2a import A2aError, ActionRefusedError, _raise_for_error


def _error(code: int, reason: str, revision: str = "rev-1") -> dict:
    return {
        "code": code,
        "message": "refused",
        "data": {"reason": reason, "policy_revision": revision, "checked": ["actor"]},
    }


def test_a_denial_and_an_unestablished_authority_are_not_the_same_exception():
    """The collapse this guards against: *an authority said no* vs *nobody decided*.

    A dashboard that counted the second as a policy breach would report drift that never happened,
    and send an operator looking for an attacker who does not exist.
    """
    with pytest.raises(ActionRefusedError) as denied:
        _raise_for_error(_error(-32030, "action_denied"))
    with pytest.raises(ActionRefusedError) as unestablished:
        _raise_for_error(_error(-32031, "authority_not_established"))

    assert denied.value.denied
    assert not denied.value.authority_not_established

    assert unestablished.value.authority_not_established
    assert not unestablished.value.denied, "an uncovered action is NOT a denial"

    assert denied.value.reason != unestablished.value.reason


def test_a_permitted_action_refused_for_want_of_a_record_is_its_own_case():
    """`evidence_not_recorded` means the policy *allowed* it and the record could not be made.

    Retrying is reasonable once recording is healthy — which is the opposite of what a caller
    should do with a denial, so the two must not arrive as the same thing.
    """
    with pytest.raises(ActionRefusedError) as unrecorded:
        _raise_for_error(_error(-32032, "evidence_not_recorded"))

    assert unrecorded.value.evidence_not_recorded
    assert not unrecorded.value.denied
    assert not unrecorded.value.authority_not_established


def test_the_policy_revision_reaches_the_caller():
    """Without it a *stale policy* refusal is indistinguishable from a real denial.

    They call for opposite responses: redeploy and retry, versus stop.
    """
    with pytest.raises(ActionRefusedError) as refused:
        _raise_for_error(_error(-32031, "authority_not_established", "procurement-2026-q3.r7"))

    assert refused.value.policy_revision == "procurement-2026-q3.r7"
    assert refused.value.checked == ["actor"]


def test_a_non_authorisation_error_is_not_dressed_up_as_a_refusal():
    """An ordinary protocol error must stay ordinary.

    Reporting a malformed request as an authorisation refusal would invent a governance event.
    """
    with pytest.raises(A2aError) as generic:
        _raise_for_error({"code": -32600, "message": "invalid request"})

    assert not isinstance(generic.value, ActionRefusedError)
    assert generic.value.code == -32600


def test_the_reason_survives_a_gateway_that_sends_only_a_code():
    """Older nodes send no `data` block. The code alone still names the refusal.

    A client upgraded ahead of its gateway must not lose the distinction — that is exactly when a
    caller is most likely to be told something it cannot interpret.
    """
    with pytest.raises(ActionRefusedError) as refused:
        _raise_for_error({"code": -32030, "message": "refused"})

    assert refused.value.denied
    assert refused.value.reason == ActionRefusedError.DENIED
    assert refused.value.policy_revision is None, "absent, not invented"
