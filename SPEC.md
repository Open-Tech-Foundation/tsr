# tsr — v1 Specification

`tsr` is a lightweight, polyglot, repo-aware task runner. It is a **command runner**, not a build system: it provides one unified interface over the native runners already in a repo (`npm`, `bun`, `cargo`, `go`, …), adds a task dependency graph and opt-in parallelism, and delegates caching to specialist tools (Turbo, Nx) rather than reimplementing it.

- **Binary:** `tsr`
- **Config file:** `tasks.toml` (also the workspace root anchor)
- **Written in:** Rust, single static binary
- **Parsing:** `toml_edit` (preserves comments and unknown keys on round-trip)

```
tsr dev            # run the 'dev' task
tsr ci             # run the 'ci' task
tsr test -- --watch
```

---

## 1. Design principles

1. **Lightweight** — a thin unifying layer, not a replacement for native runners.
2. **Delegate, don't reimplement** — execution is handed to native runners; caching is handed to Turbo/Nx.
3. **Polyglot** — one entry point across every ecosystem in the repo.
4. **Predictable by default** — sequential execution unless parallelism is explicitly requested; fail fast.
5. **TUI-primary, hand-edit-safe** — the config is intended to be edited via tooling, but must stay valid and legible when edited by hand, and unknown keys must survive a round-trip.

---

## 2. Workspace & config file

`tasks.toml` at the workspace root serves two purposes: it holds the config, and its location defines the workspace root. Root detection walks up from the current directory to the nearest `tasks.toml`.

```toml
[workspace]
members = ["apps/*", "packages/*"]   # monorepo globs; omit entirely for a single-package repo

[env]
NODE_ENV = "development"
```

- `members` — glob patterns identifying the packages in a monorepo. Omit for a single-package repo.
- `[env]` — workspace-wide environment variables inherited by every task (see §7).

### 2.1 Configless mode

`tasks.toml` is **optional**. When none is found (walking up from the current directory), `tsr <task>` still runs repo-aware by treating `<task>` as a bare form-3 auto-detect task (§3.1) anchored at the nearest directory holding an ecosystem marker (`package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml`). So in a plain npm repo, `tsr dev` runs `npm run dev`; in a Cargo crate, `tsr build` runs `cargo build` — no config to write.

- Argument passthrough (§6) works as usual: `tsr test -- --watch`.
- Package-qualified names (`web#build`) and the dependency graph require a `tasks.toml`; configless mode is single-task only.
- When neither a `tasks.toml` nor any ecosystem marker exists, `tsr <task>` is a runner error (exit `64`) with a message pointing at `tsr --init`.
- A present `tasks.toml` always takes precedence — there is no fall-through from a defined config to auto-detection, so a mistyped task name is still an error, not a silent `npm run <typo>`.

---

## 3. Tasks

Each task is a `[tasks.<name>]` table. A task takes one of three **forms**, resolved by precedence.

### 3.1 Resolution precedence

When `tsr <task>` runs, the task's form is chosen in this order:

1. **`delegate` present** → hand off to a backend: `<bin> run <task>`.
2. **`run` present** → spawn the command directly (no `npm`/`node` startup tax).
3. **Neither** → auto-detect the package's ecosystem and map the task name to its native runner (`npm run <task>`, `cargo <task>`, `go <task>`, …).

Form 3 is the core "wrapper" behaviour: a bare `[tasks.test]` just works across a polyglot repo with no per-package config.

### 3.2 Form examples

```toml
# Form 2 — direct spawn (the `npm run` replacement)
[tasks.dev]
run = "vite"
dir = "apps/web"          # optional; defaults to workspace root
args = ["--host"]         # prepended before CLI passthrough (see §6)

# Form 3 — auto-detect + native runner, fanned out across packages
[tasks.test]
packages = ["apps/*", "packages/*"]

# Form 1 — delegate caching to a specialist
[tasks.build]
delegate = "turbo"        # → `turbo run build`

# Form 1 — delegate to a non-conforming binary (full control)
[tasks.bundle]
delegate = { bin = "make", args = ["bundle"] }

# Graph + parallelism
[tasks.ci]
deps = ["lint", "test", "build"]
parallel = true
env = { CI = "true" }

# Explicit cross-package dependency
[tasks."web#build"]
run = "vite build"
dir = "apps/web"
deps = ["ui#build"]
```

### 3.3 Task fields

