# Ninja compatibility status

This is an evidence ledger, not a claim of full compatibility.

## Implemented

- Core declarations: variables, rules, build edges, defaults, includes,
  subninjas, pools (including zero-depth pools), and scoped variable/rule
  shadowing. Declaration-order lookup and duplicate rule-binding replacement
  match Ninja's parser behavior. Nested `include` and `subninja` paths resolve
  from the build working directory, including when declared in another file.
- Explicit and implicit outputs; explicit, implicit, order-only, validation,
  and dynamic inputs.
- Ninja `$` escapes, path escaping, line continuations, and variable expansion
  for paths and rule bindings, including eager edge/global values and literal
  escaped dollars. Ninja 1.14 dashed variable names and the version-gated `$^`
  newline escape are supported. Special depfile, dyndep, and response-file path
  bindings use Ninja's unescaped `$in`/`$out` expansion. Paths are canonicalized
  to Ninja identity rules across manifests, depfiles, dyndeps, dependency logs,
  tools, and CLI targets. On Windows, Knight separately retains Ninja's
  first-observed per-node separator spelling and restores it in `$in`,
  `$in_newline`, and `$out`, including mixed separators, path-variable
  expansion, and `.`/`..` canonicalization. The complete upstream generic and
  Windows canonicalization samples plus all 16 `SlashTracking` cases are mapped
  directly, with byte-exact `commands` and verbose dry-run differentials.
- Edge-local bindings can participate in output, input, order-only, validation,
  and implicit path expansion, matching Ninja's post-binding path evaluation.
- Duplicate-output, invalid escape, malformed declaration, response-file pair,
  unknown rule/pool, and dependency-cycle diagnostics.
  Tab indentation is rejected with Ninja's spaces-only policy. An upstream-
  derived differential corpus currently covers 18 accepted and more than 30
  rejected parser cases. When invoked as `ninja`, a 47-case rejection corpus
  additionally matches Ninja byte-for-byte across LF/CRLF input, physical
  locations after continuations, EOF distinctions, separator ordering,
  duplicate outputs, pools, defaults, and response-file pairs. Native Knight
  retains its richer line-and-column diagnostic format.
  Missing or malformed root, `include`, and `subninja` manifests also match the
  alias's platform-specific diagnostic bytes and parent-directive locations.
  All eight upstream core-lexer cases are mapped to executable coverage,
  including variable-value serialization, escaped continuations, dashed and
  dotted identifiers, unbraced expansion boundaries, bad escapes, comments at
  EOF, tabs, and the version-gated `$^` escape.
  Knight detects direct, indirect, and hard-link include cycles; this remains an
  intentional robustness improvement over the observed Windows Ninja crash.
  `ninja_required_version` follows Ninja's major/minor
  compatibility comparison and older-major warning policy, including patch
  suffixes and nonnumeric values. The `$^` newline escape is gated by those
  parsed major/minor components rather than loose string parsing.
- Incremental timestamp planning, `restat`, manifest regeneration/reload,
  Make-style GCC depfiles (multiple rules/outputs and Ninja-compatible escapes),
  MSVC `/showIncludes`, and Ninja v7 build-log command hashes.
  Missing depfile-discovered inputs correctly dirty their consumer instead of
  becoming fatal graph errors. Deps-log freshness and multi-output lookup use
  Ninja's output-timestamp and first-output rules. Discovered dependencies are
  loaded only after declared inputs, outputs, and command hashes establish that
  an edge is otherwise clean, so stale dependency graphs cannot pull unrelated
  generators or validations into an already-dirty rebuild. Build-log recorded
  mtimes follow Ninja's command-start/output/restat rules, unchanged restat
  outputs propagate clean state across later invocations, and `.ninja_lock` is
  removed at the end of every build attempt. A successful command may
  intentionally leave an output absent without blocking its dependents in the
  current invocation; the missing path is checked and rebuilt again on the
  next invocation, matching Ninja.
  Multi-output `restat` propagation is tracked per output, so an unchanged
  secondary output cancels only its own dependents while dependents of another
  output that changed still run. Dependency-log discovery also preserves
  validation closure ordering, while an edge already dirty from declared
  inputs skips stale discovered validations like Ninja.
  Unknown `deps` types fail only when their edge enters the selected build
  closure, not while unrelated targets or graph tools are evaluated. A dry-run
  `deps = gcc` edge without a depfile reproduces Ninja's synthetic subcommand
  failure instead of being treated as a successful command.
  Manifest regeneration honors clean `restat` results without reloading and
  stops after Ninja's exact 100 successful self-rebuild attempts with the same
  cycle-limit diagnostic.
