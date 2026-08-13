# Upstream Ninja test traceability

This ledger tracks parity against Ninja commit
`b51a1e37c2fb89bbefa600bd155e1ce13983f09d`. It is deliberately separate
from Knight's test count: one Rust test can map many upstream cases, and a
large Knight-only suite does not prove that an upstream behavior was covered.

Status meanings:

- **Mapped**: every case in the upstream suite has an explicit Knight test or
  an equivalent integration-level assertion.
- **Upstream pass**: the upstream suite runs unchanged against Knight.
- **Partial**: relevant behavior is implemented, but case-by-case closure has
  not been demonstrated.
- **Implementation-specific**: tests a C++ helper API rather than observable
  Ninja executable behavior. It is inventoried but not a drop-in requirement.

## C++ suites

| Upstream suite | Cases | Status | Current evidence |
| :--- | ---: | :--- | :--- |
| `build_log_test` | 11 | Mapped | Round-trip, signatures, duplicates, truncation, versions, spaces, restat, long lines, multi-output records, and recompaction |
| `build_test` | 121 | Mapped | Explicit 121-case evidence map covering planning, pools, incremental/log behavior, depfiles, response files, failures, phony edges, restat, mtime races, dyndeps, validations, interrupts, and diagnostics on Windows and WSL |
| `clean_test` | 20 | Mapped | All/target/rule and dry-run modes, multi-output/generator/phony edges, dep/rsp files, dyndeps, failures, spaces, and live/dead build-log outputs |
| `clparser_test` | 8 | Mapped | `build::tests::upstream_msvc_clparser_corpus` |
| `depfile_parser_test` | 29 | Mapped | `depfile::tests::upstream_depfile_parser_corpus` |
| `deps_log_test` | 10 | Mapped | Round-trip, 100K inputs, deduplication, live recompaction, headers, truncation, reverse lookup, and malformed-record recovery |
| `disk_interface_test` | 16 | Mapped | Missing/error distinctions, files/directories/symlinks, Windows cache and long paths, reads, recursive directory creation, removal, and four dependency-scan shapes |
| `dyndep_parser_test` | 42 | Mapped | `dyndep::tests::accepts_upstream_version_and_layout_corpus` and rejection corpus |
| `edit_distance_test` | 4 | Mapped | `manifest::tests::upstream_edit_distance_corpus` |
| `elide_middle_test` | 3 | Mapped | `build::tests::upstream_elide_middle_corpus` |
| `explanations_test` | 3 | Implementation-specific | C++ explanation-storage wrapper; observable `-d explain` behavior has separate differentials |
| `graph_test` | 55 | Mapped | Explicit 55-case evidence map covering dirty propagation, implicit outputs, collectors, escaping, depfiles/cycles, dyndeps, validations, phony mtimes, and scheduling priority on Windows and WSL |
| `includes_normalize_test` | 6 | Mapped | Simple, relative, case, drive, overlong-input, exact-`MAX_PATH`, and relative-to-absolute overflow cases; native Knight retains long-path support |
| `jobserver_test` | 7 | Mapped | `main::tests::upstream_jobserver_makeflags_parser_corpus` and native-mode corpus |
| `json_test` | 4 | Mapped | `main::tests::upstream_json_encoder_corpus` |
| `lexer_test` | 8 | Mapped | `manifest::tests::upstream_lexer_value_identifier_and_escape_corpus` |
| `manifest_parser_test` | 53 | Mapped | Explicit 53-case inventory with parser-state assertions, command expansion, paths, defaults, scopes/includes, dyndeps, and byte-exact alias diagnostics on Windows and WSL |
| `missing_deps_test` | 7 | Mapped | Empty/clean graphs, direct and indirect fixes, missing/cyclic discovered deps, and graph-cycle rejection |
| `msvc_helper_test` | 3 | Mapped | Helper depfile, raw output, environment, and stderr differentials |
| `state_test` | 1 | Mapped | Command expansion case in `build` tests |
| `status_test` | 2 | Mapped | Status placeholder and elapsed-time corpus |
| `string_piece_test` | 2 | Implementation-specific | C++ container API, no executable contract |
| `string_piece_util_test` | 5 | Implementation-specific | C++ helper API, no executable contract |
| `subprocess_test` | 14 | Mapped | Command failures; child/parent INT, TERM, HUP; console TTYs; single/multi/lots; stdin; jobserver |
| `util_test` | 14 | Mapped | Generic/Windows canonicalization, slash tracking, bounded buffers, 219 components, parent/absolute paths, shell escaping, and ANSI stripping |

The inventory contains 448 C++ cases. A **Partial** row remains an explicit
parity blocker regardless of how many adjacent Knight tests pass.

## Python suites

| Upstream suite | Cases | Status | Current evidence |
| :--- | ---: | :--- | :--- |
| `misc/jobserver_pool_test.py` | 5 | Implementation-specific | Tests Ninja's standalone Python pool wrapper rather than the Ninja executable; 4 pass and 1 platform case skips unchanged under WSL |
| `misc/jobserver_test.py` | 5 | Upstream pass | All five executable jobserver cases pass unchanged under WSL, including FIFO token wakeups and Ninja-alias pipe warnings |
| `misc/ninja_syntax_test.py` | 21 | Implementation-specific | Tests Ninja's optional Python manifest-writer module, not the executable |
| `misc/output_test.py` | 24 | Upstream pass | All 24 tests pass unchanged under WSL |
| `tests/builddir_target/test_builddir_target.py` | 5 | Upstream pass | All five build-directory target cases pass unchanged under WSL |
| `tests/compdb/test_compdb_validation.py` | 5 | Upstream pass | All five compilation-database validation cases pass unchanged under WSL |
| `tests/restat/test_restat_builddir.py` | 1 | Upstream pass | The build-directory restat case passes unchanged under WSL |

The Python inventory contains 66 cases. Together with the 448 C++ cases above,
this ledger tracks all 514 upstream test cases in the pinned source tree.

Of those, 36 cases exercise implementation-specific C++ helpers or optional
Python support modules rather than the Ninja executable. The executable-parity
scope is therefore 478 cases, and all 478 are now explicitly mapped or pass
unchanged. No executable suite in this pinned upstream inventory remains
Partial.

This closes the pinned test-corpus ledger. Separate explicit inventories in
`tests/differential.rs` map all 20 public/hidden tool entry points and all 15
top-level option families on Windows, Linux, macOS, FreeBSD, OpenBSD, and
NetBSD. DragonFly BSD runs the same executable inventory natively, including
its distinct build-log directory and `restat` behavior. The native CI matrix
also covers inherited-pipe jobservers, BSD `getopt`
diagnostics and operand ordering, portable timestamp manipulation, and
Knight's successful 1,025-process run when the macOS Ninja reference exhausts
its descriptor limit.
FreeBSD and OpenBSD each pass 90 native library, 6 CLI, and 128 differential
tests. NetBSD passes 90 library, 6 CLI, and all 128 differential tests, with
the 1,025-process case isolated from the parallel test runner. DragonFly passes
90 library, 6 CLI, and 127 regular differential tests; its isolated 1,025-
process case passes with an 8,192-descriptor limit. NetBSD, illumos, Solaris,
and MinGW cross-target CI checks pass, but native runtime differentials on
illumos, Solaris, MinGW, and AIX remain separate platform-validation work. New
performance work does not substitute for that platform validation.
