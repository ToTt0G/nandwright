"""Comparison logic, expected-differences matching, and report writers."""

from __future__ import annotations

import datetime as dt
import difflib
import fnmatch
import json
import re
import tomllib
from collections import defaultdict
from dataclasses import asdict, dataclass
from pathlib import Path

TEMP_WORK_RE = re.compile(r"/[^\s`:]+/n2t_(?:java|run)_[^/\s`:]+/work/")

from java_runner import JavaTruth
from n2t_adapter import N2tResult
from scenarios import SkippedCase


@dataclass(frozen=True)
class ExpectedDiff:
    case: str  # exact case_id or glob pattern
    what: str
    reason: str


@dataclass
class CaseComparison:
    case_id: str
    scenario: str
    project: str
    dialect: str
    status: str  # "match" | "expected-diff" | "mismatch" | "not-supported"
    java_outcome: str
    n2t_outcome: str
    differences: list[str]
    expected_reason: str | None
    java_detail: str
    n2t_detail: str
    java_wall_s: float
    n2t_wall_s: float
    out_matched: bool | None
    out_diff_preview: str | None
    notes: str | None = None
    mutation_kind: str | None = None

    def to_dict(self) -> dict:
        return asdict(self)


def load_expected_differences(toml_path: Path) -> list[ExpectedDiff]:
    if not toml_path.exists():
        return []
    raw = tomllib.loads(toml_path.read_text(encoding="utf-8"))
    entries = []
    for item in raw.get("difference", []):
        entries.append(
            ExpectedDiff(
                case=str(item["case"]),
                what=str(item["what"]),
                reason=str(item["reason"]),
            )
        )
    return entries


def _norm_eol(data: bytes) -> bytes:
    return data.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def _format_outcome(outcome: str, checked: bool, lines: int | None, cmp_line: int | None, script_line: int | None) -> str:
    if outcome == "passed":
        if checked:
            return f"passed ({lines}L)" if lines is not None else "passed"
        return "unchecked"
    if outcome == "comparison-failure":
        return f"cmp-fail@L{cmp_line}" if cmp_line is not None else "cmp-fail"
    if outcome == "error":
        return f"error@L{script_line}" if script_line is not None else "error"
    return outcome


def _short_msg(msg: str | None, max_len: int = 160) -> str:
    if not msg:
        return ""
    cleaned = TEMP_WORK_RE.sub("", msg)
    one_line = " ".join(cleaned.split())
    if len(one_line) > max_len:
        return one_line[: max_len - 3] + "..."
    return one_line


