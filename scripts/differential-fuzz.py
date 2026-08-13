#!/usr/bin/env python3
"""Deterministically compare Ninja and Knight across generated small DAGs."""

from __future__ import annotations

import argparse
import concurrent.futures
import os
import pathlib
import random
import re
import subprocess
import tempfile


def manifest_for(seed: int) -> tuple[str, str]:
    rng = random.Random(seed)
    shell = "cmd /d /c " if os.name == "nt" else ""
    lines = [
        "rule make",
        f"  command = {shell}echo $in > $out",
        "rule stamp",
        f"  command = {shell}echo stamp > $out",
        "pool serial",
        "  depth = 1",
        "pool unlimited",
        "  depth = 0",
    ]
    available = [f"src/{index}" for index in range(rng.randint(2, 6))]
    produced: list[str] = []
    for edge in range(rng.randint(4, 14)):
        outputs = [f"out/{edge}"]
        if rng.random() < 0.2:
            outputs.append(f"out/{edge}.imp")
        candidates = available[:]
        rng.shuffle(candidates)
        explicit_count = rng.randint(0, min(3, len(candidates)))
        explicit = candidates[:explicit_count]
        candidates = candidates[explicit_count:]
        implicit_count = rng.randint(0, min(2, len(candidates)))
        implicit = candidates[:implicit_count]
        candidates = candidates[implicit_count:]
        order_count = rng.randint(0, min(2, len(candidates)))
        order_only = candidates[:order_count]
        candidates = candidates[order_count:]
        validation_count = rng.randint(0, min(1, len(candidates)))
        validations = candidates[:validation_count]

        phony = rng.random() < 0.25
        rule = "phony" if phony else ("stamp" if not explicit and not implicit else "make")
        output_text = outputs[0]
        if len(outputs) > 1:
            output_text += " | " + " ".join(outputs[1:])
        build = f"build {output_text}: {rule}"
        if explicit:
            build += " " + " ".join(explicit)
        if implicit:
            build += " | " + " ".join(implicit)
        if order_only:
            build += " || " + " ".join(order_only)
        if validations:
            build += " |@ " + " ".join(validations)
        lines.append(build)
        if not phony and rng.random() < 0.25:
            lines.append("  pool = " + rng.choice(["serial", "unlimited"]))
        produced.extend(outputs)
        available.extend(outputs)

    roots = produced[-rng.randint(1, min(4, len(produced))):]
    lines.append("build all: phony " + " ".join(roots))
    lines.append("default all")
    return "\n".join(lines) + "\n", rng.choice(roots)


def run(executable: pathlib.Path, cwd: pathlib.Path, args: list[str]) -> tuple[int, str, str]:
    result = subprocess.run(
        [str(executable), *args], cwd=cwd, capture_output=True, text=True
    )
    return result.returncode, result.stdout.replace("\r\n", "\n"), result.stderr.replace("\r\n", "\n")


def normalize_error(value: str) -> str:
    return value.replace("ninja:", "tool:").replace("knight:", "tool:")


def normalize_output(value: str) -> str:
    value = value.replace("ninja:", "tool:").replace("knight:", "tool:")
    if not value.startswith("digraph ninja {"):
        return value
    identities: dict[str, str] = {}
    rule = 0
    definition = re.compile(
        r'^"((?:\\.|[^"])*)" \[label="((?:\\.|[^"])*)"(, shape=ellipse)?\]$'
    )
    for line in value.splitlines():
        match = definition.match(line)
        if not match:
            continue
        identifier, label, ellipse = match.groups()
        if ellipse:
            identities[identifier] = f"RULE:{rule}"
            rule += 1
        else:
            identities[identifier] = f"NODE:{label}"

    graph_edge = re.compile(
        r'^"((?:\\.|[^"])*)" -> "((?:\\.|[^"])*)"(.*)$'
    )
    normalized = []
    anonymous = 0

    def identity(identifier: str) -> str:
        nonlocal anonymous
        if identifier not in identities:
            identities[identifier] = f"ANON:{anonymous}"
            anonymous += 1
        return identities[identifier]

    for line in value.splitlines():
        match = definition.match(line)
        if match:
            identifier, label, ellipse = match.groups()
            suffix = ", shape=ellipse" if ellipse else ""
            normalized.append(
                f'"{identities[identifier]}" [label="{label}"{suffix}]'
            )
            continue
        match = graph_edge.match(line)
        if match:
            source, destination, suffix = match.groups()
            normalized.append(
                f'"{identity(source)}" -> "{identity(destination)}"{suffix}'
            )
            continue
        normalized.append(line)
    return "\n".join(normalized) + "\n"


def comparable(result: tuple[int, str, str]) -> tuple[object, ...]:
    if result[0] != 0:
        return result[0], normalize_output(result[1])
    return result[0], normalize_output(result[1]), normalize_error(result[2])


