# Benchmarks

Benchmarks are comparative evidence, not a blanket performance claim. Run them
with `scripts/benchmark.ps1`; raw results are written below `target/` and are
not committed. The runner warms both tools three times and alternates launch
order on every sample to reduce cache/order bias.

## 2026-08-13, Windows x64

Tools: Knight 0.1.0 release build and upstream Ninja 1.14.0.git release build.
The workload is a warm no-op build where every output exists and the Ninja v7
log is populated. Each row reports 1,000 fresh, interleaved process invocations.

| Shape | Edges | Tool | Median | Minimum | P95 |
| :--- | ---: | :--- | ---: | ---: | ---: |
| Independent | 1,000 | Ninja | 5.666 ms | 5.193 ms | 10.346 ms |
| Independent | 1,000 | Knight | 5.821 ms | 5.454 ms | 6.526 ms |
| Independent | 10,000 | Ninja | 20.267 ms | 18.212 ms | 25.502 ms |
| Independent | 10,000 | Knight | 19.406 ms | 18.090 ms | 20.890 ms |
| Chain | 1,000 | Ninja | 5.796 ms | 5.292 ms | 10.772 ms |
| Chain | 1,000 | Knight | 6.085 ms | 5.549 ms | 6.912 ms |

Criterion measured Knight's scoped 10,000-edge manifest parse at 5.519 ms
(95% estimate 5.467-5.574 ms), 52.3% faster than the earlier 11.569 ms
implementation. In the latest process-level warm no-op sweep, Knight is 4.2%
faster at 10,000 independent edges, while its median trails Ninja by 2.7% at
1,000 independent edges and 5.0% on the 1,000-edge chain. Knight's P95 remains
lower in all three sweeps by 18.1-36.9%. Its warm 10,000-edge manifest load is
about 4.2-4.6 ms under `-d stats` instrumentation.

The byte-exact parser-diagnostic work was benchmarked repeatedly while its
source-position representation was refined. A first positioned-token design
regressed the 10,000-edge parser to roughly 15.75 ms and was discarded. The
final design keeps accepted-build tokens compact and reconstructs continued-
line locations only on errors. Criterion measured 5.615 ms (95% estimate
5.584-5.649 ms) at 10,000 edges, with no statistically significant change at
100 or 1,000 edges and a small improvement within the noise threshold at
10,000 compared with the immediately preceding optimized baseline.

Manifests with no `deps` or `depfile` bindings now bypass per-edge dependency
binding evaluation. On the 10,000-edge independent corpus, the instrumented
`dependency metadata` phase fell from 0.585 ms to 0.145 ms. Subsequent
1,000-sample process sweeps were run while the host showed large system-wide
tail-latency spikes; their medians still placed Knight within 0.7% of Ninja at
1,000 edges and 3.6-11.9% ahead at 10,000 edges, but the quieter baseline table
above remains the more representative full distribution.

Knight also resolves a 50,000-edge dependency chain iteratively. The reference
Windows Ninja binary terminated with stack-overflow status `0xC00000FD` at
5,000 and 10,000 chain edges in this environment, so those larger chain sizes
are tracked as robustness evidence rather than comparative timing rows.

Knight does not yet lead Ninja's median on every audited shape, and no measured
lead is close to an order of magnitude. A separate 1,000-run version-only sweep
measured 3.269 ms median for Ninja and 3.333 ms for Knight, showing that fresh
Windows process startup consumes most of a small no-op invocation before build
logic runs. The requested order-of-magnitude goal remains open.

## Generated dyndep concurrency

`scripts/benchmark-dyndep.ps1` measures a fresh `-j2` build in which a generated
dyndep file and an independent requested output each perform 500 ms of work.
The dyndep later connects that output to a consumer, so serial pre-generation
would put the two waits on the critical path. Fifty alternating Windows samples
after the concurrent-prebuild change produced:

| Tool | Median | Minimum | P95 |
| :--- | ---: | ---: | ---: |
| Ninja | 718.946 ms | 696.026 ms | 1768.000 ms |
| Knight | 727.980 ms | 708.263 ms | 1741.293 ms |

The synchronized differential test also requires both producers to be active
at once, so this is correctness evidence rather than a timing-only inference.
Knight is within 1.3% of Ninja's median and 1.5% ahead at P95 on this sample,
but does not yet win the median.

## Ready dyndep loading

`scripts/benchmark-dyndep-load.ps1` measures warm quiet no-op builds in which
every real output already exists and each edge owns a separate ready dyndep
file. This isolates file discovery, loading, parsing, ownership validation,
graph expansion, and final incremental planning. The runner warms each tool
three times and alternates launch order. The initial Knight implementation took
about 149 ms at 1,000 files; it redundantly probed every ready source dyndep and
ran generated-file prebuild planning before loading it.