| Field | Type | Meaning |
|-------|------|---------|
| `run` | string | Command to spawn directly (form 2). |
| `delegate` | string \| table | Backend to hand off to (form 1). String → `<bin> run <task>`. Table → `{ bin = "...", args = [...] }`. |
| `dir` | string | Directory to run in. Defaults to workspace root. Mutually exclusive with `packages`. |
| `packages` | array | Fan out across matching packages (globs or exact names). Mutually exclusive with `dir`. |
| `deps` | array | Tasks that must run before this one (the dependency graph). |
| `parallel` | bool | Run `deps` / `packages` concurrently. Default `false` (sequential). |
| `args` | array | Default args prepended to the resolved command, before CLI passthrough. |
| `env` | table | Per-task env; overrides `[env]` (see §7). |
| `env_file` | string \| array | `.env`-style file(s) to load for this task (see §7.2). Listed order = increasing precedence (later overrides earlier). |

`dir` and `packages` are mutually exclusive; setting both is a config error (exit `64`).

---

## 4. Symbols & task-name grammar

### 4.1 Task-name grammar

Legal task-name characters: `[a-zA-Z0-9_-:]+` — letters, digits, `_`, `-`, `:`.

- `:` is an **ordinary name character** with no meaning to the parser. It exists so that ecosystem-conventional names like `build:prod` or `test:watch` are legal and `package.json` scripts import 1:1 without renaming.
- Reserved (never legal inside a name): `#`, `^`, whitespace.

### 4.2 Symbols

| Symbol | Meaning | Example |
|--------|---------|---------|
| `#` | Package↔task separator: run this exact task in this named package. | `web#build`, `web#build:prod` |
| `^` | Upstream marker: run this task in the package's dependencies first (v1.1). | `^build` |
| `*` | Glob wildcard in `members` / `packages`. | `apps/*` |

Parsing rule: split on `#` first (package vs task), then the task portion may freely contain `:`. `web#build:update` → package `web`, task `build:update`. `^test:watch` → task `test:watch` in upstream deps.

`:` is now permanently a literal and cannot be reclaimed as an operator in future versions.

---

## 5. Dependency graph & execution order

- `deps` lists the tasks that must complete before a task runs — these edges form the DAG.
- **Explicit cross-package edges** (`pkg#task`, e.g. `ui#build`) ship in **v1**; they require no graph inference.
- **Topological edges** (`^task`) are deferred to **v1.1**, because resolving "upstream dependencies" requires reading each package's manifest to build the dependency graph. See §9.

### 5.1 Parallelism

Execution is **sequential by default**. Concurrency is opt-in via `parallel = true`, and the rule is uniform:

- `deps` list → runs one at a time unless `parallel = true`.
- `packages` fan-out → runs one at a time unless `parallel = true`.

There are no exceptions: nothing runs concurrently unless a task explicitly sets `parallel = true`. This keeps default behaviour predictable and race-free.

### 5.2 Failure handling (fail-fast)

On any failure within a task's batch, `tsr` **fails fast**: it stops launching new work and kills still-running siblings, then prints a summary and exits.

```
✗ ci failed

  ✓ lint     ok        1.2s
  ✗ test     exit 1    3.4s   ← failed
  ⊘ build    skipped          (killed: sibling failed)

exit code: 1
```

In a parallel batch, "the failure" is whichever child exits non-zero first in wall-clock time; this is non-deterministic across runs and is expected. Fail-fast guarantees at most one failing child's code is reported.

---

## 6. Argument passthrough

Everything after `--` on the CLI is forwarded to the resolved command:

```
tsr test -- --watch
```

If the task defines `args`, they are prepended **before** the CLI passthrough:

```toml
[tasks.test]
run = "vitest"
args = ["--color"]
```

`tsr test -- --watch` → `vitest --color --watch`.

### 6.1 CLI surface

```
tsr <task> [-- <args>...]   run a task; args after -- are forwarded
tsr --list                  list the tasks defined in tasks.toml
tsr --config                edit tasks.toml in an interactive TUI
tsr --init                  create a starter tasks.toml here
tsr --help | --version
```

The **first positional argument is always a task name**. Every builtin is a flag
(`--list`, `--config`, `--init`, `--help`, `--version`), never a bare subcommand,
so a task named `list` or `init` is never shadowed — `tsr list` runs the user's
`list` task. This keeps the entire bare-word namespace available for
tasks/scripts, which is the point of the tool.