- Ninja `.ninja_deps` v4 reading, writing, and recompaction. Differential tests
  exercise Ninja-to-Knight and Knight-to-Ninja metadata exchange. Build and
  dependency logs automatically recompact at Ninja's redundancy thresholds,
  and reject malformed/incompatible data before appending. Build directories
  are created in the same execution phases as Ninja, while metadata files are
  opened lazily only when a record is written. Truncated dependency-log records
  are rolled back to the last valid boundary, and partial build-log lines are
  removed before appending. Recompact-only recovery discards incompatible logs
  without silently recreating them and follows Ninja's distinct build/deps-log
  exit behavior. Unreadable metadata logs fail before commands can run, while
  invalid dependency-log signatures retain Ninja's warning-and-recovery path.
  Build-log recovery distinguishes old and future versions, while accepting
  Ninja's numeric signature grammar. Tools open logs only in Ninja's
  corresponding execution phase. Dependency-
  tool output and dependency-log recompaction preserve Ninja's persisted
  node-ID ordering.
- Dyndep v1 implicit inputs, implicit outputs, and `restat`, including dyndep
  files generated during the same build and multi-level discovery where one
  loaded dyndep reveals another. Preparation iterates to a fixed point while
  preserving generated-file order. Independent requested work runs concurrently
  with generated dyndep producers, while consumers that may gain new inputs are
  held until the relevant file is loaded. Version suffixes, final-newline
  requirements, per-edge duplicate-statement detection, entry ownership, and
  output-conflict diagnostics follow Ninja. Every edge bound to a loaded file
  must be mentioned, records for edges bound to another file are rejected, and
  dynamic outputs may not redeclare either static or previously discovered
  outputs. Missing source dyndeps fail in Ninja's load phase with matching
  platform diagnostics. Ready source dyndeps bypass generated-file prebuild
  planning, while batches of independent files are read and parsed on two
  workers with deterministic file-order validation. `-d stats` reports dyndep
  graph, prebuild, and load/apply time separately.
  All 42 upstream dyndep-parser cases are now represented directly: 19
  accepted layouts and 23 rejected layouts cover version syntax, LF/CRLF,
  empty implicit lists, multiple edges, graph-aware output identity, EOF
  distinctions, bindings, and positioned diagnostics. The `ninja` alias
  matches the rejection corpus byte-for-byte, while native Knight retains
  richer line-and-column diagnostics. As in Ninja, every nonempty `restat`
  value, including `0`, enables restat behavior. Additional lexer-derived
  differentials cover continued version values, continuations that consume a
  following build line, forbidden `$^`, escaped colons, malformed braced
  escapes, continued build paths, and comments at EOF. Graph lookup now occurs
  at Ninja's parser phase, preserving diagnostic precedence over errors later
  in the same statement.
- Parallel command execution, longest-remaining-path scheduling with stable
  declaration-order ties, depth-one/live console pools with buffered output
  from concurrent ordinary work, default/custom pool
  limits (including zero-depth meaning unlimited), and Ninja-compatible
  ready-edge pool reservations, including multi-output notification order.
  Clean phonies are collapsed before the initial pool frontier is ranked by
  critical path. Once scheduling begins, an already-delayed edge claims a
  newly released pool slot before dependents made ready by that same
  completion, matching Ninja's temporal reservation semantics. Once a real
  scheduling sweep fills its command capacity, lower-priority phonies wait for
  a command completion too, preserving the pool-reservation order between
  requested targets and their validations.
  Response files, including Windows text-mode newline conversion, inherited
  and child-forwarded GNU/Cargo jobservers,
  load limiting,
  `MAKEFLAGS=n`, `-j`, `-k`, `-l`, `-n`, `-v`, `--quiet`, `--status`, `-C`,
  and `-f`. Both classic `NINJA_STATUS` placeholders and Ninja 1.14
  `--status` variables are supported. Both upstream `StatusTest` cases are
  mapped directly, including escaped percent signs and zero elapsed time.
  The upstream basic `State` command-expansion case is mapped directly as
  well. Getopt-style short-option clusters and
  attached option values are accepted, non-positive `-k` is unlimited, and
  saturated and whitespace-prefixed `-j`/`-k` values follow `strtol`, while
  `-l` follows the platform C library's complete `strtod` grammar. Attached
  long-option values preserve Ninja's Windows/POSIX `getopt` split. The
  default parallelism uses Ninja's processor-count heuristic. Status ETA
  and predicted progress use Ninja's historical per-edge CPU-time model,
  including its stale-history rejection heuristic. GNU-style unambiguous long-
  option abbreviations are accepted at top level and by `inputs`,
  `multi-inputs`, and `restat`.
  Load limiting computes Ninja's integer launch capacity for each scheduling
  sweep, decrements it as commands start, always permits one command to make
  progress from idle, and preserves `strtod` NaN as a disabled limit.
  Rule commands that expand to an empty string retain Ninja's platform split:
  they are valid no-op shell commands on POSIX and dry runs, while Windows
  execution preserves Ninja's `CreateProcess` failure framing when invoked as
  `ninja` and gives the native `knight` command a shorter diagnostic.
  All 14 cases in Ninja's upstream `SubprocessTest` suite now have explicit
  integration-level coverage. This includes byte-exact Windows command-start
  failures, child- and parent-directed INT/TERM/HUP handling, inherited console
  descriptors in a real Linux pseudo-terminal, more than 1,024 simultaneous
  processes, closed stdin, multi-process execution, and jobserver wakeups.
  Parent-directed POSIX signals terminate active process groups and return
  through the build loop so the `ninja` alias retains Ninja's build-stop output
  instead of exiting silently from the signal handler.