Ready files now bypass prebuild planning, file sets use linear-time hash
deduplication, and batches are read and parsed on two workers while semantic
application and diagnostics remain in deterministic manifest order. On Windows
x64 the final interleaved sweeps produced:

| Files | Samples | Tool | Median | Minimum | P95 |
| ---: | ---: | :--- | ---: | ---: | ---: |
| 100 | 300 | Ninja | 8.347 ms | 7.559 ms | 51.145 ms |
| 100 | 300 | Knight | 8.306 ms | 7.372 ms | 12.094 ms |
| 1,000 | 100 | Ninja | 49.088 ms | 45.658 ms | 90.103 ms |
| 1,000 | 100 | Knight | 37.642 ms | 35.451 ms | 40.814 ms |
| 5,000 | 100 | Ninja | 218.313 ms | 213.903 ms | 261.165 ms |
| 5,000 | 100 | Knight | 171.362 ms | 163.698 ms | 181.009 ms |

Knight is essentially tied at 100 files, 23.3% faster at 1,000, and 21.5%
faster at 5,000. Its P95 is lower by 30.7-76.4% across all three sizes. This is
a substantial reversal of the discovered regression, though not an
order-of-magnitude result.

## Inputs tool

`scripts/benchmark-inputs.ps1` validates byte-equivalent `-t inputs all`
results after normalizing platform newlines, then measures fresh interleaved
processes. The graph uses groups of 100 phony nodes and makes 10% of leaf paths
require shell escaping, exercising traversal, deduplication, rendering,
sorting, and output. Knight buffers the result through a 64 KiB writer instead
of flushing the line-buffered stdout path once per input.

| Inputs | Samples | Tool | Median | Minimum | P95 |
| ---: | ---: | :--- | ---: | ---: | ---: |
| 1,000 | 300 | Ninja | 12.652 ms | 11.192 ms | 55.823 ms |
| 1,000 | 300 | Knight | 12.366 ms | 11.238 ms | 55.491 ms |
| 10,000 | 300 | Ninja | 18.016 ms | 15.595 ms | 65.199 ms |
| 10,000 | 300 | Knight | 15.862 ms | 14.353 ms | 61.237 ms |
| 50,000 | 50 | Ninja | 39.744 ms | 36.736 ms | 81.184 ms |
| 50,000 | 50 | Knight | 27.492 ms | 25.817 ms | 69.578 ms |

Knight's median lead grows from 2.3% at 1,000 inputs to 12.0% at 10,000 and
30.8% at 50,000. Its P95 is also lower at every size. This is a clear tool-path
win, but remains well short of the project-wide order-of-magnitude target.

## Status output

`scripts/benchmark-status.ps1` first requires newline-normalized output parity,
then measures a redirected `-n -j1` build whose complete status stream is
captured. Knight already evaluates dry runs before emitting their statuses, so
it writes the identical result in one batch rather than flushing the pipe once
per edge.

| Edges | Samples | Tool | Median | Minimum | P95 |
| ---: | ---: | :--- | ---: | ---: | ---: |
| 1,000 | 100 | Ninja | 38.984 ms | 36.693 ms | 80.963 ms |
| 1,000 | 100 | Knight | 14.449 ms | 12.734 ms | 57.055 ms |
| 10,000 | 30 | Ninja | 327.684 ms | 311.670 ms | 381.193 ms |
| 10,000 | 30 | Knight | 32.748 ms | 27.696 ms | 80.096 ms |
| 50,000 | 10 | Ninja | 3225.384 ms | 2748.777 ms | 7383.398 ms |
| 50,000 | 10 | Knight | 143.270 ms | 115.793 ms | 225.161 ms |

Knight is 2.7x faster at 1,000 redirected statuses, 10.0x at 10,000, and 22.5x
at 50,000. The latest larger rows include exact Ninja-compatible dry-run
frontier simulation: currently ready commands are started without a `-j`
limit, pretend completions are consumed FIFO, and phony edges still unlock work
immediately. The ten-sample 50,000-edge row remains exploratory because Ninja
makes larger sweeps expensive, but every measured sample preserves the same
wide separation.

After extending byte parity to Ninja's Windows CRLF text-mode output, a
100-sample 10,000-edge rerun produced byte-identical 427,784-byte streams and
measured Ninja at 335.206 ms median versus Knight at 35.080 ms (9.6x), with
94.645 ms Knight P95 versus 546.267 ms for Ninja. The Windows stat cache now
uses a successful directory enumeration as authoritative for absent entries,
avoiding 10,000 redundant missing-file probes while preserving individual-stat
fallback if enumeration itself fails.

`scripts/benchmark-status-pty.sh` measures the smart-terminal path through a
real Linux pseudo-terminal. Thirty alternating 1,000-edge samples, backed by a
byte-for-byte PTY differential test covering normal, quiet, verbose, custom
status, and dry-run modes, produced:

