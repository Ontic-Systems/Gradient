#!/usr/bin/env python3
"""
check-attribute-mentions.py — guard against unbackticked Gradient `@attribute`
mentions in commit messages, PR titles, PR bodies, issue titles, and issue
bodies. These tokens collide with GitHub `@user-mention` syntax; mentioning
them unguarded pings real strangers (verified: many of these names map to
live GitHub accounts).

Usage:
    check-attribute-mentions.py [--file PATH | --text STRING | --stdin]
                                [--label LABEL]
    check-attribute-mentions.py --self-test

Multiple inputs may be supplied; each is checked independently and labels
print in the violation list. Exit code 0 if clean, 1 if violations found.

Convention enforced (see CONTRIBUTING.md § "Gradient `@attribute` syntax
in Markdown"): wrap every `@<known-attribute>` in inline backticks
(`` `@verified` ``) or place inside a fenced code block.

This is the script-side of the CI lane introduced by PR closing the
backlog item "@attribute CI guard". The CI workflow shells into this
script with the PR title, PR body, and commit subjects+bodies of every
commit in the PR.
"""
from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass

# Curated set of Gradient surface attributes. Includes both effect names
# used as `@<Name>` (parameterized or marker) AND module/function attrs.
# Source of truth:
#  - parser/lexer attribute recognizer in `codebase/compiler/src/parser/parser.rs`
#  - `KNOWN_EFFECTS` in `codebase/compiler/src/typechecker/effects.rs`
#  - parameterized effect names (`Throws`, `FFI`, etc.)
#  - confirmed real-GH-user collisions (Pitfall 35).
#
# Conservative inclusion: even names that returned 404 on GET /users/<n>
# the day this was authored could be registered later. Listing them costs
# nothing.
KNOWN_ATTRIBUTES: tuple[str, ...] = (
    # Module / function / decl attributes
    "trusted",
    "untrusted",
    "verified",
    "cap",
    "extern",
    "export",
    "test",
    "requires",
    "ensures",
    "budget",
    "app",
    "system",
    "runtime_only",
    "allocator",
    "panic",
    "repr",
    "bench",
    "no_std",
    # Effect names (used as `!{Name}` in source, but easy to write `@Name`
    # in prose — and the prose form pings GH users either way).
    "Heap",
    "Stack",
    "Static",
    "Async",
    "Atomic",
    "Volatile",
    "Send",
    "Mut",
    "Time",
    "Actor",
    "IO",
    "Net",
    "FS",
    "Throws",
    "FFI",
    "Pure",
    "Arena",
    "GPU",
    "Region",
)


@dataclass(frozen=True)
class Violation:
    label: str
    line_no: int
    column: int
    line: str
    token: str

    def format(self) -> str:
        # Editor-jump-friendly line:col format.
        pointer = " " * self.column + "^"
        return (
            f"{self.label}:{self.line_no}:{self.column + 1}: "
            f"unbackticked Gradient attribute mention `{self.token}`\n"
            f"  {self.line}\n  {pointer}"
        )


# Build a single regex once. `@<word>` where word starts with a letter or
# underscore. We constrain to the known set to avoid false-positives on
# unrelated `@`-prefixed mentions (real GitHub users we DON'T own).
_ATTR_ALT = "|".join(re.escape(a) for a in sorted(KNOWN_ATTRIBUTES, key=len, reverse=True))
# Match `@<attr>` only when preceded by start-of-line, whitespace, or
# punctuation that's NOT a backtick. We additionally require the
# preceding character not be alphanumeric or `.` (to avoid false-positives
# on email addresses like `foo@verified.com` and similar).
_ATTR_RE = re.compile(
    r"(?P<lead>(?:^|[^\w.`]))@(?P<name>" + _ATTR_ALT + r")(?![\w.])"
)


def find_violations(text: str, label: str = "input") -> list[Violation]:
    """Return all unbackticked `@<known-attr>` mentions in `text`.

    Skips text inside fenced code blocks (```…```) and inline code
    spans (`…`). Inside such spans, mentions are considered guarded
    per CONTRIBUTING.md.
    """
    violations: list[Violation] = []

    # First pass: mask out fenced blocks. Replace contents with spaces
    # so line/column positions remain stable for the second pass.
    masked = list(text)
    n = len(text)
    i = 0
    while i < n:
        # Detect fenced code block: ``` (three or more backticks at start
        # of line OR after whitespace) — simple recognizer.
        if text.startswith("```", i) and (i == 0 or text[i - 1] == "\n"):
            j = i + 3
            # advance to end of line (skip language tag)
            while j < n and text[j] != "\n":
                j += 1
            # find closing fence
            close = text.find("\n```", j)
            if close == -1:
                # unterminated — mask to end
                end = n
            else:
                end = close + 4  # include the trailing ```
            # Mask everything inside (preserve newlines for line numbers)
            for k in range(i, min(end, n)):
                if masked[k] != "\n":
                    masked[k] = " "
            i = end
            continue
        i += 1

    # Second pass: mask out inline code spans (`…`). Honor backslash
    # escapes and skip already-masked regions.
    text_after_fence = "".join(masked)
    out = list(text_after_fence)
    i = 0
    n = len(text_after_fence)
    while i < n:
        ch = text_after_fence[i]
        if ch == "`":
            # find next backtick on the same logical run (could be `…` or ``…``)
            # Count opening run length
            run_len = 0
            while i + run_len < n and text_after_fence[i + run_len] == "`":
                run_len += 1
            # search for matching run of identical length
            search_from = i + run_len
            close_idx = -1
            k = search_from
            while k < n:
                if text_after_fence[k] == "`":
                    cnt = 0
                    while k + cnt < n and text_after_fence[k + cnt] == "`":
                        cnt += 1
                    if cnt == run_len:
                        close_idx = k
                        break
                    k += cnt
                elif text_after_fence[k] == "\n" and run_len == 1:
                    # Single-backtick spans don't cross newlines per CommonMark.
                    break
                else:
                    k += 1
            if close_idx == -1:
                # unmatched — leave intact
                i += run_len
                continue
            # Mask the inside (between runs)
            for kk in range(i + run_len, close_idx):
                if out[kk] != "\n":
                    out[kk] = " "
            i = close_idx + run_len
            continue
        i += 1

    masked_text = "".join(out)

    # Now scan for unbackticked mentions on the masked text.
    for line_no, line in enumerate(masked_text.splitlines(), start=1):
        for m in _ATTR_RE.finditer(line):
            # `lead` may have consumed a leading char; the `@` is at
            # m.start() + len(m.group('lead')).
            at_col = m.start() + len(m.group("lead"))
            token = "@" + m.group("name")
            # Cross-reference against the ORIGINAL text so the displayed
            # line shows what the human wrote (masked text has spaces
            # where code spans were).
            original_line = text.splitlines()[line_no - 1] if line_no - 1 < len(text.splitlines()) else line
            violations.append(
                Violation(
                    label=label,
                    line_no=line_no,
                    column=at_col,
                    line=original_line,
                    token=token,
                )
            )
    return violations


