#!/usr/bin/env bash
set -euo pipefail

if (( $# < 2 || $# > 4 )); then
  echo "usage: $0 NINJA KNIGHT [EDGES] [ITERATIONS]" >&2
  exit 2
fi

ninja=$(realpath "$1")
knight=$(realpath "$2")
edges=${3:-1000}
iterations=${4:-30}
root=$(cd "$(dirname "$0")/.." && pwd)
work="$root/target/differential-benchmark-status-pty-$edges"
mkdir -p "$work"

{
  printf 'rule touch\n  command = : > $out\n'
  for ((edge = 0; edge < edges; edge++)); do
    printf 'build out/%d: touch\n' "$edge"
  done
  printf 'build all: phony'
  for ((edge = 0; edge < edges; edge++)); do
    printf ' out/%d' "$edge"
  done
  printf '\ndefault all\n'
} > "$work/build.ninja"

declare -a ninja_samples knight_samples

measure() {
  local executable=$1 start end
  start=$(date +%s%N)
  TERM=xterm script -qfec "$executable -n -j1 all" /dev/null >/dev/null
  end=$(date +%s%N)
  printf '%d' $(((end - start) / 1000))
}

cd "$work"
for ((iteration = 0; iteration < iterations; iteration++)); do
  if ((iteration % 2 == 0)); then
    ninja_samples+=("$(measure "$ninja")")
    knight_samples+=("$(measure "$knight")")
  else
    knight_samples+=("$(measure "$knight")")
    ninja_samples+=("$(measure "$ninja")")
  fi
done

summarize() {
  local name=$1
  shift
  local -a sorted
  mapfile -t sorted < <(printf '%s\n' "$@" | sort -n)
  local count=${#sorted[@]}
  local p95=$((count * 95 / 100))
  awk -v name="$name" -v count="$count" \
    -v median="${sorted[$((count / 2))]}" -v minimum="${sorted[0]}" \
    -v p95="${sorted[$p95]}" \
    'BEGIN { printf "%s samples=%d median=%.3fms min=%.3fms p95=%.3fms\n", name, count, median / 1000, minimum / 1000, p95 / 1000 }'
}

summarize ninja "${ninja_samples[@]}"
summarize knight "${knight_samples[@]}"
