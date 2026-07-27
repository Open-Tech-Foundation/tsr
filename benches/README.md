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
| `localbin` | one task calling `node_modules/.bin/localcli` (a Node script) | Local-binary resolution — the real `npm run` replacement case. `tsr`/`npm`/`bun`/`nub`/`deno` only (see below). |
| `steps5` | one task, 5 sequential commands | In-task sequencing — the runner launches **once**. |
| `graph5` | one task with 5 trivial dependencies | Dependency-graph overhead. |
| `graph10` | one task with 10 trivial dependencies | Graph overhead, scaled — shows it grow linearly. |
| `topo10/50/200` | N packages in a dependency chain, `deps = ["^build"]` | Topological fan-out across a real workspace — package-graph construction + ordered dispatch. `tsr`/`pnpm`/`bun` only (see below). |

The `builtins` and `globbing` scenarios exercise `tsr`'s in-process Rust coreutils and glob expansion engine (`rm`, `cp`, `mv`, `mkdir`, `touch`, `cat`, `echo`, `pwd`, `*`, `?`, `[...]`, `**`). Other runners invoke external sub-processes or shell delegation, while `tsr` resolves them directly in-process.

The `localbin` scenario resolves a binary from `node_modules/.bin` — the lookup
`tsr`, `npm`, `bun`, `nub`, and `deno` perform but `just`/`make`/go-task/`mise` do not — so it
compares only those tools. The stand-in binary is a trivial Node script, because
the real tools it represents (`vite`, `eslint`) are Node programs; every runner
pays Node's startup once, and the delta is the runner's own overhead on top.

The task definitions are generated for every tool by
[`gen-workspace.sh`](gen-workspace.sh) into [`workspace/`](workspace/):
[`tasks.toml`](workspace/tasks.toml) (tsr), [`package.json`](workspace/package.json)
(npm/bun/nub), [`deno.json`](workspace/deno.json) (deno), [`justfile`](workspace/justfile) (just),
[`Taskfile.yml`](workspace/Taskfile.yml) (go-task), [`Makefile`](workspace/Makefile)
(make), and [`mise.toml`](workspace/mise.toml) (mise).

The `topo` scenarios use a different shape and live in `topo/<N>/` — real
multi-package workspaces (a root `package.json` + `pnpm-workspace.yaml` +
`tasks.toml`, and `N` package manifests). They are generated too, but *not*
committed: ~270 files across the three sizes. Run `gen-workspace.sh` to create
them.

