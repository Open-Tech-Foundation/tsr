# tsr

A lightweight, polyglot, repo-aware task runner. It is a **command runner**, not
a build system: one unified interface over the native runners already in a repo
(`npm`, `bun`, `cargo`, `go`, `uv`, …), plus a task dependency graph and opt-in
parallelism. Caching is delegated to specialist tools (Turbo, Nx), never
reimplemented.

See [`SPEC.md`](SPEC.md) for the full v1 specification.

## Install

```sh
cargo build --release   # binary at target/release/tsr
```

## Usage

```sh
tsr <task>              # run a task
tsr test -- --watch     # forward args after -- to the resolved command
tsr --list              # list the tasks defined in tasks.toml
tsr --config            # edit tasks.toml in an interactive TUI
tsr --init              # scaffold a starter tasks.toml
tsr --help
```

`tsr` finds the workspace root by walking up to the nearest `tasks.toml`. The
first argument is always a task name — every builtin is a flag, so a task named
`list` or `init` is never shadowed.

**No config required.** `tasks.toml` is optional: in a repo that only has a
`package.json` (or `Cargo.toml`, `go.mod`, `pyproject.toml`), `tsr dev` runs
`npm run dev` / `cargo dev` / … by auto-detecting the ecosystem. Add a
`tasks.toml` when you want a dependency graph, monorepo fan-out, or `delegate`; a
present config always takes precedence over auto-detection.

## `tasks.toml`

```toml
[workspace]
members = ["apps/*", "packages/*"]   # omit for a single-package repo

[env]
NODE_ENV = "development"

# Form 2 — direct spawn (the `npm run` replacement, no Node startup tax)
[tasks.dev]
run = "vite"
dir = "apps/web"
args = ["--host"]

# Form 3 — auto-detect each package's ecosystem → native runner, fanned out
[tasks.test]
packages = ["apps/*", "packages/*"]

# Form 1 — delegate (e.g. hand caching to a specialist)
[tasks.build]
delegate = "turbo"                    # → `turbo run build`

[tasks.bundle]
delegate = { bin = "make", args = ["bundle"] }

# Dependency graph + opt-in parallelism
[tasks.ci]
deps = ["lint", "test", "build"]
parallel = true
env = { CI = "true" }
```

### Task resolution precedence

1. `delegate` → `<bin> run <task>` (string) or `{ bin, args }` (table).
2. `run` → spawn the command directly.
3. Neither → auto-detect the package's ecosystem and map the task name to its
   native runner (`npm run <task>`, `cargo <task>`, `go <task>`, `uv run <task>`).

### `run` strings

A `run` string with no shell metacharacters is split and spawned directly. If it
uses supported metacharacters it goes through a built-in mini-shell:

- `$VAR` / `${VAR}` expansion (against the merged env)
- `&&` `||` `;` sequencing
- `'single'` (literal) and `"double"` (expanding) quotes

Unsupported constructs — pipes `|`, redirection `>`/`<`, globs `*`/`?`/`[…]`,
command substitution `$(…)`/backticks — are rejected at load time. For real
shell power, use `delegate = { bin = "sh", args = ["-c", "…"] }` or a script file.

### Environment

Four sources are merged (lower sources augmented, never wiped); highest wins:

```
task env  >  workspace [env]  >  root .env file  >  process env
```

Only the workspace-root `.env` is auto-loaded. A `$VAR` referenced in a `run`
string but defined nowhere is a hard error.

## Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| *child's code* | A task's child failed; its exact code is propagated. |
| `64` | Runner-level error (bad config, undefined `$VAR`, rejected metacharacter, unknown task, dependency cycle, missing delegate binary, …). |

## Development

```sh
cargo test          # unit + end-to-end tests
cargo clippy --all-targets
cargo fmt
```
