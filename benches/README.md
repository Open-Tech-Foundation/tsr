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
| `builtins` | one task, `mkdir` + `touch` + `cp` + `mv` + `rm` | In-process coreutils — file ops executed in-process without sub-process spawning. |
| `globbing` | one task, `echo src/*.rs` | In-process globbing — file pattern matching resolved relative to task `dir`. |
| `localbin` | one task calling `node_modules/.bin/localcli` (a Node script) | Local-binary resolution — the real `npm run` replacement case. `tsr`/`npm`/`bun`/`deno` only (see below). |
| `steps5` | one task, 5 sequential commands | In-task sequencing — the runner launches **once**. |
| `graph5` | one task with 5 trivial dependencies | Dependency-graph overhead. |
| `graph10` | one task with 10 trivial dependencies | Graph overhead, scaled — shows it grow linearly. |

The `builtins` and `globbing` scenarios exercise `tsr`'s in-process Rust coreutils and glob expansion engine (`rm`, `cp`, `mv`, `mkdir`, `touch`, `cat`, `echo`, `pwd`, `*`, `?`, `[...]`, `**`). Other runners invoke external sub-processes or shell delegation, while `tsr` resolves them directly in-process.

The `localbin` scenario resolves a binary from `node_modules/.bin` — the lookup
`tsr`, `npm`, `bun`, and `deno` perform but `just`/`make`/go-task/`mise` do not — so it
compares only those four. The stand-in binary is a trivial Node script, because
the real tools it represents (`vite`, `eslint`) are Node programs; every runner
pays Node's startup once, and the delta is the runner's own overhead on top.

The task definitions are generated for every tool by
[`gen-workspace.sh`](gen-workspace.sh) into [`workspace/`](workspace/):
[`tasks.toml`](workspace/tasks.toml) (tsr), [`package.json`](workspace/package.json)
(npm/bun), [`deno.json`](workspace/deno.json) (deno), [`justfile`](workspace/justfile) (just),
[`Taskfile.yml`](workspace/Taskfile.yml) (go-task), [`Makefile`](workspace/Makefile)
(make), and [`mise.toml`](workspace/mise.toml) (mise).

`tsr`, `just`, go-task, `make`, and `mise` express a dependency graph natively —
one launch resolves the whole graph. `npm`, `bun`, and `deno` have **no** dependency graph,
so the graph scenarios chain the tasks with `&&` (`npm run s1 && npm run s2 && …`),
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
curl https://mise.run | sh     # provides `mise`
```

Results are written to `results/<scenario>.{md,json}`. The website's benchmark
page loads the JSON via `website/tools/sync-bench.mjs`.

## Results

Measured on the reference machine (Linux x86-64, kernel 6.12; `tsr` release
build; hyperfine 1.20). Your numbers will differ — rerun `benches/run.sh`. Lower
is faster; `×` is relative to the fastest runner in that scenario. Raw exports:
[`results/`](results/).

Mean wall-clock, in milliseconds:

| Runner | `startup` | `shell` | `builtins` | `globbing` | `steps5` | `graph5` | `graph10` |
|--------|----------:|--------:|-----------:|-----------:|---------:|---------:|----------:|
| **`tsr`** | **0.9** | **0.9** | **1.1** | **1.0** | **0.9** | **1.0** | **1.1** |
| `make` | 1.6 | 1.6 | 5.3 | 1.6 | 3.4 | 3.4 | 5.9 |
| `just` | 2.3 | 2.3 | 5.9 | 2.3 | 4.4 | 4.5 | 7.2 |
| `bun` | 2.9 | 2.8 | 7.2 | 2.8 | 2.8 | 14.0 | 28.3 |
| `deno` | 7.1 | 7.1 | 8.6 | 7.1 | 7.3 | 34.0 | 68.9 |
| `mise` | 21.1 | 21.2 | 24.7 | 21.3 | 26.4 | 36.1 | 55.2 |
| `npm` | 89.7 | 89.8 | 93.0 | 90.3 | 89.7 | 441.0 | 881.6 |
| `task` (go-task) | 104.3 | 104.2 | 108.6 | 104.0 | 108.5 | 109.3 | 116.6 |

**`localbin` — calling a local `node_modules/.bin` tool** (tsr/npm/bun/deno only): `bun` 21.3 ms · **`tsr` 28.2 ms** · `deno` 31.8 ms · `npm` 107.8 ms. Calling a project-local Node tool (`vite`/`eslint`), `tsr` is **~3.8× faster than `npm run`** — it resolves the same `node_modules/.bin` binary but skips npm's extra Node startup.

`startup`/`shell`: `tsr` sits with the native runners and ~50–60× ahead of
npm/task; `mise` lands in between (~20 ms — a Rust binary, but it does more at
startup). On the `shell` one-liner `tsr` is a touch slower than `make`/`just`
because it spawns each command as a real process while a shell runs `echo` as a
builtin — the win lands when the commands are real programs, not builtins.

The graph columns tell the bigger story. `tsr`, `just`, `make`, and `mise` resolve
the whole graph in one launch, so their cost grows gently with graph size. `npm`
has no graph: chaining `npm run` per task multiplies its ~84 ms startup, reaching
**~843 ms for ten no-op tasks (≈164× the fastest)**. `bun` chains too but from a
cheaper startup (~23 ms). `go-task` also resolves its graph in-process but from a
~100 ms startup, so it stays roughly flat — slow to start, but it doesn't compound.

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