def snapshot_outputs(directory: pathlib.Path) -> dict[str, bytes]:
    ignored = {"build.ninja", ".ninja_log", ".ninja_deps", ".ninja_lock"}
    return {
        path.relative_to(directory).as_posix(): path.read_bytes()
        for path in directory.rglob("*")
        if path.is_file()
        and path.name not in ignored
        and "src" not in path.relative_to(directory).parts
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--ninja", required=True, type=pathlib.Path)
    parser.add_argument("--knight", required=True, type=pathlib.Path)
    parser.add_argument("--seeds", type=int, default=1000)
    parser.add_argument("--start-seed", type=int, default=0)
    parser.add_argument(
        "--workers",
        type=int,
        default=min(16, (os.cpu_count() or 1) * 2),
        help="maximum concurrent reference/candidate processes",
    )
    parser.add_argument(
        "--missing-sources",
        action="store_true",
        help="leave generated source leaves absent to stress error paths",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="also compare a fresh and incremental real -j1 build per seed",
    )
    options = parser.parse_args()
    ninja = options.ninja.resolve()
    knight = options.knight.resolve()
    cases = [
        lambda target: ["-n", "-j1", target],
        lambda target: ["-t", "commands"],
        lambda target: ["-t", "commands", target],
        lambda target: ["-t", "commands", "-s", target],
        lambda target: ["-t", "commands", "-s"],
        lambda target: ["-t", "inputs", target],
        lambda target: ["-t", "inputs", "-0", target],
        lambda target: ["-t", "inputs", "-d", target],
        lambda target: ["-t", "inputs", "-E", target],
        lambda target: ["-t", "multi-inputs", target],
        lambda target: ["-t", "multi-inputs", "-d", "::", target],
        lambda target: ["-t", "multi-inputs", "-0", target],
        lambda target: ["-t", "query", target],
        lambda target: ["-t", "query", target, "all"],
        lambda target: ["-t", "graph", target],
        lambda target: ["-t", "graph"],
        lambda target: ["-t", "compdb-targets", target],
        lambda target: ["-t", "compdb-targets", "-x", target],
        lambda target: ["-t", "compdb"],
        lambda target: ["-t", "compdb", "make"],
        lambda target: ["-t", "compdb", "-x", "make"],
        lambda target: ["-t", "targets"],
        lambda target: ["-t", "targets", "all"],
        lambda target: ["-t", "targets", "depth", "3"],
        lambda target: ["-t", "targets", "depth", "0"],
        lambda target: ["-t", "targets", "rule"],
        lambda target: ["-t", "targets", "rule", "make"],
        lambda target: ["-t", "targets", "rule", "phony"],
        lambda target: ["-t", "rules"],
        lambda target: ["-t", "rules", "-d"],
        lambda target: ["-t", "clean", "-n"],
        lambda target: ["-t", "clean", "-n", target],
        lambda target: ["-t", "clean", "-n", "-r", "make"],
    ]
    if options.workers < 1:
        parser.error("--workers must be at least 1")
    with tempfile.TemporaryDirectory(prefix="knight-differential-fuzz-") as raw, \
            concurrent.futures.ThreadPoolExecutor(max_workers=options.workers) as executor:
        work = pathlib.Path(raw)
        if not options.missing_sources:
            (work / "src").mkdir()
            for source in range(6):
                (work / "src" / str(source)).write_text(
                    f"source {source}\n", encoding="utf-8"
                )
        for seed in range(options.start_seed, options.start_seed + options.seeds):
            manifest, target = manifest_for(seed)
            (work / "build.ninja").write_text(manifest, encoding="utf-8")
            arguments = [make_args(target) for make_args in cases]
            runs = [
                (args, executor.submit(run, ninja, work, args),
                 executor.submit(run, knight, work, args))
                for args in arguments
            ]
            for args, expected_run, actual_run in runs:
                expected = expected_run.result()
                actual = actual_run.result()
                if expected[0] != 0 and actual[0] != 0:
                    comparable_expected = comparable(expected)[:2]
                    comparable_actual = comparable(actual)[:2]
                else:
                    comparable_expected = comparable(expected)
                    comparable_actual = comparable(actual)
                if comparable_actual != comparable_expected:
                    print(f"seed={seed} args={args!r}")
                    print("--- build.ninja")
                    print(manifest, end="")
                    print("--- ninja", comparable_expected)
                    print("--- knight", comparable_actual)
                    if expected[2]:
                        print("--- ninja stderr", expected[2], end="")
                    if actual[2]:
                        print("--- knight stderr", actual[2], end="")
                    return 1
            if options.execute:
                execution = work / "execution"
                reference_work = execution / "ninja"
                candidate_work = execution / "knight"
                for directory in [reference_work, candidate_work]:
                    if directory.exists():
                        for path in sorted(directory.rglob("*"), reverse=True):
                            if path.is_file() or path.is_symlink():
                                path.unlink()
                            elif path.is_dir():
                                path.rmdir()
                    directory.mkdir(parents=True, exist_ok=True)
                    (directory / "build.ninja").write_text(manifest, encoding="utf-8")
                    if not options.missing_sources:
                        (directory / "src").mkdir()
                        for source in range(6):
                            (directory / "src" / str(source)).write_text(
                                f"source {source}\n", encoding="utf-8"
                            )
                build_args = ["-j1", target]
                expected_run = executor.submit(run, ninja, reference_work, build_args)
                actual_run = executor.submit(run, knight, candidate_work, build_args)
                expected = expected_run.result()
                actual = actual_run.result()
                for phase in ["fresh", "incremental"]:
                    if comparable(actual) != comparable(expected):
                        print(f"seed={seed} phase={phase} args={build_args!r}")
                        print("--- build.ninja")
                        print(manifest, end="")
                        print("--- ninja", comparable(expected))
                        print("--- knight", comparable(actual))
                        if expected[2]:
                            print("--- ninja stderr", expected[2], end="")
                        if actual[2]:
                            print("--- knight stderr", actual[2], end="")
                        return 1
                    if snapshot_outputs(candidate_work) != snapshot_outputs(reference_work):
                        print(f"seed={seed} phase={phase} filesystem mismatch")
                        print("--- build.ninja")
                        print(manifest, end="")
                        return 1
                    if phase == "fresh":
                        expected_run = executor.submit(
                            run, ninja, reference_work, build_args
                        )
                        actual_run = executor.submit(
                            run, knight, candidate_work, build_args
                        )
                        expected = expected_run.result()
                        actual = actual_run.result()
    execution = " plus fresh/incremental execution" if options.execute else ""
    print(
        f"matched {options.seeds} generated DAGs across {len(cases)} command modes{execution}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
