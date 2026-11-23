#!/usr/bin/env python3
"""Nandwright conformance harness: diff `n2t` against official Java Nand2Tetris tools.

Standard library only (Python 3.12+). Reads course files from N2T_COURSE_DIR
(default ~/n2t-private/course) and private solutions/truth/stage/reports from
N2T_PRIVATE_DIR (default ~/n2t-private). Exits cleanly with code 0 when
N2T_COURSE_DIR is absent (e.g. in CI).
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import shutil
import sys
import tempfile
from pathlib import Path

# Ensure sibling modules under conformance/ are importable regardless of cwd
HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))

from compare import (  # noqa: E402
    CaseComparison,
    compare_case,
    format_console_table,
    load_expected_differences,
    write_reports,
)
from java_runner import JavaTruth, load_or_capture_truth  # noqa: E402
from n2t_adapter import N2tResult, run_n2t, supports_json_mode  # noqa: E402
from scenarios import (  # noqa: E402
    SkippedCase,
    TestCase,
    build_broken,
    build_dialects,
    build_solved,
    build_untouched,
)

ALL_SCENARIOS = ("solved", "untouched", "broken", "dialects")
MAX_WORKERS = 3


def _default_n2t_bin() -> Path | None:
    env_bin = os.environ.get("N2T_BIN")
    if env_bin:
        return Path(env_bin).expanduser()
    repo_root = HERE.parent
    for rel in ("target/release/n2t", "target/debug/n2t"):
        cand = repo_root / rel
        if cand.exists():
            return cand
    found = shutil.which("n2t")
    return Path(found) if found else None


def _parse_scenarios(values: list[str] | None) -> list[str]:
    if not values:
        return list(ALL_SCENARIOS)
    out: list[str] = []
    for v in values:
        for part in v.split(","):
            part = part.strip()
            if part == "all":
                return list(ALL_SCENARIOS)
            if part not in ALL_SCENARIOS:
                raise SystemExit(f"error: unknown scenario '{part}' (choose from {', '.join(ALL_SCENARIOS)})")
            if part not in out:
                out.append(part)
    return out


def _matches_project(case_project: str, proj_filter: str | None) -> bool:
    if not proj_filter:
        return True
    pf = proj_filter.strip("/")
    return (
        case_project == pf
        or case_project.startswith(f"{pf}/")
        or case_project == f"mut/{pf}"
        or case_project.startswith(f"mut/{pf}/")
    )


def _matches_file(case: TestCase, file_filter: str | None) -> bool:
    if not file_filter:
        return True
    return (
        file_filter in case.case_id
        or file_filter in case.rel_script
        or Path(case.rel_script).name == file_filter
    )


def _run_n2t_isolated(
    case: TestCase,
    n2t_bin: Path,
    timeout_s: float,
) -> N2tResult:
    """Run `n2t` on a fresh isolated copy of the staged case directory."""
    max_commands = 50_000 if case.needs_keyboard else 10_000_000
    with tempfile.TemporaryDirectory(prefix="n2t_run_") as tmp:
        work_dir = Path(tmp) / "work"
        shutil.copytree(case.stage_dir, work_dir)
        script_path = work_dir / case.rel_script
        return run_n2t(
            script_path,
            n2t_bin=n2t_bin,
            dialect=case.dialect,
            out_file_name=case.out_file_name,
            timeout_s=timeout_s,
            max_commands=max_commands,
        )


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Run Nandwright `n2t` conformance checks against official Java tools."
    )
    p.add_argument(
        "--scenario",
        action="append",
        help="Scenario(s) to run: all, solved, untouched, broken, dialects (repeatable or comma-separated).",
    )
    p.add_argument(
        "--project",
        help="Filter to one project (e.g. 01, 02, 03, 03/a, 04, 05, 07, 08, 12, demo, exp2).",
    )
    p.add_argument(
        "--file",
        help="Filter to scripts matching this substring or filename (e.g. ALU.tst, c05_subbus_internal).",
    )
    p.add_argument(
        "--refresh-truth",
        action="store_true",
        help="Re-run official Java tools and overwrite cached truth records for selected cases.",
    )
    p.add_argument(
        "--capture-only",
        action="store_true",
        help="Only stage workspaces and capture/refresh Java ground truth; skip running n2t.",
    )
    p.add_argument(
        "--jobs",
        type=int,
        default=MAX_WORKERS,
        help=f"Parallel worker processes (default and hard cap: {MAX_WORKERS}).",
    )
    p.add_argument(
        "--n2t",
        type=Path,
        default=None,
        help="Path to the n2t CLI binary (default: N2T_BIN or target/release/n2t).",
    )
    p.add_argument(
        "--seed",
        type=int,
        default=20261123,
        help="Deterministic RNG seed for mutation generation (default: 20261123).",
    )
    p.add_argument(
        "--mutations",
        type=int,
        default=180,
        help="Target number of generated single-mutation HDL cases (default: 180).",
    )
    p.add_argument(
        "--timeout",
        type=float,
        default=20.0,
        help="Per-script execution timeout in seconds (default: 20.0).",
    )
    p.add_argument(
        "--no-report",
        action="store_true",
        help="Skip writing reports under N2T_PRIVATE_DIR/reports/.",
    )
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    course_dir = Path(os.environ.get("N2T_COURSE_DIR", "~/n2t-private/course")).expanduser()
    private_dir = Path(os.environ.get("N2T_PRIVATE_DIR", "~/n2t-private")).expanduser()

    if not course_dir.exists():
        print(f"skipped: N2T_COURSE_DIR not found ({course_dir})")
        return 0

    scenarios = _parse_scenarios(args.scenario)
    workers = min(max(1, args.jobs), MAX_WORKERS)

    # 1. Build staged workspaces and collect test cases
    all_cases: list[TestCase] = []
    all_skipped: list[SkippedCase] = []

    if "solved" in scenarios:
        all_cases.extend(build_solved(course_dir, private_dir))
    if "untouched" in scenarios:
        all_cases.extend(build_untouched(course_dir, private_dir))
    if "broken" in scenarios:
        broken_cases, _ = build_broken(
            course_dir, private_dir, seed=args.seed, mutation_count=args.mutations
        )
        all_cases.extend(broken_cases)
    if "dialects" in scenarios:
        d_cases, d_skipped = build_dialects(course_dir, private_dir)
        all_cases.extend(d_cases)
        all_skipped.extend(d_skipped)

    cases = [
        c
        for c in all_cases
        if _matches_project(c.project, args.project) and _matches_file(c, args.file)
    ]
    skipped = [
        s
        for s in all_skipped
        if _matches_project(s.project, args.project)
        and (not args.file or args.file in s.case_id or args.file in s.rel_script)
    ]

    if not cases:
        print("No matching test cases selected.")
        return 0

    # 2. Capture or load cached Java ground truth (capped at MAX_WORKERS)
    truths: dict[str, JavaTruth] = {}
    captured_fresh = 0
    loaded_cached = 0

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        future_map = {
            pool.submit(
                load_or_capture_truth,
                case,
                course_dir,
                private_dir,
                refresh=args.refresh_truth,
                timeout_s=args.timeout,
            ): case
            for case in cases
        }
        for fut in concurrent.futures.as_completed(future_map):
            case = future_map[fut]
            truth, was_cached = fut.result()
            truths[case.case_id] = truth
            if was_cached:
                loaded_cached += 1
            else:
                captured_fresh += 1

    # Write truth manifest summary
    truth_dir = private_dir / "truth"
    truth_dir.mkdir(parents=True, exist_ok=True)
    manifest = {
        "scenarios": scenarios,
        "seed": args.seed,
        "cases": len(cases),
        "captured_fresh": captured_fresh,
        "loaded_cached": loaded_cached,
        "outcome_counts": {
            k: sum(1 for t in truths.values() if t.outcome == k)
            for k in ("passed", "comparison-failure", "error", "timeout")
        },
    }
    (truth_dir / "manifest.json").write_text(
        json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
    )

    print(
        f"Java truth ready: {len(cases)} cases "
        f"({captured_fresh} captured, {loaded_cached} cached; "
        f"passed={manifest['outcome_counts']['passed']}, "
        f"cmp-fail={manifest['outcome_counts']['comparison-failure']}, "
        f"error={manifest['outcome_counts']['error']}, "
        f"timeout={manifest['outcome_counts']['timeout']})"
    )

    if args.capture_only:
        return 0

    # 3. Locate n2t binary
    n2t_bin = args.n2t.expanduser() if args.n2t else _default_n2t_bin()
    if n2t_bin is None or not n2t_bin.exists():
        print(
            "error: `n2t` binary not found. Build it with "
            "`cargo build --release -p n2t-cli -j 3` or pass --n2t.",
            file=sys.stderr,
        )
        return 2

    adapter_mode = "json" if supports_json_mode(str(n2t_bin)) else "human"
    expected_diffs = load_expected_differences(HERE / "expected-differences.toml")

    # 4. Run n2t on isolated copies of the same staged workspaces
    n2t_results: dict[str, N2tResult] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        future_map = {
            pool.submit(_run_n2t_isolated, case, n2t_bin, args.timeout): case
            for case in cases
        }
        for fut in concurrent.futures.as_completed(future_map):
            case = future_map[fut]
            n2t_results[case.case_id] = fut.result()

    # 5. Compare and report
    comparisons: list[CaseComparison] = [
        compare_case(truths[c.case_id], n2t_results[c.case_id], expected_diffs)
        for c in cases
    ]

    print(f"n2t binary: {n2t_bin} (adapter mode: {adapter_mode})")
    print()
    print(format_console_table(comparisons, skipped))

    if not args.no_report:
        md_path, json_path = write_reports(
            private_dir, comparisons, skipped, n2t_mode=adapter_mode, seed=args.seed
        )
        print()
        print(f"Reports written:\n  {md_path}\n  {json_path}")

    unexplained = sum(1 for c in comparisons if c.status == "mismatch")
    return 1 if unexplained > 0 else 0


if __name__ == "__main__":
    raise SystemExit(main())
