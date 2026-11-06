#!/usr/bin/env python3
"""Exhaustive finite *abstract* JetStream disposition model, NOT runtime proof.

Checks result-before-Ack, durable dead-letter-before-Term, redaction,
retry exhaustion, and both result/DLQ publication failures. The service
may deliver to a provider more than once: this is NOT an exactly-once proof.
Use explicit exceptions so python -O cannot disable a formal gate.
"""
from collections import deque
from typing import NamedTuple

MAX_ATTEMPTS = 3
OUTCOMES = ("accepted", "retryable", "poison")
TERMINAL = frozenset(("acked", "termed", "unresolved"))


class ModelViolation(ValueError):
    pass


class State(NamedTuple):
    outcome: str
    attempt: int
    phase: str
    result_published: bool = False
    dead_letter_published: bool = False
    raw_payload_in_audit: bool = False


def require(condition, reason):
    if not condition:
        raise ModelViolation(reason)


def check_state(s):
    require(s.outcome in OUTCOMES, "unknown outcome")
    require(type(s.attempt) is int and 1 <= s.attempt <= MAX_ATTEMPTS,
            "invalid delivery attempt")
    require(s.raw_payload_in_audit is False, "raw payload leaked to audit")
    if s.phase == "acked":
        require(s.outcome == "accepted" and s.result_published,
                "Ack before durable accepted result")
    if s.phase == "termed":
        require(s.dead_letter_published, "Term before durable redacted dead letter")
        if s.outcome != "poison":
            require(s.result_published, "retryable Term before result")
    if s.phase == "unresolved":
        # A result may have published before dead-letter publication failed;
        # this outcome must never be promoted to successful Ack/Term.
        require(not s.dead_letter_published, "unresolved state cannot claim DLQ success")


def successors(s):
    check_state(s)
    if s.phase in TERMINAL:
        return ()
    def make(phase, result=None, dead=None):
        return State(
            s.outcome, s.attempt, phase,
            s.result_published if result is None else result,
            s.dead_letter_published if dead is None else dead,
            False,
        )
    if s.phase == "received":
        if s.outcome == "poison":
            return (make("dead_published", dead=True), make("dlq_publish_failed"))
        # Explicitly include independent result-publish failure.
        return (make("result_published", result=True), make("publish_failed"))
    if s.phase == "result_published":
        if s.outcome == "accepted":
            return (make("acked"),)
        if s.attempt < MAX_ATTEMPTS:
            return (make("nak"),)
        return (make("dead_published", dead=True), make("dlq_publish_failed"))
    if s.phase in ("publish_failed", "dlq_publish_failed"):
        # At the final attempt, a failed durable publication must be
        # surfaced as unresolved, never converted to a false success.
        return (make("nak"),) if s.attempt < MAX_ATTEMPTS else (make("unresolved"),)
    if s.phase == "dead_published":
        return (make("termed"),)
    if s.phase == "nak":
        return (State(s.outcome, s.attempt + 1, "received"),)
    raise ModelViolation("unrecognized phase " + s.phase)


def check_transition(before, after):
    check_state(before)
    check_state(after)
    require(after.attempt >= before.attempt, "attempt count regressed")
    if after.phase == "acked":
        require(before.phase == "result_published", "Ack without published result")
    if after.phase == "termed":
        require(before.phase == "dead_published", "Term without published DLQ")
    if after.phase == "received" and before.phase == "nak":
        require(after.attempt == before.attempt + 1, "redelivery attempt not advanced")
        require(not after.result_published and not after.dead_letter_published,
                "redelivery inherited previous publish receipt")
    if before.phase in TERMINAL:
        raise ModelViolation("terminal disposition must not transition")


def verify():
    starts = tuple(State(outcome, 1, "received") for outcome in OUTCOMES)
    queue = deque((s, ()) for s in starts)
    seen = set()
    edges = 0
    reached = set()
    while queue:
        state, trace = queue.popleft()
        if state in seen:
            continue
        seen.add(state)
        reached.add(state.phase)
        check_state(state)
        next_states = successors(state)
        require(state.phase not in TERMINAL or not next_states,
                "terminal state was not absorbing")
        for next_state in next_states:
            check_transition(state, next_state)
            edges += 1
            queue.append((next_state, trace + (state,)))
    require(len(seen) >= 20 and edges >= 20, "vacuous bounded exploration")
    require({"acked", "nak", "termed", "publish_failed",
             "dlq_publish_failed", "unresolved"} <= reached,
            "missing adversarial witness")
    return len(seen), edges


def mutation_controls():
    rejected = 0
    corrupt = (
        State("accepted", 1, "acked", False),
        State("poison", 1, "acked", True),
        State("retryable", 3, "termed", True, False),
        State("accepted", 1, "acked", True, False, True),
        State("accepted", 0, "received"),
        State("accepted", MAX_ATTEMPTS + 1, "received"),
        State("forged", 1, "received"),
        State("retryable", 3, "unresolved", True, True),
    )
    for bad in corrupt:
        try:
            check_state(bad)
        except ModelViolation:
            rejected += 1
        else:
            raise ModelViolation("mutated invalid state was accepted: " + repr(bad))
    invalid_edges = (
        (State("accepted", 1, "received"), State("accepted", 1, "acked", True)),
        (State("retryable", 1, "received"), State("retryable", 1, "termed", True, True)),
        (State("retryable", 2, "nak"), State("retryable", 2, "received")),
        (State("accepted", 1, "acked", True), State("accepted", 2, "received")),
    )
    for before, after in invalid_edges:
        try:
            check_transition(before, after)
        except ModelViolation:
            rejected += 1
        else:
            raise ModelViolation("mutated invalid transition was accepted")
    require(rejected == 12, "missing negative controls")
    return rejected


if __name__ == "__main__":
    states, transitions = verify()
    negative = mutation_controls()
    print(
        f"bounded NATS disposition: {states} states / {transitions} transitions, "
        f"{negative} adversarial rejects; compiler/runtime NOT_EXECUTED"
    )