def compare_case(
    truth: JavaTruth,
    n2t: N2tResult,
    expected_diffs: list[ExpectedDiff],
) -> CaseComparison:
    java_label = _format_outcome(
        truth.outcome, truth.checked, truth.compared_lines, truth.comparison_line, truth.script_line
    )
    n2t_label = _format_outcome(
        n2t.outcome, n2t.checked, n2t.compared_lines, n2t.comparison_line, n2t.script_line
    )

    if n2t.outcome == "not-supported" and truth.dialect in {"cpu", "vm"}:
        return CaseComparison(
            case_id=truth.case_id,
            scenario=truth.scenario,
            project=truth.project,
            dialect=truth.dialect,
            status="not-supported",
            java_outcome=java_label,
            n2t_outcome="not-supported",
            differences=[],
            expected_reason=None,
            java_detail=_short_msg(truth.error_message) if truth.outcome == "error" else java_label,
            n2t_detail="not supported yet",
            java_wall_s=truth.wall_time_s,
            n2t_wall_s=n2t.wall_time_s,
            out_matched=None,
            out_diff_preview=None,
            notes=truth.notes,
            mutation_kind=truth.mutation_kind,
        )

    diffs: list[str] = []
    out_matched: bool | None = None
    out_diff_preview: str | None = None

    # 1. Outcome class + checked status
    if truth.outcome != n2t.outcome:
        diffs.append(f"outcome: java={java_label}, n2t={n2t_label}")
    elif truth.outcome == "passed" and truth.checked != n2t.checked:
        diffs.append(f"checked: java={truth.checked}, n2t={n2t.checked}")

    # 2. Passed compared line count
    if truth.outcome == "passed" and n2t.outcome == "passed" and truth.checked and n2t.checked:
        if truth.compared_lines is not None and n2t.compared_lines != truth.compared_lines:
            diffs.append(
                f"compared_lines: java={truth.compared_lines}, n2t={n2t.compared_lines}"
            )

    # 3. Comparison failure line
    if truth.outcome == "comparison-failure" and n2t.outcome == "comparison-failure":
        if truth.comparison_line != n2t.comparison_line:
            diffs.append(
                f"comparison_line: java={truth.comparison_line}, n2t={n2t.comparison_line}"
            )

    # 4. Error script line (only when Java reports a script line)
    if truth.outcome == "error" and n2t.outcome == "error":
        if truth.script_line is not None and n2t.script_line != truth.script_line:
            diffs.append(f"script_line: java={truth.script_line}, n2t={n2t.script_line}")

    # 5. `.out` byte identity after normalizing line endings only
    if truth.outcome in {"passed", "comparison-failure"} and truth.outcome == n2t.outcome:
        j_out = truth.out_bytes()
        n_out = n2t.out_bytes()
        if j_out is not None and n_out is not None:
            j_norm = _norm_eol(j_out)
            n_norm = _norm_eol(n_out)
            if j_norm == n_norm:
                out_matched = True
            else:
                out_matched = False
                diffs.append(
                    f".out bytes differ (normalized java={len(j_norm)}B, n2t={len(n_norm)}B)"
                )
                diff_lines = list(
                    difflib.unified_diff(
                        j_norm.decode("latin-1").splitlines(),
                        n_norm.decode("latin-1").splitlines(),
                        fromfile="java.out",
                        tofile="n2t.out",
                        lineterm="",
                        n=1,
                    )
                )
                out_diff_preview = "\n".join(diff_lines[:16])
        elif j_out is not None and n_out is None:
            out_matched = False
            diffs.append("java wrote .out file but n2t did not")
        elif j_out is None and n_out is not None:
            out_matched = False
            diffs.append("n2t wrote .out file but java did not")

    # Match against expected-differences.toml if any difference exists
    expected_reason = None
    if diffs:
        for exp in expected_diffs:
            if fnmatch.fnmatch(truth.case_id, exp.case):
                expected_reason = f"{exp.what}: {exp.reason}"
                break
        status = "expected-diff" if expected_reason else "mismatch"
    else:
        status = "match"

    java_detail = _short_msg(truth.error_message) if truth.outcome == "error" else java_label
    n2t_detail = _short_msg(n2t.error_message) if n2t.outcome == "error" else n2t_label

    return CaseComparison(
        case_id=truth.case_id,
        scenario=truth.scenario,
        project=truth.project,
        dialect=truth.dialect,
        status=status,
        java_outcome=java_label,
        n2t_outcome=n2t_label,
        differences=diffs,
        expected_reason=expected_reason,
        java_detail=java_detail,
        n2t_detail=n2t_detail,
        java_wall_s=truth.wall_time_s,
        n2t_wall_s=n2t.wall_time_s,
        out_matched=out_matched,
        out_diff_preview=out_diff_preview,
        notes=truth.notes,
        mutation_kind=truth.mutation_kind,
    )


def format_console_table(
    comparisons: list[CaseComparison],
    skipped: list[SkippedCase],
) -> str:
    """Render a readable summary table grouped by (scenario, project)."""
    groups: dict[tuple[str, str], list[CaseComparison]] = defaultdict(list)
    for c in comparisons:
        groups[(c.scenario, c.project)].append(c)

    lines: list[str] = []
    header = (
        f"{'Scenario':<10} {'Project':<12} {'Total':>5} {'Match':>5} "
        f"{'ExpDiff':>7} {'Mismatch':>8} {'NoSupp':>6}"
    )
    lines.append(header)
    lines.append("-" * len(header))

    totals = [0, 0, 0, 0, 0]
    for (scenario, project) in sorted(groups.keys()):
        items = groups[(scenario, project)]
        t = len(items)
        m = sum(1 for x in items if x.status == "match")
        e = sum(1 for x in items if x.status == "expected-diff")
        f = sum(1 for x in items if x.status == "mismatch")
        u = sum(1 for x in items if x.status == "not-supported")
        totals[0] += t
        totals[1] += m
        totals[2] += e
        totals[3] += f
        totals[4] += u
        lines.append(
            f"{scenario:<10} {project:<12} {t:>5} {m:>5} {e:>7} {f:>8} {u:>6}"
        )

    lines.append("-" * len(header))
    lines.append(
        f"{'TOTAL':<10} {'':<12} {totals[0]:>5} {totals[1]:>5} "
        f"{totals[2]:>7} {totals[3]:>8} {totals[4]:>6}"
    )
    if skipped:
        lines.append(f"Skipped (missing student artifacts): {len(skipped)}")

    mismatches = [c for c in comparisons if c.status == "mismatch"]
    if mismatches:
        lines.append("")
        lines.append(f"Unexplained mismatches ({len(mismatches)}):")
        for c in mismatches[:40]:
            diff_str = "; ".join(c.differences)
            lines.append(f"  - {c.case_id}: {diff_str}")
        if len(mismatches) > 40:
            lines.append(f"  ... and {len(mismatches) - 40} more (see report file)")

    return "\n".join(lines)


