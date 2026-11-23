"""Deterministic HDL mutation generator for Nandwright conformance testing.

Generates single-mutation variants of solved chips at runtime from N2T_PRIVATE_DIR.
No course material or solutions are embedded in this module.
"""

from __future__ import annotations

import random
import re
from dataclasses import dataclass
from pathlib import Path

COMMENT_RE = re.compile(r"//[^\n]*|/\*.*?\*/", re.DOTALL)
PART_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(([^;()]*)\)\s*;", re.DOTALL)
PIN_BIND_RE = re.compile(
    r"([A-Za-z_][A-Za-z0-9_]*(?:\[\d+(?:\.\.\d+)?\])?)\s*=\s*"
    r"([A-Za-z_][A-Za-z0-9_]*(?:\[\d+(?:\.\.\d+)?\])?)"
)
SLICE_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\[(\d+)(?:\.\.(\d+))?\]$")

# Replacement gates likely to preserve syntax/width (causing comparison failures)
# or trigger width/pin errors depending on chip.
GATE_REPLACEMENTS: dict[str, list[str]] = {
    "Nand": ["And", "Or", "Xor"],
    "And": ["Or", "Nand", "Xor"],
    "Or": ["And", "Nand", "Xor"],
    "Xor": ["Or", "And", "Nand"],
    "Not": ["DFF"],
    "Mux": ["And", "Or"],
    "DMux": ["And"],
    "And16": ["Or16", "Add16"],
    "Or16": ["And16", "Add16"],
    "Not16": ["Inc16"],
    "Add16": ["And16", "Or16"],
    "Inc16": ["Not16"],
    "Mux16": ["And16", "Or16"],
    "HalfAdder": ["And", "Xor"],
    "FullAdder": ["HalfAdder"],
    "Bit": ["DFF"],
    "Register": ["PC", "Inc16"],
    "RAM8": ["RAM64"],
    "RAM64": ["RAM8", "RAM512"],
    "RAM512": ["RAM64", "RAM4K"],
    "RAM4K": ["RAM512", "RAM16K"],
    "DMux4Way": ["DMux8Way"],
    "DMux8Way": ["DMux4Way"],
    "Mux4Way16": ["Mux8Way16"],
    "Mux8Way16": ["Mux4Way16"],
}

WIDTH_SWAP: dict[str, str] = {
    "Not": "Not16",
    "And": "And16",
    "Or": "Or16",
    "Mux": "Mux16",
    "Not16": "Not",
    "And16": "And",
    "Or16": "Or",
    "Mux16": "Mux",
    "Add16": "HalfAdder",
    "Inc16": "Not",
    "Register": "Bit",
    "Bit": "Register",
    "RAM8": "Register",
    "RAM64": "RAM8",
    "RAM512": "RAM64",
    "RAM4K": "RAM512",
    "RAM16K": "RAM4K",
}

# Symmetric input pin pairs whose swap is a no-op; avoid swapping only these.
SYMMETRIC_PAIRS: dict[str, set[frozenset[str]]] = {
    "Nand": {frozenset({"a", "b"})},
    "And": {frozenset({"a", "b"})},
    "Or": {frozenset({"a", "b"})},
    "Xor": {frozenset({"a", "b"})},
    "And16": {frozenset({"a", "b"})},
    "Or16": {frozenset({"a", "b"})},
    "Add16": {frozenset({"a", "b"})},
    "HalfAdder": {frozenset({"a", "b"})},
    "FullAdder": {frozenset({"a", "b"}), frozenset({"a", "c"}), frozenset({"b", "c"})},
}

MUTATION_KINDS = (
    "swap_pins",
    "replace_gate",
    "drop_part",
    "change_slice",
    "wrong_const",
    "undriven_internal",
    "comb_loop",
    "wrong_width",
    "misspelled_name",
)


@dataclass(frozen=True)
class Mutation:
    mut_id: str
    kind: str
    project: str
    chip_name: str
    rel_hdl: str
    rel_tst: str
    description: str
    mutated_hdl: str


def _strip_comments_preserve_len(text: str) -> str:
    """Replace comments with spaces/newlines preserving exact byte/char offsets."""
    def repl(m: re.Match[str]) -> str:
        return "".join("\n" if ch == "\n" else " " for ch in m.group(0))
    return COMMENT_RE.sub(repl, text)


def _find_parts_section(clean: str) -> tuple[int, int] | None:
    m = re.search(r"\bPARTS\s*:", clean)
    if not m:
        return None
    end = clean.rfind("}")
    if end == -1 or end <= m.end():
        return None
    return m.end(), end


