#!/usr/bin/env python3
"""
ADR meta-linter — advisory radar over docs/adr/.

Surfaces three classes of signal :

  (a) cross-reference graph     — pairs of ADRs that cite the same primitive
                                  (a heuristic for "these two ADRs talk about
                                  the same thing without acknowledging each
                                  other") ;
  (b) lexical opposition        — one ADR says `MUST NOT` X, another says
                                  `MAY` X ;
  (c) orphan ADRs               — not cited by any other ADR or doc.

Output is intentionally advisory. As Godel's incompleteness reminds us,
semantic contradiction detection is undecidable in general ; this script is
a radar, not a proof. Exits 0 even when warnings are emitted so it can run
as a CI hint without blocking merges. Use `--strict` to flip to exit 1 on
any warning (useful for local discipline, not for the CI gate).

Usage :
    scripts/adr-lint.py                  # walk docs/adr/, print warnings
    scripts/adr-lint.py --strict         # exit 1 if any warning is emitted
    scripts/adr-lint.py --json           # machine-readable output
    scripts/adr-lint.py --adr-dir DIR    # alternate ADR directory
    scripts/adr-lint.py --extra-docs DIR # additional doc tree to scan for
                                         # citations (default: docs/)
    scripts/adr-lint.py --emit-state PATH
                                         # write a STATE.md table summarising
                                         # the ADR corpus (number, title,
                                         # status, citers, primitives).
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

ADR_FILE_RE = re.compile(r"^(\d{3})-[a-z0-9-]+\.md$")
ADR_REF_RE = re.compile(r"\bADR-(\d{3})\b")
TITLE_RE = re.compile(r"^#\s+ADR-\d{3}:\s+(.+?)\s*$")

# Primitives that, when co-cited across multiple ADRs, suggest a topic
# cluster worth cross-linking. Kept deliberately small so the signal-to-noise
# stays high — adding too many drops the radar to false positives.
PRIMITIVE_PATTERNS: dict[str, re.Pattern[str]] = {
    "blake3": re.compile(r"\bblake3\b", re.IGNORECASE),
    "mtime": re.compile(r"\bmtime\b", re.IGNORECASE),
    "sqlite": re.compile(r"\bsqlite\b", re.IGNORECASE),
    "executor": re.compile(r"\bExecutor(Bridge|Reporter)?\b"),
    "cache_validation": re.compile(r"\bcache[ _-]validation\b", re.IGNORECASE),
    "event_bus": re.compile(r"\b(EventBus|tokio::broadcast)\b"),
    "session": re.compile(r"\bsession_id\b|\bsessions?\b", re.IGNORECASE),
    "state_db": re.compile(r"\bStateDb\b|\bstate\.db\b"),
    "ray": re.compile(r"\bray\b", re.IGNORECASE),
    "slurm": re.compile(r"\bslurm\b", re.IGNORECASE),
    "skip_job": re.compile(r"\bskip_job\b|\bJobSkipped\b"),
    "cancel": re.compile(r"\bJobCancelled\b|\bcancel_job_ids\b"),
}

# Heuristic opposition patterns. RFC-2119 keywords plus a few oxymake-specific
# verbs. The check is intentionally crude — same primitive, opposite modal.
MODAL_STRONG_NEG = re.compile(
    r"\b(MUST NOT|SHALL NOT|MAY NOT|cannot|never|forbidden|rejected)\b",
    re.IGNORECASE,
)
MODAL_PERMISSIVE = re.compile(
    r"\b(MAY|SHOULD|can be|allowed|optional|supports)\b",
)


@dataclass
class AdrFile:
    number: str
    path: Path
    body: str
    title: str = ""
    status: str = "?"
    metadata: dict[str, str] = field(default_factory=dict)
    references: set[str] = field(default_factory=set)
    primitives: set[str] = field(default_factory=set)

    @property
    def name(self) -> str:
        return f"ADR-{self.number}"


@dataclass
class Warning:
    kind: str
    message: str
    locations: list[str]

    def render(self) -> str:
        loc = ", ".join(self.locations) if self.locations else "—"
        return f"  [{self.kind}] {self.message}  ({loc})"

    def to_dict(self) -> dict[str, object]:
        return {"kind": self.kind, "message": self.message, "locations": self.locations}


def parse_metadata(body: str) -> dict[str, str]:
    """Extract `- **Field:** value` pairs from the Metadata section."""
    out: dict[str, str] = {}
    in_meta = False
    for line in body.splitlines():
        stripped = line.strip()
        if stripped.startswith("## Metadata"):
            in_meta = True
            continue
        if in_meta and stripped.startswith("## "):
            break
        if not in_meta:
            continue
        match = re.match(r"-\s+\*\*([A-Za-z][A-Za-z _-]*)\:\*\*\s*(.*)", stripped)
        if match:
            key = match.group(1).strip().lower().replace(" ", "_")
            out[key] = match.group(2).strip()
    return out


def parse_title_status(body: str) -> tuple[str, str]:
    """Pull `# ADR-NNN: Title` and the first line of `## Status` from body."""
    title = ""
    status = "?"
    lines = body.splitlines()
    for idx, line in enumerate(lines):
        if not title:
            match = TITLE_RE.match(line)
            if match:
                title = match.group(1).strip()
        if line.strip().startswith("## Status"):
            for follow in lines[idx + 1 :]:
                if follow.strip().startswith("## "):
                    break
                if follow.strip():
                    status = follow.strip()
                    break
            break
    return title, status


def load_adr(path: Path) -> AdrFile | None:
    file_match = ADR_FILE_RE.match(path.name)
    if not file_match:
        return None
    number = file_match.group(1)
    if number == "000":
        # Template — skip from corpus analysis (it cites no one).
        return None
    body = path.read_text(encoding="utf-8")
    references = {n for n in ADR_REF_RE.findall(body) if n != number}
    primitives = {
        name for name, pattern in PRIMITIVE_PATTERNS.items() if pattern.search(body)
    }
    title, status = parse_title_status(body)
    return AdrFile(
        number=number,
        path=path,
        body=body,
        title=title,
        status=status,
        metadata=parse_metadata(body),
        references=references,
        primitives=primitives,
    )


def discover_adrs(adr_dir: Path) -> list[AdrFile]:
    adrs: list[AdrFile] = []
    for path in sorted(adr_dir.iterdir()):
        adr = load_adr(path)
        if adr is not None:
            adrs.append(adr)
    return adrs


def discover_external_refs(roots: Iterable[Path]) -> set[str]:
    """Walk auxiliary doc trees and collect ADR-XXX citations."""
    refs: set[str] = set()
    for root in roots:
        if not root.exists():
            continue
        for path in root.rglob("*.md"):
            if "adr/" in str(path):
                continue
            try:
                text = path.read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                continue
            refs.update(ADR_REF_RE.findall(text))
    return refs


def check_cross_references(adrs: list[AdrFile]) -> list[Warning]:
    """(a) pairs of ADRs that share a primitive without cross-citing."""
    by_primitive: dict[str, list[AdrFile]] = defaultdict(list)
    for adr in adrs:
        for primitive in adr.primitives:
            by_primitive[primitive].append(adr)
    warnings: list[Warning] = []
    seen_pairs: set[tuple[str, str, str]] = set()
    for primitive, owners in by_primitive.items():
        if len(owners) < 2:
            continue
        for i, left in enumerate(owners):
            for right in owners[i + 1 :]:
                if left.number in right.references or right.number in left.references:
                    continue
                key = (primitive, left.number, right.number)
                if key in seen_pairs:
                    continue
                seen_pairs.add(key)
                warnings.append(
                    Warning(
                        kind="cross-ref",
                        message=(
                            f"{left.name} and {right.name} both reference "
                            f"`{primitive}` but neither cites the other"
                        ),
                        locations=[str(left.path), str(right.path)],
                    )
                )
    return warnings


def check_lexical_opposition(adrs: list[AdrFile]) -> list[Warning]:
    """(b) MUST NOT in one ADR vs MAY in another over the same primitive."""
    warnings: list[Warning] = []
    for primitive, pattern in PRIMITIVE_PATTERNS.items():
        denials: list[tuple[AdrFile, str]] = []
        permissions: list[tuple[AdrFile, str]] = []
        for adr in adrs:
            for line in adr.body.splitlines():
                if not pattern.search(line):
                    continue
                if MODAL_STRONG_NEG.search(line):
                    denials.append((adr, line.strip()))
                elif MODAL_PERMISSIVE.search(line):
                    permissions.append((adr, line.strip()))
        for deny_adr, _ in denials:
            for allow_adr, _ in permissions:
                if deny_adr.number == allow_adr.number:
                    continue
                warnings.append(
                    Warning(
                        kind="lexical-opposition",
                        message=(
                            f"{deny_adr.name} forbids `{primitive}` while "
                            f"{allow_adr.name} permits it — verify intent"
                        ),
                        locations=[str(deny_adr.path), str(allow_adr.path)],
                    )
                )
    # De-duplicate identical (kind, message) pairs.
    deduped: dict[tuple[str, str], Warning] = {}
    for warning in warnings:
        deduped.setdefault((warning.kind, warning.message), warning)
    return list(deduped.values())


def check_orphans(adrs: list[AdrFile], external_refs: set[str]) -> list[Warning]:
    """(c) ADRs not cited by any other ADR or external doc."""
    cited: set[str] = set(external_refs)
    for adr in adrs:
        cited.update(adr.references)
    warnings: list[Warning] = []
    for adr in adrs:
        if adr.number in cited:
            continue
        warnings.append(
            Warning(
                kind="orphan",
                message=(
                    f"{adr.name} is not cited by any other ADR or scanned doc"
                ),
                locations=[str(adr.path)],
            )
        )
    return warnings


def emit_state(
    adrs: list[AdrFile],
    external_refs: set[str],
    output: Path,
) -> None:
    """Write a STATE.md snapshot — number, title, status, citers, primitives.

    Deterministic by ADR number so the file is diff-friendly across reruns.
    """
    # Inverted index: which other ADRs cite this one ?
    citers: dict[str, set[str]] = defaultdict(set)
    for adr in adrs:
        for ref in adr.references:
            citers[ref].add(adr.number)

    lines: list[str] = []
    lines.append("# ADR State")
    lines.append("")
    lines.append(
        "*Projection of `docs/adr/` emitted by `scripts/adr-lint.py "
        "--emit-state`. Do not edit by hand — regenerate.*"
    )
    lines.append("")
    lines.append(
        "Status is the first non-empty line under each ADR's `## Status` "
        "section, verbatim. *Cited by* counts incoming references from "
        "other ADRs (external doc citations are not listed but are tallied "
        "in the *external citations* column)."
    )
    lines.append("")
    lines.append("| ADR | Title | Status | Cited by | External | Primitives |")
    lines.append("|---|---|---|---|---|---|")
    for adr in sorted(adrs, key=lambda a: a.number):
        inbound = sorted(citers.get(adr.number, set()))
        cited_str = ", ".join(f"ADR-{n}" for n in inbound) if inbound else "—"
        external = "✓" if adr.number in external_refs else "—"
        primitives = ", ".join(sorted(adr.primitives)) if adr.primitives else "—"
        title = adr.title or "(no title)"
        title = title.replace("|", "\\|")
        primitives = primitives.replace("|", "\\|")
        status = adr.status.replace("|", "\\|")
        lines.append(
            f"| ADR-{adr.number} | {title} | {status} | {cited_str} | "
            f"{external} | {primitives} |"
        )
    lines.append("")
    lines.append(f"*{len(adrs)} ADRs scanned. Regenerate after every ADR edit.*")
    lines.append("")
    output.write_text("\n".join(lines), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--adr-dir",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "docs" / "adr",
        help="directory containing ADR markdown files",
    )
    parser.add_argument(
        "--extra-docs",
        type=Path,
        nargs="*",
        default=None,
        help="additional documentation directories to scan for citations",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="exit 1 if any warning is emitted (default: advisory, exit 0)",
    )
    parser.add_argument(
        "--json", action="store_true", help="emit warnings as JSON to stdout"
    )
    parser.add_argument(
        "--emit-state",
        type=Path,
        default=None,
        help="write a STATE.md projection of the ADR corpus to this path",
    )
    args = parser.parse_args(argv)

    adr_dir: Path = args.adr_dir
    if not adr_dir.is_dir():
        print(f"adr-lint: {adr_dir} is not a directory", file=sys.stderr)
        return 2

    extra_docs: list[Path] = list(args.extra_docs or [])
    if not extra_docs:
        default_docs = adr_dir.parent
        if default_docs.is_dir():
            extra_docs.append(default_docs)

    adrs = discover_adrs(adr_dir)
    external_refs = discover_external_refs(extra_docs) - {a.number for a in adrs}

    warnings: list[Warning] = []
    warnings.extend(check_cross_references(adrs))
    warnings.extend(check_lexical_opposition(adrs))
    warnings.extend(check_orphans(adrs, external_refs))

    if args.emit_state is not None:
        emit_state(adrs, external_refs, args.emit_state)
        if not args.json:
            print(f"  STATE.md written to {args.emit_state}")

    if args.json:
        print(
            json.dumps(
                {
                    "adr_count": len(adrs),
                    "warning_count": len(warnings),
                    "warnings": [w.to_dict() for w in warnings],
                },
                indent=2,
            )
        )
    else:
        print(f"adr-lint: scanned {len(adrs)} ADRs in {adr_dir}")
        if not warnings:
            print("  no warnings.")
        else:
            print(f"  {len(warnings)} advisory warning(s):")
            for warning in warnings:
                print(warning.render())
        print(
            "  (advisory — semantic contradiction is undecidable ; "
            "treat output as a radar, not a proof.)"
        )

    if args.strict and warnings:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
