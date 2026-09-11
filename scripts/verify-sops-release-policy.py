#!/usr/bin/env python3
"""Fail closed when a production SOPS rule still uses bootstrap-only recipients."""

from __future__ import annotations

import re
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

ENV_RULE = re.compile(r"^\s*-\s*path_regex:\s*(?P<value>.+?)\s*$")
ANCHOR_ITEM = re.compile(
    r"^\s*-\s*&(?P<name>[A-Za-z0-9_-]+)\s+"
    r"(?P<recipient>age1[a-z0-9]{58})(?:\s+#.*)?\s*$"
)
LIST_ITEM = re.compile(r"^\s*-\s*(?P<value>\S+)(?:\s+#.*)?\s*$")
AGE_RECIPIENT = re.compile(r"^age1[a-z0-9]{58}$")
ALIAS = re.compile(r"^\*(?P<name>[A-Za-z0-9_-]+)$")
AGE_KEY = re.compile(r"^\s*(?:-\s*)?age:\s*$")
EXACT_RULES = {
    r"^env/enc/dev\.env\.enc$": "dev",
    r"^env/enc/prod\.env\.enc$": "prod",
}


def parse_anchors(lines: list[str]) -> dict[str, str]:
    anchors: dict[str, str] = {}
    for raw_line in lines:
        match = ANCHOR_ITEM.match(raw_line)
        if not match:
            continue
        name = match.group("name")
        recipient = match.group("recipient")
        previous = anchors.get(name)
        if previous is not None and previous != recipient:
            raise ValueError(f"recipient anchor `{name}` is declared more than once")
        anchors[name] = recipient
    return anchors


def resolve_recipient(value: str, anchors: dict[str, str]) -> str | None:
    if AGE_RECIPIENT.fullmatch(value):
        return value
    alias = ALIAS.fullmatch(value)
    if alias is None:
        return None
    name = alias.group("name")
    try:
        return anchors[name]
    except KeyError as error:
        raise ValueError(f"unknown recipient alias `*{name}`") from error


def parse_recipients(path: Path) -> dict[str, set[str]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    anchors = parse_anchors(lines)
    recipients: dict[str, set[str]] = {"dev": set(), "prod": set()}
    current: str | None = None
    in_age = False

    for raw_line in lines:
        rule_match = ENV_RULE.match(raw_line)
        if rule_match:
            value = rule_match.group("value").strip().strip("\"'")
            current = EXACT_RULES.get(value)
            in_age = False
            continue

        stripped = raw_line.strip()
        if current is not None and AGE_KEY.match(raw_line):
            in_age = True
            continue

        if in_age:
            item_match = LIST_ITEM.match(raw_line)
            if item_match:
                recipient = resolve_recipient(item_match.group("value"), anchors)
                if recipient is not None:
                    recipients[current].add(recipient)
                    continue
            if stripped and not stripped.startswith("#"):
                in_age = False

    return recipients


def fail(message: str) -> NoReturn:
    print(f"production SOPS policy: {message}", file=sys.stderr)
    raise SystemExit(1)


def verify(policy_path: Path) -> dict[str, set[str]]:
    if not policy_path.is_file():
        fail(f"missing policy file: {policy_path}")

    try:
        recipients = parse_recipients(policy_path)
    except ValueError as error:
        fail(str(error))

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
    return recipients


def self_test() -> None:
    alex = "age1" + "a" * 58
    prod = "age1" + "b" * 58
    recovery = "age1" + "c" * 58
    valid = f"""keys:
  - &alex {alex}
  - &prod_operator {prod}
  - &recovery {recovery}
creation_rules:
  - path_regex: ^env/enc/dev\\.env\\.enc$
    key_groups:
      - age:
          - *alex
          - *recovery
  - path_regex: ^env/enc/prod\\.env\\.enc$
    key_groups:
      - age:
          - *prod_operator
          - *recovery
"""
    with tempfile.TemporaryDirectory() as directory:
        path = Path(directory, ".sops.yaml")
        path.write_text(valid, encoding="utf-8")
        parsed = parse_recipients(path)
        assert parsed == {"dev": {alex, recovery}, "prod": {prod, recovery}}

        direct = valid.replace("*alex", alex).replace("*prod_operator", prod)
        path.write_text(direct, encoding="utf-8")
        assert parse_recipients(path) == parsed

        path.write_text(valid.replace("*alex", "*missing"), encoding="utf-8")
        try:
            parse_recipients(path)
        except ValueError as error:
            assert str(error) == "unknown recipient alias `*missing`"
        else:
            raise AssertionError("unknown aliases must fail closed")

        duplicate = valid.replace(
            f"  - &prod_operator {prod}",
            f"  - &alex {prod}\n  - &prod_operator {prod}",
        )
        path.write_text(duplicate, encoding="utf-8")
        try:
            parse_recipients(path)
        except ValueError as error:
            assert "declared more than once" in str(error)
        else:
            raise AssertionError("conflicting anchor declarations must fail closed")


def main(argv: list[str]) -> int:
    if argv[1:] == ["--self-test"]:
        self_test()
        print("production SOPS policy self-test passed")
        return 0
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

    recipients = verify(policy_path)
    print(
        "production SOPS policy verified "
        f"(dev recipients={len(recipients['dev'])}, "
        f"prod recipients={len(recipients['prod'])})"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
