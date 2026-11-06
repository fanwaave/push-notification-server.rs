#!/usr/bin/env python3
"""Exhaustive finite *abstract* JetStream disposition model, not runtime proof.

Result publication precedes Ack; failed publication cannot produce Ack/Term;
dead-letter publication precedes Term. Provider delivery is NOT exactly-once.
"""
from collections import deque
from typing import NamedTuple

MAX_ATTEMPTS = 3
OUTCOMES = ("accepted", "retryable", "poison")
TERMINAL = {"acked", "termed", "unresolved"}


class State(NamedTuple):
    outcome: str
    attempt: int
    phase: str
    result_published: bool = False
    dead_letter_published: bool = False
    raw_payload_in_audit: bool = False


def successors(s):
    if s.phase in TERMINAL:
        return ()
    make = lambda phase, result=None, dead=None: State(
        s.outcome, s.attempt, phase,
        s.result_published if result is None else result,
        s.dead_letter_published if dead is None else dead,
        False,
    )
    if s.phase == "received":
        if s.outcome == "poison":
            return (make("dead_published", dead=True),)
        # Represent both durable result publication and a publish failure.
        return (make("result_published", result=True), make("publish_failed"))
    if s.phase == "result_published":
        if s.outcome == "accepted":
            return (make("acked"),)
        if s.attempt < MAX_ATTEMPTS:
            return (make("nak"),)
        return (make("dead_published", dead=True),)
    if s.phase == "publish_failed":
        return (make("nak"),) if s.attempt < MAX_ATTEMPTS else (make("unresolved"),)
    if s.phase == "dead_published":
        return (make("termed"),)
    if s.phase == "nak":
        return (State(s.outcome, s.attempt + 1, "received"),)
    raise AssertionError(s)


def verify():
    start = tuple(State(outcome, 1, "received") for outcome in OUTCOMES)
    queue = deque((s, ()) for s in start)
    seen = set()
    edges = 0
    reached = set()
    while queue:
        s, trace = queue.popleft()
        if s in seen:
            continue
        seen.add(s)
        reached.add(s.phase)
        assert 1 <= s.attempt <= MAX_ATTEMPTS, trace
        assert not s.raw_payload_in_audit, trace
        if s.phase == "acked":
            assert s.outcome == "accepted" and s.result_published, trace
        if s.phase == "termed":
            assert s.dead_letter_published, trace
            if s.outcome != "poison":
                assert s.result_published, trace
        if s.phase == "unresolved":
            assert not s.result_published and not s.dead_letter_published, trace
        next_states = successors(s)
        if s.phase in TERMINAL:
            assert not next_states, trace
        for nxt in next_states:
            edges += 1
            assert nxt.attempt >= s.attempt, trace
            if nxt.phase == "acked":
                assert s.phase == "result_published", trace
            if nxt.phase == "termed":
                assert s.phase == "dead_published", trace
            if nxt.phase == "received" and s.phase == "nak":
                assert nxt.attempt == s.attempt + 1, trace
            queue.append((nxt, trace + (s,)))
    assert len(seen) >= 18 and edges >= 18, (len(seen), edges)
    assert {"acked", "nak", "termed", "publish_failed", "unresolved"} <= reached
    return len(seen), edges


if __name__ == "__main__":
    states, transitions = verify()
    print(f"bounded NATS disposition model: {states} states, {transitions} transitions; abstract proof only")