- Debug modes `explain`, `keepdepfile`, `keeprsp`, and `nostatcache`, plus the
  `phonycycle` warning policy, including the legacy self-reference behavior
  exposed by graph tools. The compatibility exception is restricted to
  Ninja's exact single-output phony shape; same-edge references on multi-output
  phonies remain real dependency cycles. `-d stats` reports manifest, metadata, closure,
  scheduler, log, filesystem, and edge-evaluation timings.
- Tools: `targets`, `commands`, `clean`, `query`, `compdb`, `compdb-targets`,
  `rules`, `recompact`, `restat`, `deps`, `inputs`, `multi-inputs`, `graph`,
  `cleandead`, `missingdeps`, `wincodepage`, and the deprecated `msvc` helper.
  Ninja's hidden early `urtle` tool is also accepted without appearing in the
  public tool list.
  The Python-backed `browse` tool is available on POSIX, matching Ninja's
  platform split, embedded server, query parsing, HTML rendering, HTTP
  behavior, and command-line help, while `msvc` and `wincodepage` are
  advertised only on Windows. The Windows binary embeds Ninja's UTF-8 active-
  code-page and long-path-aware application manifest; `wincodepage` reports
  the actual process code page rather than a fixed label, with Unicode paths
  beyond `MAX_PATH` covered differentially. Unknown tools, target modes,
  targets, debug settings, and warning flags provide Ninja-compatible spelling
  suggestions. Ninja's four-case edit-distance corpus is mapped directly,
  including bounded distance and replacement-disabled behavior.
  `commands`, `clean`, `compdb`, `rules`, `targets`, `inputs`, and
  `multi-inputs` short/long options, including bundled short flags and attached
  delimiters, have differential coverage. Missing arguments and attached
  values follow the platform `getopt` split for `inputs`, `multi-inputs`, and
  `restat`; the deprecated Windows `msvc` helper follows its getopt/usage exit
  behavior as well. Unknown Windows `getopt_long` words preserve its unusual
  fallback to a short-option cluster, including an initially valid `d` option.
  Top-level cluster expansion stops at `-t`, leaving tool clusters intact for
  the selected parser. The bundled Windows `getopt` operand permutation quirk
  after multi-character clusters is reproduced on Windows, and
  `POSIXLY_CORRECT` disables operand permutation at both levels like upstream.
  `inputs` uses a single collector
  across all requested targets, matching Ninja's shared-input deduplication,
  and sorts rendered shell-escaped paths rather than their raw spellings.
  Cleaning loads valid dyndeps while tolerating malformed ones, honors
  dry-run/verbose/quiet, removes empty output directories, continues after
  individual removal failures, and counts auxiliary depfiles and response
  files. Target cleaning traverses prerequisites, while rule cleaning retains
  Ninja's distinct behavior for paths produced by the built-in `phony` rule.
  `cleandead` preserves former outputs that remain explicit, implicit, or
  order-only inputs in the current graph.
  Plain `compdb` and target-scoped `compdb-targets` preserve Ninja's distinct
  phony-edge filtering behavior. Compilation databases use Ninja's exact
  pretty-printed JSON shape, platform newline bytes, and JSON control-character
  escapes. All four upstream JSON encoder cases are mapped directly, including
  standard escapes, arbitrary C0 controls, and unescaped UTF-8. Response-file
  expansion also preserves Ninja's legacy first-marker
  behavior. `compdb-targets` rejects input-only nodes as non-targets instead of
  silently returning an empty database. Edges used only for validation are
  excluded, while an output used as both a validation and a regular input
  remains present. `restat`
  compacts logs, handles missing and selected outputs, and rewrites metadata
  even under `-n` like Ninja.
  `missingdeps` scans the default-target closure when no targets are supplied,
  reads both `.ninja_deps` entries and plain depfiles, and ignores unrelated
  branches.
