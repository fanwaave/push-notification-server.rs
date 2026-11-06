# Formal NATS disposition and AttoLang reference

Run the model with `python3 conformance/formal_wave6_model.py`.

This is a **bounded, exhaustive abstract model** of the NATS v1 result,
Ack/Nak, redacted dead-letter and Term ordering. It explores successful result
publication, publish failure, retries through a finite attempt limit, and poison
envelopes. A failed result publication may not Ack; if the final result cannot
be published, the modeled state is `unresolved` and requires operational
reconciliation rather than falsely claiming success. Terminal states are
absorbing. Exploration witnesses enforce non-vacuity.

`push_disposition_guards.atto` is a pure AttoLang reference policy for the
same admission boundary; it is **not compiler-executed or wired into Rust**.
Future integration must compare real Rust dispositions and compiled AttoLang
output against common trace vectors, including NATS redelivery and final-attempt
failures, on exact source heads.

**Scope:** This model cannot prove provider side effects exactly-once, durable
JetStream storage, credential redaction in real payloads, or physical delivery.
The service is at-least-once. The existing Rust tests and live NATS CI remain
required to prove concrete behavior.