def write_reports(
    private_dir: Path,
    comparisons: list[CaseComparison],
    skipped: list[SkippedCase],
    *,
    n2t_mode: str,
    seed: int,
) -> tuple[Path, Path]:
    """Write Markdown and JSON conformance reports under N2T_PRIVATE_DIR/reports/."""
    reports_dir = private_dir / "reports"
    reports_dir.mkdir(parents=True, exist_ok=True)
    today = dt.date.today().isoformat()
    md_path = reports_dir / f"conformance-{today}.md"
    json_path = reports_dir / f"conformance-{today}.json"

    summary = {
        "date": today,
        "n2t_adapter_mode": n2t_mode,
        "mutation_seed": seed,
        "total": len(comparisons),
        "match": sum(1 for c in comparisons if c.status == "match"),
        "expected_diff": sum(1 for c in comparisons if c.status == "expected-diff"),
        "mismatch": sum(1 for c in comparisons if c.status == "mismatch"),
        "not_supported": sum(1 for c in comparisons if c.status == "not-supported"),
        "skipped": len(skipped),
    }

    payload = {
        "summary": summary,
        "cases": [c.to_dict() for c in comparisons],
        "skipped": [asdict(s) for s in skipped],
    }
    json_path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")

    # Build Markdown report (no course file contents or solutions)
    md: list[str] = [
        f"# Nandwright Conformance Report ({today})",
        "",
        f"- Adapter mode: `{n2t_mode}`",
        f"- Mutation seed: `{seed}`",
        f"- Total cases: **{summary['total']}** "
        f"(match: **{summary['match']}**, expected-diff: **{summary['expected_diff']}**, "
        f"mismatch: **{summary['mismatch']}**, not-supported: **{summary['not_supported']}**)",
        f"- Skipped (needs student artifact): **{summary['skipped']}**",
        "",
        "## Summary by Scenario & Project",
        "",
        "| Scenario | Project | Total | Match | ExpDiff | Mismatch | Not Supported |",
        "|---|---|---:|---:|---:|---:|---:|",
    ]
    groups: dict[tuple[str, str], list[CaseComparison]] = defaultdict(list)
    for c in comparisons:
        groups[(c.scenario, c.project)].append(c)
    for (scenario, project) in sorted(groups.keys()):
        items = groups[(scenario, project)]
        md.append(
            f"| {scenario} | {project} | {len(items)} | "
            f"{sum(1 for x in items if x.status == 'match')} | "
            f"{sum(1 for x in items if x.status == 'expected-diff')} | "
            f"{sum(1 for x in items if x.status == 'mismatch')} | "
            f"{sum(1 for x in items if x.status == 'not-supported')} |"
        )

    mismatches = [c for c in comparisons if c.status == "mismatch"]
    if mismatches:
        md.extend([
            "",
            f"## Unexplained Mismatches ({len(mismatches)})",
            "",
            "| Case | Dialect | Java | n2t | Difference |",
            "|---|---|---|---|---|",
        ])
        for c in mismatches:
            diff_txt = "; ".join(c.differences).replace("|", "\\|")
            j_det = c.java_detail.replace("|", "\\|")
            n_det = c.n2t_detail.replace("|", "\\|")
            md.append(
                f"| `{c.case_id}` | {c.dialect} | {c.java_outcome} (`{j_det}`) "
                f"| {c.n2t_outcome} (`{n_det}`) | {diff_txt} |"
            )

    exp_diffs = [c for c in comparisons if c.status == "expected-diff"]
    if exp_diffs:
        md.extend([
            "",
            f"## Expected Differences ({len(exp_diffs)})",
            "",
            "| Case | Java | n2t | Difference | Reason |",
            "|---|---|---|---|---|",
        ])
        for c in exp_diffs:
            diff_txt = "; ".join(c.differences).replace("|", "\\|")
            reason = (c.expected_reason or "").replace("|", "\\|")
            md.append(
                f"| `{c.case_id}` | {c.java_outcome} | {c.n2t_outcome} | {diff_txt} | {reason} |"
            )

    errors = [c for c in comparisons if c.java_outcome.startswith("error") and c.status == "match"]
    if errors:
        md.extend([
            "",
            f"## Matched Errors Side-by-Side ({len(errors)})",
            "",
            "| Case | Java Error | n2t Error |",
            "|---|---|---|",
        ])
        for c in errors:
            j_det = c.java_detail.replace("|", "\\|")
            n_det = c.n2t_detail.replace("|", "\\|")
            md.append(f"| `{c.case_id}` | `{j_det}` | `{n_det}` |")

    if skipped:
        md.extend([
            "",
            f"## Skipped Scripts ({len(skipped)})",
            "",
            "| Case | Dialect | Reason |",
            "|---|---|---|",
        ])
        for s in skipped:
            md.append(f"| `{s.case_id}` | {s.dialect} | {s.reason} |")

    md_path.write_text("\n".join(md) + "\n", encoding="utf-8")
    return md_path, json_path