- A deterministic generated-DAG differential corpus combines multi-output and
  phony edges with explicit, implicit, order-only, and validation inputs, plus
  bounded and unlimited pools. It compares 33 dry-run/traversal/tool modes and
  fresh plus incremental real builds. Materialized-source and missing-source
  graphs each match Ninja on 3,400 Windows seeds, totaling 224,400 tool-mode
  comparisons and 13,600 real build phases. Both corpora also match
  on 500 Linux seeds each (33,000 tool-mode comparisons and 2,000 real build
  phases). The harness runs
  independent reference/candidate processes concurrently while preserving
  deterministic result order. This audit found and now covers exact
  empty-compdb JSON, missing regular phony inputs, phony-specific missing
  order-only behavior, dry-run no-work output, and Ninja's unlimited dry-run
  ready-frontier/FIFO-completion simulation even when `-j1` is supplied.
- CLI node lookup supports Ninja's `target^` first-dependent syntax, dependency
  log reverse lookup, and build-directory fallback at the same execution
  phases as Ninja. Existing filesystem paths not present in the graph are not
  silently accepted as targets.
- Filesystem stat failures remain distinct from missing paths, including with
  `-d nostatcache`, and abort planning with Ninja-compatible diagnostics. The
  `deps` tool retains Ninja's unusual report-and-continue behavior for the same
  failures, while post-command failures during restat, generator, or dependency
  recording stop the build in Ninja's corresponding phase. POSIX epoch-zero
  timestamps are normalized to one nanosecond in dependency metadata, matching
  Ninja's reserved-zero convention. Batched stat-cache groups treat a missing
  or non-directory parent as authoritative evidence that all requested children
  are absent, avoiding redundant per-child probes while retaining fallback for
  permission and other enumeration failures.
- `graph` emits Ninja-shaped Graphviz output with implicit root selection,
  direct single-input/single-output edges, rule nodes for fan-in/fan-out, and
  dotted order-only edges. It loads only dyndeps reachable from the displayed
  roots and warns without failing when one is missing or malformed. IDs remain
  deterministic rather than pointer-based.
- CMake configure, compiler detection, build, no-op rebuild, manifest
  regeneration, and header-triggered rebuild on Windows.
- Iterative dependency traversal verified on a 50,000-edge chain without call
  stack growth.
  A 100,000-edge commandless phony chain also completes in Knight; the Windows
  Ninja reference crashes with status `0xC0000005` at 10,000 phony edges.
- Inputless phony dependencies retain Ninja's always-dirty behavior.
- Default root discovery reports rootless cyclic graphs instead of silently
  treating them as no-work builds.
- Windows console interrupts terminate Knight with status 2 and tear down its
  full descendant process tree through a kill-on-close job object. Interrupted
  edges remove outputs whose timestamps changed, and depfile-producing edges
  remove every output plus the depfile, while untouched pre-existing outputs
  are retained.
- POSIX signals terminate Knight with Ninja's status 130 and tear down every active child
  process group with the same interrupted-output cleanup. This path is
  exercised by the Linux integration suite.
