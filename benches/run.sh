#!/usr/bin/env bash
# Cross-tool benchmark for `tsr` — measures per-invocation runner overhead by
# running a task that does almost nothing (`true`) and a trivial command
# (`echo hello`) through each task runner.
#
# Compared: tsr · npm · bun · just · task (go-task) · make
# Harness:  hyperfine (statistical, with warmup). Commands run with --shell=none
#           so we time each runner, not an extra wrapping shell.
#
# Usage:  benches/run.sh            # benchmarks every tool that is installed
#         TSR=/path/to/tsr benches/run.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$here/workspace"

TSR="${TSR:-$repo/target/release/tsr}"
if [[ ! -x "$TSR" ]]; then
  echo "building tsr (release)…" >&2
  (cd "$repo" && cargo build --release --quiet)
fi

if ! command -v hyperfine >/dev/null 2>&1; then
  echo "error: hyperfine is required (cargo install hyperfine)" >&2
  exit 1
fi

# Build the hyperfine argument list from whichever runners are present, so the
# benchmark degrades gracefully on a machine missing some tools.
build_args() {
  local task="$1"
  args=()
  args+=(-n "tsr"  "$TSR $task")
  command -v npm  >/dev/null 2>&1 && args+=(-n "npm"  "npm run --silent $task")
  command -v bun  >/dev/null 2>&1 && args+=(-n "bun"  "bun run $task")
  command -v just >/dev/null 2>&1 && args+=(-n "just" "just $task")
  command -v task >/dev/null 2>&1 && args+=(-n "task" "task $task")
  command -v make >/dev/null 2>&1 && args+=(-n "make" "make $task")
}

mkdir -p "$here/results"
for scenario in noop hello; do
  echo "== scenario: $scenario ==" >&2
  build_args "$scenario"
  hyperfine \
    --shell=none \
    --warmup 20 \
    --min-runs 100 \
    --export-markdown "$here/results/$scenario.md" \
    --export-json "$here/results/$scenario.json" \
    "${args[@]}"
done

echo "results written to benches/results/{noop,hello}.md" >&2
