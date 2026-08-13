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
| `build_test` | 121 | Partial | Broad scheduling/build differentials; full case mapping remains open |
| `clean_test` | 20 | Partial | Clean, rule clean, dyndep, dead-output, directory, and failure behavior |
| `clparser_test` | 8 | Mapped | `build::tests::upstream_msvc_clparser_corpus` |
| `depfile_parser_test` | 29 | Mapped | `depfile::tests::upstream_depfile_parser_corpus` |
| `deps_log_test` | 10 | Mapped | Round-trip, 100K inputs, deduplication, live recompaction, headers, truncation, reverse lookup, and malformed-record recovery |
| `disk_interface_test` | 16 | Partial | Missing/error distinctions, stat-cache behavior, long paths, and timestamps |
| `dyndep_parser_test` | 42 | Mapped | `dyndep::tests::accepts_upstream_version_and_layout_corpus` and rejection corpus |
| `edit_distance_test` | 4 | Mapped | `manifest::tests::upstream_edit_distance_corpus` |
| `elide_middle_test` | 3 | Mapped | `build::tests::upstream_elide_middle_corpus` |
| `explanations_test` | 3 | Implementation-specific | C++ explanation-storage wrapper; observable `-d explain` behavior has separate differentials |
| `graph_test` | 55 | Partial | Generated DAG corpus plus graph/query/target traversal differentials |
| `includes_normalize_test` | 6 | Partial | Four path groups mapped; two `MAX_PATH` rejections intentionally remain long-path successes |
| `jobserver_test` | 7 | Mapped | `main::tests::upstream_jobserver_makeflags_parser_corpus` and native-mode corpus |
| `json_test` | 4 | Mapped | `main::tests::upstream_json_encoder_corpus` |
| `lexer_test` | 8 | Mapped | `manifest::tests::upstream_lexer_value_identifier_and_escape_corpus` |
| `manifest_parser_test` | 53 | Partial | Acceptance, rejection, and byte-exact alias diagnostic corpora |
| `missing_deps_test` | 7 | Mapped | Empty/clean graphs, direct and indirect fixes, missing/cyclic discovered deps, and graph-cycle rejection |
| `msvc_helper_test` | 3 | Mapped | Helper depfile, raw output, environment, and stderr differentials |
| `state_test` | 1 | Mapped | Command expansion case in `build` tests |
| `status_test` | 2 | Mapped | Status placeholder and elapsed-time corpus |
| `string_piece_test` | 2 | Implementation-specific | C++ container API, no executable contract |
| `string_piece_util_test` | 5 | Implementation-specific | C++ helper API, no executable contract |
| `subprocess_test` | 14 | Mapped | Command failures; child/parent INT, TERM, HUP; console TTYs; single/multi/lots; stdin; jobserver |
| `util_test` | 14 | Partial | Canonical paths, slash tracking, spellcheck, shell escaping, and platform utilities |

The inventory contains 448 C++ cases. A **Partial** row remains an explicit
parity blocker regardless of how many adjacent Knight tests pass.

## Python suites

| Upstream suite | Cases | Status | Current evidence |
| :--- | ---: | :--- | :--- |
| `misc/jobserver_pool_test.py` | 5 | Partial | Pool/token behavior covered, but unchanged-suite closure is not recorded |
| `misc/jobserver_test.py` | 5 | Partial | Native Knight passes 4/5; the `ninja` alias reproduces the unsupported pipe policy |
| `misc/ninja_syntax_test.py` | 21 | Implementation-specific | Tests Ninja's optional Python manifest-writer module, not the executable |
| `misc/output_test.py` | 24 | Upstream pass | All 24 tests pass unchanged under WSL |
| `tests/builddir_target/test_builddir_target.py` | 5 | Upstream pass | All five build-directory target cases pass unchanged under WSL |
| `tests/compdb/test_compdb_validation.py` | 5 | Upstream pass | All five compilation-database validation cases pass unchanged under WSL |
| `tests/restat/test_restat_builddir.py` | 1 | Upstream pass | The build-directory restat case passes unchanged under WSL |

The Python inventory contains 66 cases. Together with the 448 C++ cases above,
this ledger tracks all 514 upstream test cases in the pinned source tree.

The next parity work should turn **Partial** executable suites into **Mapped**
or **Upstream pass** rows. New performance work does not close this ledger.
