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

| Runner | `startup` | `shell` | `steps5` | `graph5` | `graph10` |
|--------|----------:|--------:|---------:|---------:|----------:|
| `make` | 1.4 | 1.4 | 3.0 | 3.1 | 5.1 |
| **`tsr`** | **1.6** | **2.5** | **5.0** | **5.1** | **9.6** |
| `just` | 1.9 | 1.9 | 3.8 | 3.8 | 6.1 |
| `bun` | 2.3 | 2.3 | 2.3 | 11.5 | 22.7 |
| `mise` | 19.7 | 19.7 | 24.2 | 32.2 | 45.8 |
| `task` (go-task) | 99.5 | 99.8 | 103.6 | 104.7 | 107.4 |
| `npm` | 83.9 | 84.0 | 83.4 | 416.8 | 843.1 |

**`localbin` — calling a local `node_modules/.bin` tool** (tsr/npm/bun only —
just/make/go-task/mise don't resolve project-local binaries): `bun` 19.7 ms ·
**`tsr` 27.5 ms** · `npm` 100.2 ms. Calling a project-local Node tool
(`vite`/`eslint`), `tsr` is **~3.6× faster than `npm run`** and near `bun` — it
resolves the same `node_modules/.bin` binary but skips npm's extra Node startup.

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