def _parse_io_pins(clean: str) -> tuple[dict[str, int], dict[str, int]]:
    inputs: dict[str, int] = {}
    outputs: dict[str, int] = {}
    for kind, target in (("IN", inputs), ("OUT", outputs)):
        m = re.search(rf"\b{kind}\b(.*?);", clean, re.DOTALL)
        if not m:
            continue
        for item in m.group(1).split(","):
            item = item.strip()
            if not item:
                continue
            pm = re.match(r"^([A-Za-z_][A-Za-z0-9_]*)(?:\[(\d+)\])?$", item)
            if pm:
                target[pm.group(1)] = int(pm.group(2)) if pm.group(2) else 1
    return inputs, outputs


def _parse_parts(raw: str, clean: str) -> list[tuple[int, int, str, list[tuple[int, int, str, str]]]]:
    """Return list of (start, end, chip_name, [(bind_start, bind_end, lhs, rhs), ...])."""
    bounds = _find_parts_section(clean)
    if not bounds:
        return []
    p_start, p_end = bounds
    parts = []
    for pm in PART_RE.finditer(clean, p_start, p_end):
        chip = pm.group(1)
        body_start = pm.start(2)
        body = pm.group(2)
        bindings = []
        for bm in PIN_BIND_RE.finditer(body):
            bindings.append((
                body_start + bm.start(),
                body_start + bm.end(),
                bm.group(1),
                bm.group(2),
            ))
        parts.append((pm.start(), pm.end(), chip, bindings))
    return parts


def _base_name(pin: str) -> str:
    return pin.split("[", 1)[0]


def _replace_span(text: str, start: int, end: int, repl: str) -> str:
    return text[:start] + repl + text[end:]