`--config` opens a TUI for authoring tasks with every option (form, `dir`/
`packages`, `deps`, `parallel`, `args`, `env`, `env_file`). It opens on a menu of
workflows (add / edit / delegate / delete / graph), not a bare list. It edits
through the `toml_edit` document, so comments and unknown keys survive (§1.5),
and validates each change before writing. Changes autosave — a committed form or
delete is written immediately, so there is no unsaved state; since validation
precedes the commit, an autosave never writes a broken config. It also offers a
read-only graph/dry-run preview.

---

## 7. Environment variables

### 7.1 Sources & precedence

Sources, merged (never replaced — a task's `env` adds to and overrides the inherited set, it does not wipe `PATH` etc.). Highest wins:

```
task env  >  task env_file(s)  >  workspace [env]  >  root .env file  >  process env
```

### 7.2 `.env` loading

- Only the **workspace-root** `.env` is auto-loaded (next to `tasks.toml`) — no flag.
- Per-package `.env` files are **not** auto-loaded. This is by design: frameworks (Next, Vite, …) load their own app-level `.env` at runtime; `tsr` owns only the shared, workspace-level vars.

#### `env_file` (per-task)

A task may declare additional `.env`-style files to load, as a string or an array:

```toml
[tasks.test]
run = "vitest"
env_file = [".env.local", ".env.test"]   # loaded in order; .env.test overrides .env.local
```

- **Resolution:** paths are relative to the task's `dir` (or the workspace root when `dir` is unset).
- **Precedence:** `env_file` values layer **above** the root `.env` and workspace `[env]`, and **below** the inline task `env`. So `env_file` is how a task overrides the default `.env` (e.g. `.env.test` for a test task).
- **Order:** the list is applied left-to-right; **later files override earlier** ones. A single string is equivalent to a one-element list.
- **Missing files are skipped** (like the root `.env`), so an optional `.env.local` need not exist — handy in CI. Values honour §7.3 expansion.

### 7.3 Expansion

- `$VAR` / `${VAR}` are expanded by the mini-shell (see §8), **after** the full merge, against the final resolved env.
- `[env]` values may reference process env and **earlier** keys in the same block. No forward references; no dependency graph for env resolution.
- A referenced-but-**undefined** `$VAR` in a `run` string is a **hard error**, caught at load time where possible, exit `64`:

```
✗ config error: task 'deploy'
  run = "deploy --target $TARGET"
                          ^^^^^^^
  '$TARGET' is not defined in task env, env_file, workspace [env], or .env

exit code: 64
```

### 7.4 Process env

Fully inherited; no filtering / allow-listing in v1.

---

## 8. `run` execution & the mini-shell

`run` strings execute one of two ways, chosen by scanning for shell metacharacters:

1. **No metacharacters** → the string is split and the command is spawned **directly** (`execvp`-style). Fast, fully cross-platform. This is the common path and where `tsr` beats `npm run` (no Node startup tax).
2. **Supported metacharacters present** → the string runs through `tsr`'s own **minimal shell**.

### 8.1 Mini-shell — supported (the entire feature set)

- **`$VAR` / `${VAR}`** — expansion from the merged env (§7).
- **`&&` `||` `;`** — sequencing with correct exit-code semantics (`&&` on `0`, `||` on non-zero, `;` always).
- **Quoting** — `'single'` (literal, no expansion) and `"double"` (expansion applies).

### 8.2 Mini-shell — rejected (never attempted)

These are rejected at **load time** with a clear, specific error (exit `64`), because they require OS-level plumbing outside the tool's scope:

| Construct | Message points to |
|-----------|-------------------|
| `\|` pipes | use `delegate` or a script file |
| `>` `>>` `2>&1` redirection | use a script file |
| `*` `?` `[...]` globs | pass an explicit path |
| `$(...)` / backtick substitution | use a script file |

### 8.3 Escape hatch

When a `run` string needs real shell power, opt in explicitly:

```toml
[tasks.pipeline]
delegate = { bin = "sh", args = ["-c", "cat x | grep y > z"] }
```

…or point `run` at a script file: `run = "./scripts/build.sh"`.

### 8.4 Detection order

`scan → no metachars → direct spawn` · `has supported-only metachars → mini-shell` · `has any unsupported metachar → error 64 at load`. Metacharacter *detection* always runs before *rejection*, so metachar-free strings never touch the mini-shell.

---

## 9. Detection layer

- **v1** — detect each package's **ecosystem** (via marker files: `package.json` → npm/bun, `Cargo.toml` → cargo, `go.mod` → go, `pyproject.toml` → uv/poetry) and its **manifest name** (so `packages` can match against names like `@scope/pkg`, not just path globs).
- **v1.1** — additionally read **dependency edges** from each manifest (path/workspace deps) to build the package dependency graph that `^task`, affected-detection, and cross-package ordering require.

### 9.1 `packages` matching

`packages` entries match against **either** a path glob (`apps/*`) **or** an exact manifest name (`@opentf/workeros-web`). Matching against manifest names is what allows faithful conversion of `bun run --filter <name>` style scripts.

### 9.2 Local binary resolution (`node_modules/.bin`)

For `run = "vite"` to genuinely replace `npm run dev`, a directly-spawned command must resolve **locally-installed** binaries. Before spawning, `tsr` prepends `node_modules/.bin` to `PATH` — collected by walking up from the task's working directory to the workspace root (inclusive), **nearest first**, so a package's own `.bin` wins over a hoisted root one. This is the same lookup npm/bun/yarn/pnpm perform.

Only existing directories are added, so it is a no-op in non-JS packages. The command itself is still spawned directly (`execvp`-style) — this only fixes *where* the binary is found, and pays no Node startup tax.

---

## 10. Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| *child's code* | On task failure, the first failed child's **exact** exit code is propagated verbatim (`1`, `2`, `130`, …), so CI sees the real signal. |
| `64` | **Runner-level** error: config parse failure, `dir`+`packages` both set, unknown task name, `delegate` binary not found, undefined `$VAR`, rejected mini-shell metacharacter. |

The distinction lets pipelines tell "my task failed" (child code) apart from "the runner itself broke" (`64`).

---

## 11. v1 / v1.1 boundary

| Capability | v1 | v1.1 |
|-----------|:--:|:--:|
| `run` (direct spawn) + mini-shell | ✓ | |
| `delegate` (string + table forms) | ✓ | |
| Auto-detect ecosystem → native runner | ✓ | |
| `packages` fan-out (glob + name match) | ✓ | |
| Explicit cross-package deps (`pkg#task`) | ✓ | |
| Opt-in `parallel`, fail-fast | ✓ | |
| Env model + root `.env` | ✓ | |
| Package **dependency graph** | | ✓ |
| Topological deps (`^task`) | | ✓ |
| Affected / changed detection | | ✓ |

The arrival of the dependency graph *is* what defines v1.1 as "the monorepo release." v1 stays deliberately graph-free (beyond explicit `pkg#task` edges) to remain lightweight.

### Explicitly out of scope (delegated, not built)

Content-hash caching, remote caching, and inputs/outputs tracking are **never** implemented in `tsr` — they are ceded to delegated backends (Turbo, Nx). Adding them would contradict the "lightweight, delegate" principle.

---

## Appendix A — Example `tasks.toml`

```toml
# tasks.toml — workspace root anchor + config
# Task names: [a-zA-Z0-9_-:]+  |  '#' = pkg#task  |  '^' = upstream (v1.1)

[workspace]
members = ["apps/*", "packages/*"]

[env]
NODE_ENV = "development"

[tasks.dev]
run = "vite"
dir = "apps/web"
args = ["--host"]

[tasks.test]
packages = ["apps/*", "packages/*"]

[tasks.build]
delegate = "turbo"

[tasks.bundle]
delegate = { bin = "make", args = ["bundle"] }

[tasks.ci]
deps = ["lint", "test", "build"]
parallel = true
env = { CI = "true" }

[tasks."web#build"]
run = "vite build"
dir = "apps/web"
deps = ["ui#build"]
```

## Appendix B — Converting existing scripts

A Bun workspace script:

```json
"build:update": "bun run --filter '@opentf/workeros-programs' --filter '@opentf/workeros-coreutils' --filter '@opentf/workeros-web' build"
```

becomes:

```toml
[tasks.build:update]
packages = [
  "@opentf/workeros-programs",
  "@opentf/workeros-coreutils",
  "@opentf/workeros-web",
]
```

The `packages` list matches manifest names; form-3 auto-detection resolves `build` to each package's native runner. Add `parallel = true` to fan out concurrently.