| Tool | Median | Minimum | P95 |
| :--- | ---: | ---: | ---: |
| Ninja | 998.706 ms | 857.916 ms | 1251.628 ms |
| Knight | 39.616 ms | 36.240 ms | 49.293 ms |

That is a 25.2x Knight median lead for this status-heavy terminal workload.
It is the first audited order-of-magnitude win, not yet evidence for the full
project-wide 10x requirement.

## Compilation database

`scripts/benchmark-compdb.ps1` requires newline-normalized, byte-identical
pretty JSON before timing fresh interleaved `-t compdb cc` processes. Each edge
has one source input, and the output sizes below include every JSON field and
indentation byte.

| Edges | Samples | Tool | Output | Median | Minimum | P95 |
| ---: | ---: | :--- | ---: | ---: | ---: | ---: |
| 1,000 | 200 | Ninja | 211,563 B | 18.011 ms | 12.667 ms | 63.527 ms |
| 1,000 | 200 | Knight | 211,563 B | 17.023 ms | 12.552 ms | 61.950 ms |
| 10,000 | 50 | Ninja | 2,165,563 B | 42.131 ms | 33.869 ms | 138.216 ms |
| 10,000 | 50 | Knight | 2,165,563 B | 31.899 ms | 28.204 ms | 78.461 ms |
| 50,000 | 10 | Ninja | 11,005,563 B | 140.070 ms | 130.669 ms | 169.900 ms |
| 50,000 | 10 | Knight | 11,005,563 B | 96.838 ms | 90.646 ms | 99.970 ms |

Knight's median lead is 5.5% at 1,000 edges, 24.3% at 10,000, and 30.9% at
50,000. Its 64 KiB output buffer keeps the exact Ninja representation while
reducing write overhead as databases grow.

## Phony chains

`scripts/benchmark-phony.ps1` validates normalized no-work output and measures
fresh interleaved traversals of commandless phony chains. The implementation
checks leaf source existence in one pass while relying on scheduler order for
producer-backed inputs, avoiding recursive rescans.

| Edges | Samples | Tool | Median | Minimum | P95 |
| ---: | ---: | :--- | ---: | ---: | ---: |
| 1,000 | 200 | Ninja | 12.917 ms | 11.378 ms | 62.089 ms |
| 1,000 | 200 | Knight | 12.979 ms | 12.034 ms | 61.033 ms |
| 2,000 | 100 | Ninja | 14.184 ms | 12.282 ms | 101.237 ms |
| 2,000 | 100 | Knight | 13.890 ms | 13.093 ms | 61.787 ms |

Knight is within 0.5% of Ninja's median at 1,000 edges and leads by 2.1% at
2,000; its P95 is lower in both rows and 39.0% lower at 2,000. A missing or
non-directory parent is now authoritative for every requested child in the
same stat-cache group. Before that fix, Knight redundantly issued 2,000
individual missing-child stats and measured about 37.6 ms median on the
2,000-edge corpus. The Windows Ninja reference crashes with status
`0xC0000005` at 10,000 phony
edges, so larger sizes are robustness rather than comparative evidence.
Knight completed a 100,000-edge chain in 181.161 ms median across ten samples
(176.589 ms minimum, 194.787 ms P95).

## Bounded-pool planning

`scripts/benchmark-pools.ps1` measures quiet dry-run planning for independent
three-edge chains whose final two edges share a depth-one pool. Root
completions fill the delayed queue, and each pooled completion both frees a
slot and reveals a newer same-pool dependent. This stresses critical-path
ranking, initial clean-frontier construction, and Ninja's temporal reservation
order without launching child commands. Output and exit status are checked before
the alternating timed samples.

| Chains | Edges | Samples | Tool | Median | Minimum | P95 |
| ---: | ---: | ---: | :--- | ---: | ---: | ---: |
| 1,000 | 3,000 | 100 | Ninja | 89.402 ms | 85.630 ms | 185.228 ms |
| 1,000 | 3,000 | 100 | Knight | 19.896 ms | 18.312 ms | 66.984 ms |
| 3,000 | 9,000 | 30 | Ninja | 240.462 ms | 235.908 ms | 338.845 ms |
| 3,000 | 9,000 | 30 | Knight | 35.824 ms | 33.816 ms | 81.684 ms |
| 10,000 | 30,000 | 50 | Ninja | 858.117 ms | 836.220 ms | 945.050 ms |
| 10,000 | 30,000 | 50 | Knight | 100.846 ms | 93.159 ms | 152.539 ms |

Knight is 4.5x, 6.7x, and 8.5x faster by median as the corpus grows, with
2.8x-6.2x lower P95. Quiet dry runs no longer materialize command,
description, or response-file expansions after an edge is already known dirty.
Instrumentation on the 30,000-edge case attributes about
19.3 ms to manifest parsing, 9.7 ms to filesystem stat, 7.5 ms to scheduler-
graph construction, and 47.4 ms to edge evaluation. This path is approaching,
but has not reached, the project-wide 10x requirement.