def _candidates_for_chip(
    project: str,
    chip_name: str,
    rel_hdl: str,
    rel_tst: str,
    raw_hdl: str,
) -> dict[str, list[tuple[str, str]]]:
    """Return dict[kind -> list[(description, mutated_hdl)]] for one chip."""
    clean = _strip_comments_preserve_len(raw_hdl)
    bounds = _find_parts_section(clean)
    if not bounds:
        return {}
    p_start, p_end = bounds
    inputs, outputs = _parse_io_pins(clean)
    parts = _parse_parts(raw_hdl, clean)
    if not parts:
        return {}

    io_names = set(inputs) | set(outputs) | {"true", "false"}
    out: dict[str, list[tuple[str, str]]] = {k: [] for k in MUTATION_KINDS}

    # 1. swap_pins
    for _, _, pchip, bindings in parts:
        sym = SYMMETRIC_PAIRS.get(pchip, set())
        for i in range(len(bindings)):
            for j in range(i + 1, len(bindings)):
                s1, e1, l1, r1 = bindings[i]
                s2, e2, l2, r2 = bindings[j]
                if r1 == r2:
                    continue
                if frozenset({_base_name(l1), _base_name(l2)}) in sym:
                    continue
                # Swap RHS values between bindings i and j
                mut = raw_hdl[:s1] + f"{l1}={r2}" + raw_hdl[e1:s2] + f"{l2}={r1}" + raw_hdl[e2:]
                out["swap_pins"].append((f"{pchip}: swap {l1}={r1} <-> {l2}={r2}", mut))
                if len(out["swap_pins"]) >= 8:
                    break
            if len(out["swap_pins"]) >= 8:
                break

    # 2. replace_gate
    for start, _, pchip, _ in parts:
        for repl_chip in GATE_REPLACEMENTS.get(pchip, []):
            mut = _replace_span(raw_hdl, start, start + len(pchip), repl_chip)
            out["replace_gate"].append((f"replace {pchip} with {repl_chip}", mut))

    # 3. drop_part
    for idx, (start, end, pchip, _) in enumerate(parts):
        # Remove the whole part statement
        mut = _replace_span(raw_hdl, start, end, "")
        out["drop_part"].append((f"drop part #{idx + 1} ({pchip})", mut))

    # 4. change_slice
    for _, _, pchip, bindings in parts:
        for s, e, lhs, rhs in bindings:
            for side_name, is_lhs in ((lhs, True), (rhs, False)):
                sm = SLICE_RE.match(side_name)
                if not sm:
                    continue
                base = sm.group(1)
                lo = int(sm.group(2))
                hi = int(sm.group(3)) if sm.group(3) is not None else None
                variants: list[str] = []
                if hi is None:
                    # Single bit index: shift by +1 or -1, or push out of bounds
                    variants.append(f"{base}[{lo + 1}]")
                    if lo > 0:
                        variants.append(f"{base}[{lo - 1}]")
                    variants.append(f"{base}[16]")
                else:
                    width = hi - lo + 1
                    if width > 1:
                        variants.append(f"{base}[{lo}..{hi - 1}]")
                    variants.append(f"{base}[{lo + 1}..{hi + 1}]")
                    variants.append(f"{base}[{lo}..{hi + 2}]")
                for var in variants:
                    new_bind = f"{var}={rhs}" if is_lhs else f"{lhs}={var}"
                    mut = _replace_span(raw_hdl, s, e, new_bind)
                    out["change_slice"].append((f"{pchip}: {side_name} -> {var}", mut))

    # 5. wrong_const
    for _, _, pchip, bindings in parts:
        for s, e, lhs, rhs in bindings:
            if rhs == "true":
                mut = _replace_span(raw_hdl, s, e, f"{lhs}=false")
                out["wrong_const"].append((f"{pchip}: {lhs}=true -> false", mut))
            elif rhs == "false":
                mut = _replace_span(raw_hdl, s, e, f"{lhs}=true")
                out["wrong_const"].append((f"{pchip}: {lhs}=false -> true", mut))
            elif _base_name(lhs) not in {"out", "sum", "carry", "zr", "ng"} and _base_name(rhs) not in outputs:
                for const in ("true", "false"):
                    mut = _replace_span(raw_hdl, s, e, f"{lhs}={const}")
                    out["wrong_const"].append((f"{pchip}: {lhs}={rhs} -> {const}", mut))

    # 6. undriven_internal
    # Find internal wires (not in IN/OUT/true/false) used as RHS on non-output formal pins
    for _, _, pchip, bindings in parts:
        for s, e, lhs, rhs in bindings:
            rb = _base_name(rhs)
            lb = _base_name(lhs)
            if lb in {"out", "sum", "carry", "zr", "ng"}:
                continue
            if rb not in io_names:
                mut = _replace_span(raw_hdl, s, e, f"{lhs}=nwUndrivenPin")
                out["undriven_internal"].append((f"{pchip}: {lhs}={rhs} -> undriven nwUndrivenPin", mut))
    # Fallback for chips with no internal wires (e.g. Not): replace an input read with undriven wire
    if not out["undriven_internal"]:
        for _, _, pchip, bindings in parts:
            for s, e, lhs, rhs in bindings:
                if _base_name(lhs) not in {"out", "sum", "carry", "zr", "ng"}:
                    mut = _replace_span(raw_hdl, s, e, f"{lhs}=nwUndrivenPin")
                    out["undriven_internal"].append((f"{pchip}: {lhs}={rhs} -> undriven nwUndrivenPin", mut))
                    break
            if out["undriven_internal"]:
                break

    # 7. comb_loop
    loop_snippets = [
        ("inject 2-gate Not combinational loop", "\n    Not(in=nwLoopA, out=nwLoopB);\n    Not(in=nwLoopB, out=nwLoopA);\n"),
        ("inject self-feedback Nand loop", "\n    Nand(a=nwLoopSelf, b=nwLoopSelf, out=nwLoopSelf);\n"),
    ]
    for desc, snippet in loop_snippets:
        mut = raw_hdl[:p_end] + snippet + raw_hdl[p_end:]
        out["comb_loop"].append((desc, mut))

    # 8. wrong_width
    for start, _, pchip, bindings in parts:
        swapped = WIDTH_SWAP.get(pchip)
        if swapped:
            mut = _replace_span(raw_hdl, start, start + len(pchip), swapped)
            out["wrong_width"].append((f"width swap {pchip} -> {swapped}", mut))
        for s, e, lhs, rhs in bindings:
            rb = _base_name(rhs)
            if rb in inputs and "[" not in rhs:
                if inputs[rb] == 16:
                    mut = _replace_span(raw_hdl, s, e, f"{lhs}={rb}[0..7]")
                    out["wrong_width"].append((f"{pchip}: narrow {rhs} to {rb}[0..7]", mut))
                elif inputs[rb] == 1:
                    mut = _replace_span(raw_hdl, s, e, f"{lhs}={rb}[0..1]")
                    out["wrong_width"].append((f"{pchip}: invalid width slice {rb}[0..1]", mut))
                    break

    # 9. misspelled_name
    for start, _, pchip, bindings in parts:
        mut_chip = _replace_span(raw_hdl, start, start + len(pchip), f"{pchip}Typo")
        out["misspelled_name"].append((f"misspell chip {pchip} -> {pchip}Typo", mut_chip))
        if bindings:
            s, e, lhs, rhs = bindings[0]
            lb = _base_name(lhs)
            bad_lhs = lhs.replace(lb, f"{lb}Typo", 1)
            mut_pin = _replace_span(raw_hdl, s, e, f"{bad_lhs}={rhs}")
            out["misspelled_name"].append((f"misspell pin {pchip}.{lhs} -> {bad_lhs}", mut_pin))

    # Deduplicate mutated texts within each kind while preserving order
    for kind in list(out.keys()):
        seen: set[str] = set()
        deduped: list[tuple[str, str]] = []
        for desc, text in out[kind]:
            if text != raw_hdl and text not in seen:
                seen.add(text)
                deduped.append((desc, text))
        out[kind] = deduped
    return out


