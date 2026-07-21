# Benchmarks

Cross-tool benchmark comparing `tsr`'s per-invocation overhead against other task
runners. Task runners spend most of their wall-clock on *the work they launch* —
so to compare the **runners themselves**, we run tasks that do almost nothing and
measure what's left: process startup, config parsing, and the cost of getting to
the child command.

## What's measured

| Scenario | Task body | Isolates |
|----------|-----------|----------|
| `noop` | spawn `true` | Pure runner overhead — startup + config parse + spawn. |
| `hello` | `echo hello` | The same, with a trivial real command. |

The same two tasks are defined for every tool in [`workspace/`](workspace/):
[`tasks.toml`](workspace/tasks.toml) (tsr), [`package.json`](workspace/package.json)
(npm/bun), [`justfile`](workspace/justfile) (just),
[`Taskfile.yml`](workspace/Taskfile.yml) (go-task), and
[`Makefile`](workspace/Makefile) (make).

## Method

- Harness: [`hyperfine`](https://github.com/sharkdp/hyperfine) — statistical, with
  warmup runs and outlier detection.
- `--shell=none`: each runner is timed directly, not inside an extra wrapping
  shell, so the shell's own startup isn't charged to every tool equally.
- `--warmup 20 --min-runs 100`.

Why this is a fair comparison: every tool is asked to do the *same negligible
work*, so the delta between them is overhead. It is **not** a claim about build
performance — caching and incremental builds are explicitly delegated to
Turbo/Nx (see the [docs](../website/app/docs/page.mdx)), and are out of scope
here.

## Run it

```sh
benches/run.sh
```

The script benchmarks whichever runners are installed and writes
`results/noop.md` and `results/hello.md`. Install the comparison tools with:

```sh
cargo install hyperfine just
npm install -g @go-task/cli    # provides `task`
```

## Results

Measured on the reference machine (Linux x86-64, kernel 6.12; `tsr` release
build; hyperfine 1.20, `--shell=none`, 20 warmup runs). Your numbers will differ
— rerun `benches/run.sh` locally. Lower is faster. Raw exports:
[`results/noop.md`](results/noop.md), [`results/hello.md`](results/hello.md).

**`noop` — spawn `true` (sorted fastest → slowest):**

| Runner | Mean | Relative to `make` |
|--------|-----:|-------------------:|
| `make` | 1.4 ms | 1.00× |
| **`tsr`** | **1.6 ms** | **1.16×** |
| `just` | 2.0 ms | 1.43× |
| `bun run` | 2.4 ms | 1.78× |
| `npm run` | 85.6 ms | 62.6× |
| `task` (go-task) | 102.9 ms | 75.2× |

The `hello` scenario (`echo hello`) is within noise of the same ordering.

### Takeaway

`tsr` runs a metacharacter-free `run` string by splitting it and spawning the
child directly (`execvp`-style) — no language runtime to boot and no wrapping
shell. It lands essentially tied with the native runners (`make`, `just`) and a
touch ahead of `bun run`, while the Node-based `npm run` and go-task's `task`
pay ~85–100 ms of interpreter/startup on *every* invocation — roughly **60–75×**
slower. This "no startup tax" is exactly the common-path win `tsr` is designed
for.

> This harness earned its keep: the first run measured `tsr` at ~16 ms because a
> fixed 15 ms child-poll interval added a full tick to every fast task. That
> became [adaptive backoff](../src/exec.rs) (`POLL_MIN`/`POLL_MAX`), dropping the
> figure to ~1.6 ms.
