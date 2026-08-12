#!/usr/bin/env python3
"""Fail closed when a production SOPS rule still uses bootstrap-only recipients."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from typing import NoReturn

ENV_RULE = re.compile(r"^\s*-\s*path_regex:\s*(?P<value>.+?)\s*$")
LIST_ITEM = re.compile(r"^\s*-\s*(?P<value>\S+)\s*$")
EXACT_RULES = {
    r"^env/enc/dev\.env\.enc$": "dev",
    r"^env/enc/prod\.env\.enc$": "prod",
}


def parse_recipients(path: Path) -> dict[str, set[str]]:
    recipients: dict[str, set[str]] = {"dev": set(), "prod": set()}
    current: str | None = None
    in_age = False

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        rule_match = ENV_RULE.match(raw_line)
        if rule_match:
            value = rule_match.group("value").strip().strip("\"'")
            current = EXACT_RULES.get(value)
            in_age = False
            continue

        stripped = raw_line.strip()
        if current is not None and stripped == "age:":
            in_age = True
            continue

        if in_age:
            item_match = LIST_ITEM.match(raw_line)
            if item_match:
                value = item_match.group("value")
                if value.startswith("age1"):
                    recipients[current].add(value)
                    continue
            if stripped and not stripped.startswith("#"):
                in_age = False

    return recipients


def fail(message: str) -> NoReturn:
    print(f"production SOPS policy: {message}", file=sys.stderr)
    raise SystemExit(1)


def main(argv: list[str]) -> int:
    if len(argv) != 3:
        print(
            "usage: verify-sops-release-policy.py <.sops.yaml> <environment>",
            file=sys.stderr,
        )
        return 2

    policy_path = Path(argv[1])
    environment = argv[2].strip().lower()
    if environment != "prod":
        return 0
    if not policy_path.is_file():
        fail(f"missing policy file: {policy_path}")

    recipients = parse_recipients(policy_path)
    dev = recipients["dev"]
    prod = recipients["prod"]
    if not dev:
        fail("exact dev recipient rule is missing or empty")
    if not prod:
        fail("exact prod recipient rule is missing or empty")
    if len(prod) < 2:
        fail("prod must have at least two independently controlled recipients")
    if not prod.difference(dev):
        fail("prod must include at least one recipient not used by dev")
    if prod == dev:
        fail("prod and dev recipient sets must not be identical")

    print(
        "production SOPS policy verified "
        f"(dev recipients={len(dev)}, prod recipients={len(prod)})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