def generate_mutations(
    course_projects_dir: Path,
    solutions_dir: Path,
    *,
    seed: int = 20261123,
    target_count: int = 180,
) -> list[Mutation]:
    """Deterministically generate single-mutation HDL variants from solved chips.

    Excludes Memory.hdl (whose official Memory.tst waits interactively on Keyboard).
    """
    rng = random.Random(seed)

    # Discover all solved hardware chips that have an official non-interactive .tst
    chip_targets: list[tuple[str, str, str, str, Path]] = []
    for hdl_path in sorted(solutions_dir.rglob("*.hdl")):
        rel = hdl_path.relative_to(solutions_dir)
        chip_name = hdl_path.stem
        if chip_name == "Memory":
            # Memory.tst blocks forever on Java headless keyboard wait loop
            continue
        project = rel.parts[0]
        if project == "03" and len(rel.parts) >= 2:
            project = f"03/{rel.parts[1]}"
        # Pick official .tst for this chip
        tst_candidates = [
            course_projects_dir / rel.with_suffix(".tst"),
        ]
        if chip_name == "Computer":
            tst_candidates = [course_projects_dir / "05/ComputerAdd.tst"]
        rel_tst = None
        for cand in tst_candidates:
            if cand.exists():
                rel_tst = str(cand.relative_to(course_projects_dir))
                break
        if rel_tst is None:
            continue
        chip_targets.append((project, chip_name, str(rel), rel_tst, hdl_path))

    # Collect pools by mutation kind
    pools: dict[str, list[tuple[str, str, str, str, str, str]]] = {k: [] for k in MUTATION_KINDS}
    for project, chip_name, rel_hdl, rel_tst, hdl_path in chip_targets:
        raw_hdl = hdl_path.read_text(encoding="utf-8", errors="replace")
        by_kind = _candidates_for_chip(project, chip_name, rel_hdl, rel_tst, raw_hdl)
        for kind, items in by_kind.items():
            for desc, mutated in items:
                pools[kind].append((project, chip_name, rel_hdl, rel_tst, desc, mutated))

    # Shuffle each pool deterministically, spreading across chips (round-robin by chip)
    balanced_pools: dict[str, list[tuple[str, str, str, str, str, str]]] = {}
    for kind, items in pools.items():
        by_chip: dict[str, list[tuple[str, str, str, str, str, str]]] = {}
        for item in items:
            by_chip.setdefault(item[1], []).append(item)
        chip_keys = sorted(by_chip.keys())
        rng.shuffle(chip_keys)
        for ck in chip_keys:
            rng.shuffle(by_chip[ck])
        rr: list[tuple[str, str, str, str, str, str]] = []
        while True:
            progressed = False
            for ck in chip_keys:
                if by_chip[ck]:
                    rr.append(by_chip[ck].pop())
                    progressed = True
            if not progressed:
                break
        balanced_pools[kind] = rr

    # Select balanced quota per kind until target_count reached
    per_kind_base = max(1, target_count // len(MUTATION_KINDS))
    selected: list[tuple[str, tuple[str, str, str, str, str, str]]] = []
    used_per_kind: dict[str, int] = {k: 0 for k in MUTATION_KINDS}

    for kind in MUTATION_KINDS:
        take = min(per_kind_base, len(balanced_pools[kind]))
        for item in balanced_pools[kind][:take]:
            selected.append((kind, item))
        used_per_kind[kind] = take

    # Top up round-robin from remaining pools if needed
    while len(selected) < target_count:
        progressed = False
        for kind in MUTATION_KINDS:
            idx = used_per_kind[kind]
            if idx < len(balanced_pools[kind]) and len(selected) < target_count:
                selected.append((kind, balanced_pools[kind][idx]))
                used_per_kind[kind] = idx + 1
                progressed = True
        if not progressed:
            break

    # Sort deterministically by (kind, chip_name, counter) and assign stable IDs
    mutations: list[Mutation] = []
    counters: dict[tuple[str, str], int] = {}
    for kind, (project, chip_name, rel_hdl, rel_tst, desc, mutated) in selected:
        key = (kind, chip_name)
        counters[key] = counters.get(key, 0) + 1
        mut_id = f"m_{kind}_{chip_name}_{counters[key]:02d}"
        mutations.append(
            Mutation(
                mut_id=mut_id,
                kind=kind,
                project=project,
                chip_name=chip_name,
                rel_hdl=rel_hdl,
                rel_tst=rel_tst,
                description=desc,
                mutated_hdl=mutated,
            )
        )
    return mutations
