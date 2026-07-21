# Benchmarks

Cross-tool benchmark comparing `tsr` against other task runners. Task runners
spend most of their wall-clock on *the work they launch* — so to compare the
**runners themselves**, we run tasks that do almost nothing and measure what's
left: process startup, config parsing, and the cost of reaching the child
commands.

## Scenarios

| Scenario | Shape | Isolates |
|----------|-------|----------|
| `startup` | one task, spawns `true` | Pure per-invocation overhead. |
| `steps5` | one task, 5 sequential commands | In-task sequencing — the runner launches **once**. |
| `graph5` | one task with 5 trivial dependencies | Dependency-graph overhead. |
| `graph10` | one task with 10 trivial dependencies | Graph overhead, scaled — shows it grow linearly. |

The task definitions are generated for every tool by
[`gen-workspace.sh`](gen-workspace.sh) into [`workspace/`](workspace/):
[`tasks.toml`](workspace/tasks.toml) (tsr), [`package.json`](workspace/package.json)
(npm/bun), [`justfile`](workspace/justfile) (just),
[`Taskfile.yml`](workspace/Taskfile.yml) (go-task), and
[`Makefile`](workspace/Makefile) (make).

`tsr`, `just`, go-task, and `make` express a dependency graph natively — one
launch resolves the whole graph. `npm` and `bun` have **no** dependency graph, so
the graph scenarios chain the tasks with `&&` (`npm run s1 && npm run s2 && …`),
exactly as their users do — which is why the per-invocation cost compounds for
them. That contrast is the point of the benchmark, not a handicap.

## Method

- Harness: [`hyperfine`](https://github.com/sharkdp/hyperfine) — statistical, with
  warmup and outlier detection. `--warmup 20 --min-runs 80`.
- `startup`/`steps5` run with `--shell=none` (each runner timed directly). The
  graph scenarios use a shell because the npm/bun variants are `&&` chains; the
  constant shell cost applies to every command equally.

It is **not** a claim about build performance — caching and incremental builds
are explicitly delegated to Turbo/Nx (see the [docs](../website/app/docs/page.mdx)),
and are out of scope here.

## Run it

```sh
benches/gen-workspace.sh    # (re)generate the per-tool task definitions
benches/run.sh              # benchmark whichever runners are installed
```

Install the comparison tools with:

```sh
cargo install hyperfine just
npm install -g @go-task/cli    # provides `task`
```

Results are written to `results/<scenario>.{md,json}`. The website's benchmark
page loads the JSON via `website/tools/sync-bench.mjs`.

## Results

Measured on the reference machine (Linux x86-64, kernel 6.12; `tsr` release
build; hyperfine 1.20). Your numbers will differ — rerun `benches/run.sh`. Lower
is faster; `×` is relative to the fastest runner in that scenario. Raw exports:
[`results/`](results/).

Mean wall-clock, in milliseconds:

| Runner | `startup` | `steps5` | `graph5` | `graph10` |
|--------|----------:|---------:|---------:|----------:|
| `make` | 1.5 | 3.2 | 3.3 | 6.0 |
| **`tsr`** | **1.7** | **5.0** | **5.1** | **9.4** |
| `just` | 2.2 | 4.2 | 4.2 | 6.9 |
| `bun` | 2.6 | 2.6 | 13.4 | 26.5 |
| `task` (go-task) | 111.9 | 122.7 | 122.5 | 134.0 |
| `npm` | 94.6 | 94.6 | 490.9 | 947.7 |

The graph columns tell the story. `tsr`, `just`, and `make` resolve the whole
graph in one launch, so they stay in the low single-digit milliseconds. `npm` has
no graph: chaining `npm run` per task multiplies its ~95 ms startup, reaching
**~948 ms for ten no-op tasks (≈158× the fastest)**. `bun` chains too but from a
cheaper startup (~26 ms). `go-task` *does* resolve its graph in-process, so it
stays flat at its ~120 ms startup rather than multiplying — slow to start, but it
doesn't compound.

Exact tables: [`results/startup.md`](results/startup.md),
[`results/steps5.md`](results/steps5.md), [`results/graph5.md`](results/graph5.md),
[`results/graph10.md`](results/graph10.md).

### Takeaway

For a single task, `tsr` spawns the child directly (`execvp`-style) — no language
runtime, no wrapping shell — so it sits with the native runners (`make`, `just`)
and well ahead of `npm run`. The gap **compounds across a dependency graph**:
`tsr` resolves the whole graph in one process, so its cost stays flat while a
chained `npm`/`bun` pays its startup once per task. That is the case `tsr` is
built for.

> This harness earned its keep on the first run: `tsr` measured ~16 ms because a
> fixed 15 ms child-poll interval added a full tick to every fast task. That
> became [adaptive backoff](../src/exec.rs) (`POLL_MIN`/`POLL_MAX`), dropping it
> to ~1.6 ms.