- Non-console stdout and stderr share one capture pipe, preserving emitted
  order. Raw bytes are retained apart from Ninja's Windows CRT text-mode CRLF
  conversion, which is reproduced for tool, diagnostic, status, and captured
  command output. ANSI escapes follow Ninja's terminal, `TERM=dumb`,
  `NO_COLOR`, `CLICOLOR_FORCE`, and `FORCE_COLOR` precedence.
  Failed-command headers and command lines precede buffered output and
  dependency-extraction diagnostics. Smart terminals receive Ninja's
  carriage-return start/finish refreshes, clear-to-end-of-line sequences, and
  final newline framing. Quiet, verbose, custom-status, dry-run, and unterminated
  command output modes have byte-for-byte PTY differential coverage.
  Non-verbose status lines query the live terminal width and use Ninja's
  ANSI-aware middle elision without wrapping. All three upstream
  `ElideMiddle` cases are mapped directly, and 20-column plain, colored,
  non-verbose, and verbose output matches Ninja in a real Linux PTY.
  MSVC dependency filtering recognizes bare CR, LF, and CRLF boundaries,
  retains prefix-only lines, and emits Ninja's deliberately LF-only filtered
  output on Windows without changing ordinary command-output text mode.
  All eight upstream `CLParser` cases are mapped directly, covering default
  and custom prefixes, initial-space trimming, compiler input echoes, the
  post-include echo boundary, system-header filtering, duplicate headers, and
  canonicalized path duplicates. An empty configured prefix correctly falls
  back to Ninja's English `/showIncludes` prefix. Windows include paths use
  Ninja's case-insensitive same-drive relativization across mixed separators,
  `.`/`..`, differently-cased absolute roots, and cross-drive inputs. The four
  path-behavior groups from the upstream `IncludesNormalize` corpus are mapped
  directly; Knight deliberately retains its long-path support instead of
  reproducing Ninja's two `MAX_PATH` rejection cases. The deprecated `msvc`
  helper also writes Ninja's CRLF depfile bytes and space escapes, preserves
  raw child output when dependency extraction is not requested, imports its
  binary environment block, and forwards child stderr byte-for-byte.
- Diagnostic identity follows the invocation name: the normal executable uses
  `knight:`, while a copy or link installed as `ninja` uses Ninja-compatible
  `ninja:`/`ninja explain:` prefixes. `-d explain` identifies the dirty input
  that actually propagates through each edge, respects clean restat pruning,
  and reports dyndep loads. Knight intentionally emits a missing-output reason
  once, while an executable named `ninja` reproduces Ninja's legacy duplicate
  dyndep explanation, fatal unknown-tool/numeric-option diagnostics, and
  stdout-routed build-stop summaries for drop-in output parity.
  Multiple failed commands distinguish Ninja's `subcommands failed` and
  `cannot make progress due to previous errors` summaries. Alias help,
  version, debug/warning lists, and their intentionally nonzero list/help exit
  statuses match Ninja byte-for-byte.

## Not yet complete

- Exact semantics for every tool option.
- Full lexical and diagnostic-text parity across Ninja's complete test corpus.
  Ninja's own POSIX `misc/output_test.py` now passes all 24 tests unchanged,
  including smart-terminal progress, invocation identity, `compdb-targets`,
  input ordering, absent-output scheduling, dyndep diagnostics, child exit
  status 130, and signal-status cases. Wider upstream unit-test coverage is
  still being ported into differential tests.
- Broader cross-platform runtime validation. The current Windows gates pass 76
  library, 6 CLI, and 121 differential tests; Linux-under-WSL passes 71 library,
  5 CLI, and 96 differential tests. Release builds, clippy, a Windows-hosted
  Linux target check, and a CMake no-op rebuild also pass locally; macOS and
  other Unix variants are not yet exercised in CI here.
  Ninja's upstream builddir-target (5/5), compdb-validation (5/5), and
  restat-builddir (1/1) Python integration suites also pass unchanged under
  WSL. Its jobserver suite passes 4/5: FIFO inheritance, token efficiency,
  MAKEFLAGS forwarding, and no-jobserver scheduling pass; the only mismatch is
  that Knight supports the inherited POSIX pipe protocol rather than printing
  Ninja's "not supported" warning. A native Rust regression verifies that the
  pipe protocol enforces its token limit. When installed as `ninja`, Knight
  instead maps the complete upstream MAKEFLAGS parser corpus and reproduces
  Ninja's invalid/unsupported-mode warnings, mode announcement, initialization
  errors, quiet/dry-run policy, and MAKEFLAGS precedence byte-for-byte. Native
  `knight` retains the additional pipe-protocol support.
- Performance superiority across every representative workload. Knight leads
  the 10,000-edge median and all three P95 measurements in the latest warm
  no-op sweep, but trails two 1,000-edge medians and is nowhere near the
  requested order of magnitude; see `BENCHMARKS.md`.

Case-level closure is tracked in `UPSTREAM_TESTS.md`. Partial executable-suite
rows in that ledger are parity blockers, even when the corresponding feature
already has broad differential coverage.