# --------------------------------------------------------------------------- #
# Self-tests
# --------------------------------------------------------------------------- #

_TEST_CASES: list[tuple[str, str, int]] = [
    # (label, text, expected_violation_count)
    ("bare-attr-in-subject", "feat(stdlib): @verified pilot module", 1),
    ("guarded-inline", "feat(stdlib): `@verified` pilot module", 0),
    ("guarded-fenced", "Body:\n```\n@verified\n```\n", 0),
    ("two-bare-mentions", "@cap whitelist + @system mode", 2),
    ("mixed-guard", "`@cap` whitelist + @system mode", 1),
    ("email-not-attr", "ping foo@verified.com if curious", 0),
    ("user-mention-not-listed", "thanks @dependabot for the bump", 0),
    ("dotted-not-attr", "module.@app inside path", 0),
    ("issue-link", "Closes #324 via @untrusted source mode", 1),
    ("backtick-double", "Use ``@verified`` inline", 0),
    ("fenced-with-language", "Example:\n```rust\n#[cfg(@app)]\n```\n", 0),
    ("unterminated-fence-tolerated", "Body:\n```\n@verified never closes", 0),
    ("multi-line-body", "Line one @cap fix\nLine two `@cap` fix\n", 1),
    ("parameterized-effect-prose", "@Throws cascade across modules", 1),
    ("ffi-in-prose", "Rejected @FFI tag without cap", 1),
    ("guarded-effect", "`@Throws(E)` cascade", 0),
    ("trailing-paren-not-attr", "(@app)", 1),
    ("at-start-of-line", "@allocator must be backticked", 1),
    ("punctuation-after", "@allocator, oh no", 1),
    ("conventional-commit-scope", "feat(@app): infer effects", 1),
    ("conventional-commit-scope-guarded", "feat(`@app`): infer effects", 0),
]


def _run_self_tests() -> int:
    failures = 0
    for label, text, expected in _TEST_CASES:
        got = find_violations(text, label=label)
        if len(got) != expected:
            failures += 1
            print(f"FAIL  {label}: expected {expected}, got {len(got)}", file=sys.stderr)
            for v in got:
                print(f"      → {v.format()}", file=sys.stderr)
        else:
            print(f"ok    {label}")
    if failures:
        print(f"\n{failures} self-test(s) failed.", file=sys.stderr)
        return 1
    print(f"\nAll {len(_TEST_CASES)} self-tests passed.")
    return 0


# --------------------------------------------------------------------------- #
# CLI
# --------------------------------------------------------------------------- #

def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--file",
        action="append",
        default=[],
        metavar="PATH",
        help="Read text from PATH. May be given multiple times.",
    )
    parser.add_argument(
        "--text",
        action="append",
        default=[],
        metavar="STRING",
        help="Inline text to check. May be given multiple times.",
    )
    parser.add_argument(
        "--stdin",
        action="store_true",
        help="Read text from stdin.",
    )
    parser.add_argument(
        "--label",
        default=None,
        help="Label for the next --file or --text input (must precede it).",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="Run built-in self-tests and exit.",
    )
    parser.add_argument(
        "--list-attributes",
        action="store_true",
        help="Print the known attribute set and exit.",
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return _run_self_tests()

    if args.list_attributes:
        for a in sorted(KNOWN_ATTRIBUTES):
            print("@" + a)
        return 0

    all_violations: list[Violation] = []

    for path in args.file:
        try:
            with open(path, "r", encoding="utf-8") as f:
                text = f.read()
        except OSError as e:
            print(f"error reading {path}: {e}", file=sys.stderr)
            return 2
        all_violations.extend(find_violations(text, label=path))

    for idx, text in enumerate(args.text):
        label = f"--text[{idx}]"
        all_violations.extend(find_violations(text, label=label))

    if args.stdin:
        text = sys.stdin.read()
        all_violations.extend(find_violations(text, label="<stdin>"))

    if not args.file and not args.text and not args.stdin:
        parser.error("supply at least one of --file, --text, --stdin, --self-test, --list-attributes")

    if all_violations:
        print(
            f"Found {len(all_violations)} unbackticked Gradient attribute "
            f"mention(s). Wrap each in backticks (`@attr`).\n",
            file=sys.stderr,
        )
        for v in all_violations:
            print(v.format(), file=sys.stderr)
            print(file=sys.stderr)
        print(
            "See CONTRIBUTING.md \u00a7 \"Gradient `@attribute` syntax in Markdown\".",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main())
