# Knight

[![CI](https://github.com/c-schembri/knight/actions/workflows/ci.yml/badge.svg)](https://github.com/c-schembri/knight/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Knight is a fast, diagnostic-first build executor compatible with Ninja build
files. It reads the same manifests, uses the same build and dependency logs,
and can be selected anywhere a build system expects a Ninja executable.

Knight is written in Rust and tested differentially against upstream Ninja
1.14. Its focus is practical compatibility without giving up useful native
diagnostics, predictable tail latency, or performance on large build graphs.

> Knight is under active development. The audited Ninja surface is extensive,
> but compatibility is not yet claimed for every platform and workload. See
> [Compatibility](#compatibility) before replacing Ninja in critical builds.

## Highlights

- **Drop-in build execution:** variables, rules, pools, `phony`, `include`,
  scoped `subninja`, response files, depfiles, dyndeps, validations, `restat`,
  manifest regeneration, load limits, and GNU/Cargo jobservers.
- **Ninja metadata interoperability:** reads and writes Ninja v7 `.ninja_log`
  files and v4 `.ninja_deps` files. Ninja and Knight can alternate in the same
  build directory.
- **Native dependency support:** GCC-style depfiles and MSVC
  `/showIncludes`, including Ninja-compatible filtering and path handling.
- **Familiar command line:** common Ninja flags, status formats, debug modes,
  and the full audited subtool surface, including `commands`, `compdb`,
  `graph`, `query`, `clean`, `deps`, `missingdeps`, and `restat`.
- **Better diagnostics:** native `knight` invocations report positioned parser
  errors and concise build failures. Installing the binary as `ninja` switches
  diagnostic identity and compatibility quirks to match upstream output.
- **Cross-platform:** native CI runs on Windows, Linux, macOS, FreeBSD,
  OpenBSD, NetBSD, and DragonFly BSD, with additional cross-target checks for
  illumos, Solaris, and MinGW.

## Quick Start

Knight currently installs from source and requires Rust 1.85 or newer.

```console
git clone https://github.com/c-schembri/knight.git
cd knight
cargo build --release
```

The executable is written to `target/release/knight` on Unix and
`target\release\knight.exe` on Windows.

Run it in a directory containing `build.ninja`:

```console
knight
knight -j 8
knight app tests
knight -C out/debug
```

PowerShell, without installing the binary:

```powershell
.\target\release\knight.exe -C path\to\build
```

For CMake projects, point the Ninja generator at Knight:

```console
cmake -S . -B build -G Ninja -DCMAKE_MAKE_PROGRAM=/path/to/knight
cmake --build build
```

To exercise the strictest command-output compatibility behavior, install a
copy or link named `ninja`:

```console
ln -s /path/to/knight /usr/local/bin/ninja
```

```powershell
Copy-Item .\knight.exe .\ninja.exe
```

Knight uses its invocation name intentionally: `knight` enables its native
diagnostic presentation, while `ninja` reproduces Ninja's diagnostic identity
and stable compatibility quirks.

## Command Line

The everyday interface follows Ninja:

```text
usage: knight [options] [targets...]

-C DIR             change to DIR before doing anything else
-f FILE            use FILE as the manifest [default: build.ninja]
-j N               run N jobs in parallel (0 means unlimited)
-k N               keep going until N jobs fail (0 means unlimited)
-l N               do not start jobs when system load exceeds N
-n                 dry run
-v                 show full command lines
--quiet            suppress progress output
--status FORMAT    customize progress status
-d MODE            enable a debug mode
-t TOOL [ARGS...]  run a subtool
```

Useful examples:

```console
# Explain why an edge is dirty.
knight -d explain app

# Print phase-level build-engine timings.
knight -d stats

# Preview work without running commands.
knight -n -v

# Produce a Clang-compatible compilation database.
knight -t compdb cxx cc > compile_commands.json

# Inspect persisted compiler dependencies.
knight -t deps

# Render the selected graph with Graphviz.
knight -t graph app | dot -Tsvg -o graph.svg
```

Run `knight -d list` and `knight -t list` for the available debug modes and
subtools. `knight --version` prints the supported Ninja compatibility version;
`knight --knight-version` prints Knight's package version.

## Compatibility

Knight supports the core Ninja language and execution model, including:

- Explicit and implicit outputs.
- Explicit, implicit, order-only, validation, and dynamic inputs.
- Ninja escaping, continuation, expansion, canonical path identity, and
  Windows separator spelling.
- Pools, console edges, critical-path scheduling, failure limits, dry runs,
  status formatting, response files, and interruption cleanup.
- Incremental timestamp planning, command hashing, `generator`, `restat`,
  manifest rebuilds, GCC depfiles, MSVC includes, and generated dyndeps.
- Ninja's public tool and top-level option surfaces covered by the differential
  suite.

Compatibility is tested against a pinned upstream Ninja commit rather than
inferred from similar output. The Windows gate currently runs 95 library tests,
7 CLI tests, and 155 executable differential tests. Native differential suites
also run on Linux, macOS, FreeBSD, OpenBSD, NetBSD, and DragonFly BSD.

The remaining qualification work is primarily native runtime coverage on
illumos, Solaris, MinGW, and AIX, plus performance leadership on every graph
shape. Exact supported cases, intentional native improvements, alias behavior,
and outstanding work are recorded in:

- [COMPATIBILITY.md](COMPATIBILITY.md), the current compatibility ledger.
- [UPSTREAM_TESTS.md](UPSTREAM_TESTS.md), case-level upstream traceability.

## Performance

The benchmark suite builds isolated, byte-identical work trees with Knight and
upstream Ninja, alternates launch order, and verifies representative output
hashes. On the current Windows x64 reference machine (Ryzen 9 9900X, 24 jobs,
Clang/LLD), Knight wins 9 of 11 lifecycle medians:

| Workload | Ninja | Knight | Difference |
| :--- | ---: | ---: | ---: |
| 128-file parallel clean build | 992.737 ms | 841.643 ms | Knight 15.2% faster |
| 128-file serial clean build | 4721.978 ms | 4717.338 ms | Tied |
| Warm no-op | 6.274 ms | 6.033 ms | Knight 3.8% faster |
| One-source rebuild and relink | 126.730 ms | 101.642 ms | Knight 19.8% faster |
| Shared-header rebuild | 1072.106 ms | 820.106 ms | Knight 23.5% faster |
| 256 parallel short commands | 3008.617 ms | 2430.099 ms | Knight 19.2% faster |
| 256 serial short commands | 4181.147 ms | 3866.787 ms | Knight 7.5% faster |
| 1,000 ready dyndep files | 65.923 ms | 52.684 ms | Knight 20.1% faster |

These are results from one machine, not universal claims. Compiler time,
process startup, filesystem caches, antivirus software, and host scheduling can
dominate short samples. Full distributions, exact configurations, remaining
losses, and every focused benchmark are documented in
[BENCHMARKS.md](BENCHMARKS.md).

Run the end-to-end suite from PowerShell with an upstream Ninja binary:

```powershell
powershell -NoProfile -File scripts\benchmark-build-lifecycle.ps1 `
  -Ninja C:\path\to\ninja.exe
```

## Development

Build and run the local test suite:

```console
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

The executable differential tests use `KNIGHT_NINJA` to locate the upstream
reference executable:

```console
KNIGHT_NINJA=/path/to/ninja cargo test --test differential
```

```powershell
$env:KNIGHT_NINJA = 'C:\path\to\ninja.exe'
cargo test --test differential
```

CI builds a pinned Ninja revision before running the suite. When changing
parser, scheduler, metadata, CLI, or output behavior, add a differential test
that runs the same fixture through both executables. Benchmark changes should
use interleaved samples and preserve raw results under `target/`.

## Project Layout

- `src/manifest.rs`: manifest loading, parsing, expansion, and diagnostics.
- `src/build.rs`: graph planning, incremental state, scheduling, and command
  execution.
- `src/deps_log.rs`, `src/depfile.rs`, `src/dyndep.rs`: dependency formats.
- `tests/differential.rs`: executable behavior compared with upstream Ninja.
- `scripts/`: lifecycle benchmarks, focused benchmarks, and differential fuzzing.
- `benches/`: Criterion microbenchmarks for parser and filtering hot paths.

## License

Knight is licensed under the [Apache License 2.0](LICENSE).
