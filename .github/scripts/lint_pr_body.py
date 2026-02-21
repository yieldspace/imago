#!/usr/bin/env python3
# -*- coding: utf-8 -*-

from __future__ import annotations

import json
import os
import re
import sys

REQUIRED_HEADINGS = ("Motivation", "Summary", "Validation")
HEADING_PATTERN = re.compile(r"^##[ \t]+(?P<title>[^\n]+)\s*$")
FENCE_PATTERN = re.compile(r"^[ \t]*(?P<fence>`{3,}|~{3,})")
REQUIRED_MARKER_PATTERN = re.compile(r"\s*\(\s*required\s*\)\s*$", re.IGNORECASE)


def emit_error(message: str) -> None:
    print(f"::error::{message}")


def load_event(event_path: str) -> dict:
    try:
        with open(event_path, "r", encoding="utf-8") as fh:
            data = json.load(fh)
    except OSError as exc:
        emit_error(f"Failed to read GITHUB_EVENT_PATH: {exc}")
        raise
    except json.JSONDecodeError as exc:
        emit_error(f"Failed to parse GITHUB_EVENT_PATH as JSON: {exc}")
        raise

    if not isinstance(data, dict):
        emit_error("GITHUB_EVENT_PATH JSON root must be an object.")
        raise ValueError("event json root is not object")
    return data


def parse_sections(pr_body: str) -> dict[str, str]:
    sections: dict[str, str] = {}
    current_title: str | None = None
    current_body_lines: list[str] = []
    in_fenced_code_block = False
    fence_char = ""
    fence_len = 0

    for line in pr_body.splitlines():
        fence_match = FENCE_PATTERN.match(line)
        if fence_match:
            fence = fence_match.group("fence")
            if not in_fenced_code_block:
                in_fenced_code_block = True
                fence_char = fence[0]
                fence_len = len(fence)
            elif fence[0] == fence_char and len(fence) >= fence_len:
                in_fenced_code_block = False
                fence_char = ""
                fence_len = 0

        if not in_fenced_code_block:
            heading_match = HEADING_PATTERN.match(line)
            if heading_match:
                if current_title is not None:
                    body = "\n".join(current_body_lines).strip()
                    sections.setdefault(current_title, body)
                current_title = normalize_section_title(heading_match.group("title"))
                current_body_lines = []
                continue

        if current_title is not None:
            current_body_lines.append(line)

    if current_title is not None:
        body = "\n".join(current_body_lines).strip()
        sections.setdefault(current_title, body)

    return sections


def normalize_section_title(title: str) -> str:
    normalized = title.strip()
    normalized = REQUIRED_MARKER_PATTERN.sub("", normalized)
    return normalized.strip()


def validate_pr_body(pr_body: str) -> list[str]:
    errors: list[str] = []
    if not pr_body.strip():
        errors.append("pull_request.body must not be empty.")
        return errors

    sections = parse_sections(pr_body)

    for heading in REQUIRED_HEADINGS:
        if heading not in sections:
            errors.append(f"`## {heading}` section is missing.")
            continue

        if not sections[heading]:
            errors.append(f"`## {heading}` section body must not be empty.")

    return errors


def main() -> int:
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    if not event_path:
        emit_error("Environment variable GITHUB_EVENT_PATH is not set.")
        return 1

    try:
        event = load_event(event_path)
    except Exception:
        return 1

    pull_request = event.get("pull_request")
    if not isinstance(pull_request, dict):
        emit_error("Event payload does not contain pull_request object.")
        return 1

    pr_body = pull_request.get("body")
    if not isinstance(pr_body, str):
        emit_error("pull_request.body must be a string.")
        return 1

    errors = validate_pr_body(pr_body)
    if errors:
        for error in errors:
            emit_error(error)
        return 1

    print("PR body lint passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
