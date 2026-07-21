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
| `shell` | one task, `echo $HOME && echo done` | Shell support — `$VAR` expansion + `&&`, which tsr's mini-shell handles in-process. |
| `localbin` | one task calling `node_modules/.bin/localcli` (a Node script) | Local-binary resolution — the real `npm run` replacement case. `tsr`/`npm`/`bun` only (see below). |
| `steps5` | one task, 5 sequential commands | In-task sequencing — the runner launches **once**. |
| `graph5` | one task with 5 trivial dependencies | Dependency-graph overhead. |
| `graph10` | one task with 10 trivial dependencies | Graph overhead, scaled — shows it grow linearly. |

The `shell` scenario exercises the mini-shell tsr supports natively (`$VAR`,
`&&`/`||`/`;`, quoting). Pipes, redirects, globs and command substitution are
**not** in the mini-shell — for those, tsr users reach for a `delegate` to
`sh -c` or a script file, so they aren't part of this comparison.

The `localbin` scenario resolves a binary from `node_modules/.bin` — the lookup
`tsr`, `npm`, and `bun` perform but `just`/`make`/go-task do not — so it compares
only those three. The stand-in binary is a trivial Node script, because the real
tools it represents (`vite`, `eslint`) are Node programs; every runner pays Node's
startup once, and the delta is the runner's own overhead on top.

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

| Runner | `startup` | `shell` | `steps5` | `graph5` | `graph10` |
|--------|----------:|--------:|---------:|---------:|----------:|
| `make` | 1.4 | 1.5 | 3.1 | 3.3 | 5.6 |
| **`tsr`** | **1.7** | **2.5** | **5.0** | **5.2** | **9.5** |
| `just` | 2.1 | 2.1 | 4.1 | 4.1 | 7.0 |
| `bun` | 2.4 | 2.5 | 2.5 | 12.2 | 25.7 |
| `task` (go-task) | 105.0 | 106.0 | 110.0 | 114.5 | 116.0 |
| `npm` | 87.6 | 89.0 | 89.9 | 452.1 | 900.9 |

**`localbin` — calling a local `node_modules/.bin` tool** (tsr/npm/bun only):
`bun` 20.1 ms · **`tsr` 27.5 ms** · `npm` 105.3 ms. Calling a project-local Node
tool (`vite`/`eslint`), `tsr` is **~3.8× faster than `npm run`** and on par with
`bun` — it resolves the same `node_modules/.bin` binary but skips npm's extra Node
startup.

`startup`/`shell`: `tsr` sits with the native runners and ~60× ahead of npm/task.
On the `shell` one-liner it's a touch slower than `make`/`just` because it spawns
each command as a real process while a shell runs `echo` as a builtin — the win
lands when the commands are real programs, not builtins.

The graph columns tell the bigger story. `tsr`, `just`, and `make` resolve the
whole graph in one launch, so they stay in the low single-digit milliseconds.
`npm` has no graph: chaining `npm run` per task multiplies its ~88 ms startup,
reaching **~901 ms for ten no-op tasks (≈161× the fastest)**. `bun` chains too but
from a cheaper startup (~26 ms). `go-task` *does* resolve its graph in-process, so
it stays flat at its ~110 ms startup rather than multiplying — slow to start, but
it doesn't compound.

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