`tsr`, `just`, go-task, `make`, and `mise` express a dependency graph natively —
one launch resolves the whole graph. `npm`, `bun`, `nub`, and `deno` have **no** dependency graph,
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
npm install -g pnpm            # topo scenarios
curl https://mise.run | sh     # provides `mise`
```

Results are written to `results/<scenario>.{md,json}`. The website's benchmark
page loads the JSON via `website/tools/sync-bench.mjs`.

## Results

Measured on the reference machine (Linux x86-64, kernel 6.12; `tsr` release
build; hyperfine 1.20; npm 11.9, bun 1.4, nub 0.6, deno 2.8, just 1.57, go-task
3.52, make 4.4, mise 2026.7). Your numbers will differ — rerun `benches/run.sh`.
Lower is faster. Raw exports: [`results/`](results/).

Mean wall-clock, in milliseconds:

| Runner | `startup` | `shell` | `builtins` | `globbing` | `steps5` | `graph5` | `graph10` |
|--------|----------:|--------:|-----------:|-----------:|---------:|---------:|----------:|
| **`tsr`** | **0.8** | **0.8** | **1.0** | **0.9** | **0.8** | **0.9** | **1.0** |
| `make` | 1.4 | 1.4 | 4.6 | 1.4 | 3.2 | 3.3 | 5.3 |
| `just` | 2.0 | 2.0 | 5.2 | 2.0 | 4.0 | 4.0 | 6.3 |
| `bun` | 2.6 | 2.4 | 6.2 | 2.4 | 2.4 | 12.1 | 25.0 |
| `deno` | 6.5 | 6.2 | 7.5 | 6.2 | 6.4 | 30.5 | 60.4 |
| `nub` | 8.3 | 7.8 | 11.2 | 7.9 | 7.9 | 39.3 | 79.4 |
| `mise` | 19.7 | 19.7 | 23.2 | 20.0 | 25.0 | 32.7 | 48.6 |
| `npm` | 85.6 | 83.8 | 87.6 | 84.1 | 83.5 | 418.5 | 851.7 |
| `task` (go-task) | 100.1 | 99.5 | 103.3 | 100.7 | 105.8 | 107.7 | 110.8 |

**`localbin` — calling a local `node_modules/.bin` tool** (tsr/npm/bun/nub/deno only): `bun` 20.7 ms · **`tsr` 27.6 ms** · `deno` 28.9 ms · `nub` 63.1 ms · `npm` 102.1 ms. Calling a project-local Node tool (`vite`/`eslint`), `tsr` is **~3.7× faster than `npm run`** — it resolves the same `node_modules/.bin` binary but skips npm's extra Node startup. This is the one scenario `tsr` does not lead: once Node's own startup dominates, `bun` reaches the tool first.

`startup`/`shell`: `tsr` sits with the native runners and ~105–120× ahead of
npm/task; `mise` lands in between (~20 ms — a Rust binary, but it does more at
startup). On the `shell` one-liner `tsr` is a touch slower than `make`/`just`
because it spawns each command as a real process while a shell runs `echo` as a
builtin — the win lands when the commands are real programs, not builtins.

The graph columns tell the bigger story. `tsr`, `just`, `make`, and `mise` resolve
the whole graph in one launch, so their cost grows gently with graph size. `npm`
has no graph: chaining `npm run` per task multiplies its ~86 ms startup, reaching
**~852 ms for ten no-op tasks (≈864× `tsr`)**. `bun`, `nub`, and `deno` chain too, but
from much cheaper startups, so ten tasks cost them ~25 ms, ~79 ms and ~60 ms.
`go-task` also resolves its graph in-process but from a ~100 ms startup, so it
stays roughly flat — slow to start, but it doesn't compound.

### Topological fan-out (`^task`)

A real multi-package workspace: `N` packages in a dependency chain, each with a
no-op `build`. Only three runners are compared, because only three actually order
packages by dependency — `tsr` (`deps = ["^build"]`), `pnpm` (`-r`) and `bun`
(`--filter`). The chain is built so the correct order is the **reverse** of
alphabetical, which is how that was verified.

| Runner | `topo10` | `topo50` | `topo200` |
|--------|---------:|---------:|----------:|
| **`tsr`** | **1.4** | **3.7** | **11.9** |
| `bun` | 10.9 | 45.9 | 177.7 |
| `pnpm` | 169.5 | 297.2 | 633.0 |

`tsr`'s marginal cost is **~56–62 µs per package** and *falls* slightly as the
workspace grows (62.4 → 58.1 → 55.6 µs), so reading every package manifest to
build the graph stays off the critical path — the per-package cost is dominated
by spawning the child, not by graph construction.

The two ratios move in opposite directions, which is worth stating plainly:
against `pnpm`, `tsr`'s lead **narrows** with scale (118× → 80× → 53×) as pnpm
amortises its large fixed startup; against `bun` it **widens** (7.6× → 12.3× →
14.9×), because bun's per-package overhead is the larger term.

**Not compared: Turbo and Nx.** They exist to cache, which `tsr` delegates rather
than reimplements (SPEC §11). A no-op benchmark would time them either cold (where
`tsr` "wins" only by having no cache to populate) or warm (where they "win" only by
skipping the work) — neither number would mean anything. **Also not compared:
`npm --workspaces`**, which walks packages in *name* order, not dependency order;
it isn't doing this job, so including it would show it losing a race it never entered.

Exact tables, one per scenario: [`startup`](results/startup.md) ·
[`shell`](results/shell.md) · [`builtins`](results/builtins.md) ·
[`globbing`](results/globbing.md) · [`localbin`](results/localbin.md) ·
[`steps5`](results/steps5.md) · [`graph5`](results/graph5.md) ·
[`graph10`](results/graph10.md) · [`topo10`](results/topo10.md) ·
[`topo50`](results/topo50.md) · [`topo200`](results/topo200.md).

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
