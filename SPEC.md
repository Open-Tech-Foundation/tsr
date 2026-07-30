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
| `deps` | array | Tasks that must run before this one (the dependency graph). Accepts `task`, `pkg#task`, and `^task` (§5.0). |
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
| `^` | Upstream marker: run this task in the package's dependencies first (§5.0). Requires `packages`. | `^build` |
| `*` | Glob wildcard in `members` / `packages`. | `apps/*` |

Parsing rule: split on `#` first (package vs task), then the task portion may freely contain `:`. `web#build:update` → package `web`, task `build:update`. `^test:watch` → task `test:watch` in upstream deps.

`:` is now permanently a literal and cannot be reclaimed as an operator in future versions.

---

## 5. Dependency graph & execution order

- `deps` lists the tasks that must complete before a task runs — these edges form the DAG.
- **Explicit cross-package edges** (`pkg#task`, e.g. `ui#build`) ship in **v1**; they require no graph inference.
- **Topological edges** (`^task`) resolve against the package dependency graph (§9).

### 5.0 Topological edges (`^task`)

`^name` inside `deps` means: *before this task runs in a package, run task `name` in every package that package depends on.*

```toml
[tasks.build]
packages = ["apps/*", "packages/*"]
deps = ["^build"]
```

- **`packages` is required.** `^` is relative to "the package this is running in", and only a fan-out supplies one. On a task that runs once in a single directory it is a config error (exit `64`).
- **Upstream packages are visited even when the pattern did not select them.** Building `apps/*` builds the libraries those apps import; that is the entire point of the marker.
- **`^name` may differ from the task's own name** (`deps = ["^codegen"]`), in which case `name` is looked up as an ordinary task and run in each upstream package.
- Each `(task, package)` pair runs **at most once**, so a library shared by several dependents is built once.
- Ordinary `deps` alongside `^` deps still run once, globally, before the fan-out.
- A **cycle in the package graph** is a runner error (exit `64`): no valid order exists, and silently choosing one would be wrong. Note that package graphs are not otherwise required to be acyclic — the error is raised only when an order is actually needed.

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

**`--no-bail`** opts out: every batch runs to completion, siblings are neither skipped nor killed, and the summary lists every failure. The propagated exit code is still the **first** failing child's, so CI sees the same signal either way. This is the flag for "tell me everything that is broken", not just the first thing.

`--no-bail` covers **task** failures only. A runner-level error (§10) still stops the run, because it means `tsr` itself could not proceed — a missing `delegate` binary will be missing for every package, so continuing would only repeat the same error.

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
tsr <task> --since <ref>    run only in packages affected since a git ref
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

The options below may follow a task name. Every one is a flag, never a bare word,
so none shadows anything; anything else after a task name is still the "forward
args after `--`" error.

| Option | Meaning |
|--------|---------|
| `--since <ref>` | Run only in packages affected by changes since a git ref (§9.3). |
| `--resume-from <pkg>` | Skip every package ordered before `pkg` (§9.4). |
| `--no-bail` | Run every batch to completion instead of stopping at the first failure (§5.2). |
| `--dry-run` | Print the plan and run nothing (§12.5). |
| `--allow-unsafe-env` | Let the config set the guarded environment variables (§12.2). |
| `--reporter <fmt>` | Terminal format: `human` (default) or `ndjson` (§6.2). |
| `--reporter-file <path>` | Also write the NDJSON stream to `path` (§6.2). |

Each accepts both `--flag value` and `--flag=value`.

### 6.2 Reporters

There are two independent sinks. `--reporter` chooses what the **terminal** gets;
`--reporter-file <path>` additionally writes the NDJSON stream to a file. Either
may be used alone, and `--reporter-file` on its own is the common case: a human
summary on the terminal *and* a machine-readable record.

| Reporter | Terminal output |
|----------|-----------------|
| `human` *(default)* | Nothing on success; a result table on failure (§5.2). |
| `ndjson` | One JSON object per line on **stderr**, always — success included. |

**Only `--reporter-file` produces a parseable stream.** Child processes inherit
stdio, so anything written to stdout or stderr shares the stream with their
output — and a child that logs JSON to stderr (pino, `jest --json`, `tracing`)
emits lines indistinguishable from reporter events, including ones carrying a
`type` field. Filtering by "is this line JSON?" is therefore *not* sufficient.
`--reporter ndjson` is for reading; `--reporter-file` is for scripting, because
nothing else writes to that file.

A failure to create the file is reported **before any task runs** (exit `64`) —
discovering the sink is unwritable after a long build would be useless.

Both sinks emit a `task` event as each unit of work finishes, then one `summary`
event:

```json
{"durationMs":12.4,"exitCode":null,"label":"build (packages/ui)","status":"ok","type":"task"}
{"durationMs":48.9,"exitCode":1,"failed":1,"ok":3,"runnerError":null,"skipped":2,"status":"failed","task":"build","type":"summary"}
```

`status` is one of `ok`, `failed`, `skipped`. `exitCode` is `null` unless the
unit failed.

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

1. **Every word static** → the string is split and the command is spawned **directly** (`execvp`-style). Fast, fully cross-platform. This is the common path and where `tsr` beats `npm run` (no Node startup tax).
2. **Variables, globs or operators present** → the string runs through `tsr`'s own **minimal shell**.

A `run` string is parsed into an **AST** (program → command → word → part), not split straight into argv. Keeping the structure is what lets expansion tell text that came from an unquoted literal — where `*` is a pattern — from text that came from quotes or a variable, where it is just a character.

### 8.1 Mini-shell — supported (the entire feature set)

- **`$VAR` / `${VAR}`** — expansion from the merged env (§7). `${...}` takes a plain variable name only; parameter expansion (`${VAR:-default}`, `${#VAR}`, …) is rejected with a specific error.
- **`&&` `||` `;`** — sequencing with correct exit-code semantics (`&&` on `0`, `||` on non-zero, `;` always).
- **Quoting** — `'single'` (literal, no expansion) and `"double"` (expansion applies). Quoted text is never globbed.
- **Globs** — `*`, `?`, `[...]` and `**` match against the filesystem, relative to the task's `dir` (§3.2), following `sh` rules: `*` does not cross a `/` and does not match a leading dot, while `**` spans directories (including zero of them, so `a/**/*.js` matches `a/x.js`); case sensitivity follows the platform. A pattern that matches nothing stays literal, exactly as in `sh`. Matches are returned with `/` separators on every platform, including Windows, so one `run` string produces one argv everywhere.

Two rules keep globbing predictable:

- **Expanded values are never rescanned.** If `$FILES` holds `*.js`, it stays the literal string `*.js`. Only the `run` string itself can contain a pattern.
- **Globs resolve when their command runs**, not when the task is planned — so in `build && rm dist/*.map` the pattern sees the files `build` just produced.

Variables, by contrast, resolve **before** the sequence starts, so an undefined `$VAR` fails the task cleanly instead of half-way through (§7.3).

### 8.2 Mini-shell — rejected (never attempted)

These are rejected at **load time** with a clear, specific error (exit `64`), because they require OS-level plumbing outside the tool's scope:

| Construct | Message points to |
|-----------|-------------------|
| `\|` pipes | use `delegate` or a script file |
| `>` `>>` `2>&1` redirection | use a script file |
| `$(...)` / backtick substitution | use a script file |
| `&` background, `( )` subshells | use `delegate` for real shell control |
| `{a,b}` brace expansion | list the paths explicitly, or quote the braces |

### 8.3 Escape hatch

When a `run` string needs real shell power, opt in explicitly:

```toml
[tasks.pipeline]
delegate = { bin = "sh", args = ["-c", "cat x | grep y > z"] }
```

…or point `run` at a script file: `run = "./scripts/build.sh"`.

### 8.4 Detection order

`parse → every word static → direct spawn` · `variables, globs or operators present → mini-shell` · `any unsupported construct → error 64 at load`. Classification is a property of the parsed AST, so a plain string never touches expansion, and quoting alone (`echo 'a b'`) still takes the direct path.

### 8.5 Built-in commands

`rm -rf dist` has to mean the same thing on Linux, macOS and Windows — but the coreutils it names are Unix binaries that do not exist on Windows. So `tsr` implements the file operations tasks actually use, in-process:

| Builtin | Options |
|---------|---------|
| `rm` | `-r`/`-R`/`--recursive`, `-f`/`--force` |
| `cp` | `-r`/`-R`/`--recursive` |
| `mv` | — |
| `mkdir` | `-p`/`--parents` |
| `touch` | — |
| `cat` | — (stdin when no operands) |
| `echo` | leading `-n` |
| `pwd` | — |
| `true` / `false` | — (operands ignored) |

- **A builtin always wins** over a binary of the same name on `PATH`, on every platform. One `run` string, one behaviour. `delegate` is the escape hatch when a task genuinely needs the platform's own tool (§8.3).
- Builtins apply to **`run` strings only** — never to `delegate` or an auto-detected native runner (§3.1).
- Short options bundle (`-rf`), `--` ends option parsing, and relative paths resolve against the task's `dir`.
- `true` and `false` exist so `cmd || true` — the standard "this step must not fail the build" idiom — works on Windows, where there is no `/bin/true` to fall back on.
- **Exit codes:** `0` success, `1` a failed operation, `2` a usage error (unknown flag, missing operand). As in POSIX, `rm -f` with nothing to remove succeeds silently — which is what makes `rm -rf dist/*` idempotent once `dist` is empty.

---

## 9. Detection layer

- **v1** — detect each package's **ecosystem** (via marker files: `package.json` → npm/bun, `Cargo.toml` → cargo, `go.mod` → go, `pyproject.toml` → uv/poetry) and its **manifest name** (so `packages` can match against names like `@scope/pkg`, not just path globs).
- **v1.1** — additionally read **dependency edges** from each manifest to build the package dependency graph that `^task`, affected-detection, and cross-package ordering require.

One rule spans every ecosystem: an edge exists exactly when a **declared dependency name matches another workspace package's manifest name**. Version specifiers and protocols are never inspected, so `workspace:*`, `path = "../ui"`, `replace` directives and plain ranges all resolve identically, and external registry dependencies drop out because nothing matches them.

| Ecosystem | Dependency fields read |
|-----------|------------------------|
| npm / bun | `dependencies`, `devDependencies`, `peerDependencies`, `optionalDependencies` |
| cargo | `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, the same three under any `[target.<cfg>]`, following `package = "…"` renames and resolving `{ workspace = true }` against the workspace root |
| go | `require` and `replace`, single-line and block form |
| python | PEP 621 `[project]` + optional groups, PEP 735 `[dependency-groups]`, Poetry's tables |

A malformed or absent manifest contributes no edges rather than failing discovery: the native runner reports a broken manifest far better than `tsr` could.

### 9.1 `packages` matching

`packages` entries match against **either** a path glob (`apps/*`) **or** an exact manifest name (`@opentf/workeros-web`). Matching against manifest names is what allows faithful conversion of `bun run --filter <name>` style scripts.

### 9.2 Local binary resolution (`node_modules/.bin`)

For `run = "vite"` to genuinely replace `npm run dev`, a directly-spawned command must resolve **locally-installed** binaries. Before spawning, `tsr` prepends `node_modules/.bin` to `PATH` — collected by walking up from the task's working directory to the workspace root (inclusive), **nearest first**, so a package's own `.bin` wins over a hoisted root one. This is the same lookup npm/bun/yarn/pnpm perform.

Only existing directories are added, so it is a no-op in non-JS packages. The command itself is still spawned directly (`execvp`-style) — this only fixes *where* the binary is found, and pays no Node startup tax.

On **Windows** the name also has to be resolved against `PATHEXT`, because the tools this targets are batch shims — `npm` is `npm.cmd`, and `node_modules/.bin` holds `vite.cmd`. `tsr` searches the job's `PATH` in order, trying each `PATHEXT` extension for a bare name (a name written with an extension, or any path with a separator, is used as given). Without this a bare `npm` or `vite` cannot be spawned at all, since the OS appends only `.exe` when searching.


### 9.3 Affected / changed detection (`--since <ref>`)

`tsr <task> --since <ref>` restricts every `packages` fan-out to the packages **affected** by changes since `ref`.

Affected = the packages the changed files live in, **plus every package that transitively depends on them**. Changing a library selects its dependents, because those are exactly the ones whose result could differ. The reverse does not hold: changing an app does not select the libraries it consumes.

- **Changes are read from git**: committed changes, unstaged edits, *and* untracked files (a brand-new package exists only as untracked files). Any git failure — not a repository, unknown ref, git missing — is a runner error (exit `64`) rather than a silent full or empty run.
- **A changed file outside every package widens to everything.** A root `tasks.toml`, lockfile, CI workflow or shared config could affect any package, so the selection is not narrowed at all. Running too much costs time; skipping work that should have run is a correctness failure.
- **Only the selection narrows — never the upstream.** `^task` still builds a package's dependencies whether or not they changed, so a filtered run stays correct rather than merely fast.
- **A pattern that matches packages, none of them affected, is a clean no-op** (exit `0`), not an error. An unmatched *pattern* remains an error (§9.1) because that is a typo.

### 9.4 Resuming (`--resume-from <pkg>`)

`tsr <task> --resume-from <pkg>` treats every package **ordered before `pkg`** in the workspace's topological order as already built, and runs the rest. It is the "the run died two-thirds of the way through; carry on from there" flag.

- `pkg` is matched the way `packages` entries are (§9.1): relative path **or** manifest name.
- The skipped prefix stays skipped even when a later package reaches it as an `^task` **upstream** dependency — otherwise the resume would rebuild the very prefix it was told to skip.
- The resume point itself runs; everything that depends on it runs.
- A `pkg` that matches no package is a runner error (exit `64`) — a typo would otherwise silently skip everything or nothing.
- A cyclic package graph is a runner error: resuming is only meaningful along a real build order.

`--since` and `--resume-from` compose; a package must survive **both** filters to be selected.
---

## 10. Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success. |
| *child's code* | On task failure, the first failed child's **exact** exit code is propagated verbatim (`1`, `2`, `130`, …), so CI sees the real signal. |
| `64` | **Runner-level** error: config parse failure, `dir`+`packages` both set, unknown task name, `delegate` binary not found, undefined `$VAR`, rejected mini-shell metacharacter, or a rejected security guard (§12). A failing **builtin** (§8.5) is a task failure, not a runner error, so it propagates its own `1`/`2`. |
| `130` | The run was **interrupted** (Ctrl-C / `SIGTERM`, §12.4). Outranks whatever a killed child reported: the run ended because the user stopped it. |

The distinction lets pipelines tell "my task failed" (child code) apart from "the runner itself broke" (`64`).

---

## 11. v1 / v1.1 boundary

| Capability | v1 | v1.1 |
|-----------|:--:|:--:|
| `run` (direct spawn) + mini-shell | ✓ | |
| Globbing + cross-platform builtins (§8.5) | ✓ | |
| `delegate` (string + table forms) | ✓ | |
| Auto-detect ecosystem → native runner | ✓ | |
| `packages` fan-out (glob + name match) | ✓ | |
| Explicit cross-package deps (`pkg#task`) | ✓ | |
| Opt-in `parallel`, fail-fast | ✓ | |
| Env model + root `.env` | ✓ | |
| Package **dependency graph** (§9) | | ✓ |
| Topological deps (`^task`, §5.0) | | ✓ |
| Affected / changed detection (§9.3) | | ✓ |

The arrival of the dependency graph *is* what defines v1.1 as "the monorepo release." v1 stays deliberately graph-free (beyond explicit `pkg#task` edges) to remain lightweight.

**All three v1.1 capabilities are implemented.**

### Explicitly out of scope (delegated, not built)

Content-hash caching, remote caching, and inputs/outputs tracking are **never** implemented in `tsr` — they are ceded to delegated backends (Turbo, Nx). Adding them would contradict the "lightweight, delegate" principle.

---

## 12. Security model

### 12.0 The trust boundary

Running `tsr build` in a repository is **running that repository's code**, exactly as `npm run build` or `make` is. `tsr` does not sandbox the programs it spawns, and it never will: a task runner that stopped `cargo` from writing outside the repo would have stopped being a task runner.

What `tsr` *does* guard is the part with no process boundary around it — **the things `tsr` performs itself**:

| `tsr` does this itself | So it is guarded |
|---|---|
| In-process builtins (§8.5) — there is no `/bin/rm` to audit or deny | §12.1 workspace confinement |
| Builds the environment every child inherits | §12.2 guarded variables |
| Chooses which `tasks.toml` gets to run commands | §12.3 discovery boundary |
| Owns the lifetime of every process it spawns | §12.4 process-tree containment |

Two tiers, because they defend against different things:

- **Config-relaxable guards** (§12.1) defend against *accidents* — a stale `dir`, a glob that reaches further than intended. A `tasks.toml` may widen them, so they are no defence against a config you do not trust.
- **CLI-only guards** (§12.2) exist precisely for the case where the config *is* what you are wary of. Nothing in `tasks.toml` can lift them.

Anything a guard rejects is a runner-level error: exit `64`, before the first child is spawned.

### 12.1 Workspace confinement

Every path `tsr` resolves itself must stay inside the workspace — the directory holding `tasks.toml`.

- **Builtin operands** (`rm`, `cp`, `mv`, `mkdir`, `touch`, `cat`) are refused when they resolve outside it. This is the guard that matters most: a builtin is `tsr` itself, always preferred over a binary of the same name (§8.5), so there is no `PATH`, sandbox or audit log to fall back on.
- **`dir`, `env_file`, `packages` and `workspace.members`** are rejected at load time. A glob is judged by its literal prefix — `apps/*` cannot escape `apps/`, `../*` has already left.

Resolution is **physical**: the longest existing prefix of a path is canonicalized, so a symlink inside the workspace pointing out of it is out of it. Only a not-yet-created tail is joined lexically, where no symlink can remain.

The check follows the operation, not just the operand. `cp -r` and `mv` walk a tree, and a symlink found *inside* one is a second way out — `fs::copy` would follow it — so every link met on the walk is checked in its own right. `rm -r` does not follow directory symlinks at all. `env_file` is re-checked when it is read, not only when it is validated, so a link created in between is not followed.

The escape hatch is a config key, for builds that genuinely write outside their tree:

```toml
[security]
allow_paths = ["../shared-cache", "/tmp/build"]
```

### 12.2 Guarded environment variables

A **config** may not set a variable whose only purpose is to decide what code some *other* program loads:

| Group | Variables |
|---|---|
| Dynamic-loader injection | `LD_PRELOAD`, `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`, `DYLD_LIBRARY_PATH` |
| Interpreter startup hooks | `NODE_OPTIONS`, `BASH_ENV`, `PYTHONSTARTUP`, `PERL5OPT`, `RUBYOPT`, `PHP_INI_SCAN_DIR` |
| JVM injection | `JAVA_TOOL_OPTIONS`, `JDK_JAVA_OPTIONS`, `_JAVA_OPTIONS` |
| Module search paths | `PYTHONPATH`, `PERL5LIB`, `RUBYLIB` |
| Toolchain flags naming a program to run | `GOFLAGS` (`-toolexec`), `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER` |
| Programs git & ssh shell out to | `GIT_SSH`, `GIT_SSH_COMMAND`, `GIT_EXTERNAL_DIFF`, `GIT_PROXY_COMMAND`, `SSH_ASKPASS`, `SUDO_ASKPASS` |
| `tsr`'s own namespace | anything prefixed `TSR_` |

Without this, a `tasks.toml` — or a committed `.env` — that appears to run `cargo test` can execute arbitrary code inside an unrelated process. Names are compared the way the platform compares them: exactly on unix, case-insensitively on Windows.

**The list is not exhaustive, and cannot be.** Every toolchain ships some way to make its compiler or interpreter load extra code, and new ones arrive with new tools. Variables that are *commonly* set on purpose (`CC`, `CLASSPATH`, `GOPATH`, `PYTHONHOME`) are deliberately left out: a guard that fires on ordinary configuration gets switched off wholesale, which is worse than not having it. Treat §12.2 as a guard against the well-known vectors, not as a boundary.

`PATH` is not banned, since extending it is ordinary. Two rules apply instead:

1. The value must still reference `$PATH`, so it augments rather than replaces — the same "merged, never wiped" rule the env model already follows (§7.1).
2. No entry may be **empty** or a bare `.`. Both are read as the working directory by every shell, so they put whatever directory a task happens to run in ahead of the real `PATH`. An explicit relative entry is fine: the objection is to the invisible form, since `":$PATH"` looks like nothing in a diff.

```toml
[env]
PATH = "./bin:$PATH"   # fine — written out
PATH = "/only/mine"    # rejected — replaces
PATH = ":$PATH"        # rejected — the empty entry is the working directory
```

**Scope.** Only config-supplied sources are checked — `[env]`, task `env`, `env_file`, and the root `.env` — over the tasks that will actually run. The **process** environment is passed through untouched: it belongs to whoever invoked `tsr`, and a runner that refused the environment it was given would be broken rather than safe.

**Opt-in:** `--allow-unsafe-env`, a CLI flag and deliberately not a `[security]` key.

### 12.3 Discovery boundary

Which `tasks.toml` is found decides what gets to run commands, so the upward walk (§2) is bounded. It stops at the first of:

- the **repository root** — a directory holding `.git`, checked after that directory itself, since a workspace anchored at the repo root is the norm;
- the user's **home directory**;
- a **filesystem boundary**, the rule `git` applies to its own discovery.

Without this, a `tasks.toml` left in `/tmp` or in a home directory silently governs every project beneath it.

A config that is **world-writable**, or that sits in a world-writable non-sticky directory, is refused (unix): anyone on the machine could otherwise choose what `tsr` runs. The same check covers the root `.env` and every `env_file` a reachable task loads — those set the environment each child inherits, so whoever can write one chooses what the build sees. Only *writability* is checked, never readability: a world-readable `.env` is what `umask 022` produces, and failing on it would fire on nearly every repo. Group-writable is accepted — a `umask` of `002` is a common default and rejecting it would fire on ordinary checkouts. Ownership is not checked, for the same reason git's "dubious ownership" is a recurring nuisance; a file another user owns is only reachable through a directory they can write to, which the above already catches.

### 12.4 Process-tree containment

Killing the process `tsr` spawned is not the same as stopping the work: `npm run dev` is a launcher whose `vite` keeps the port. A child that a run may have to kill is therefore spawned into its own **process group** (unix) or **job object** (windows), and the group is what is torn down — `SIGTERM`, then `SIGKILL` after a 2s grace.

Isolation is withheld in exactly one case, because on unix it costs interactivity: a process group outside the terminal's foreground one is stopped by `SIGTTIN` the moment it reads stdin. Both of these must hold for it to be withheld:

- **stdin is a terminal.** Under CI, a pipe, or `< /dev/null` there is no foreground group to be outside of, so isolation costs nothing and is always applied.
- **No parallelism.** Nothing in a sequential run can abort a child that is already running, because there is no sibling to fail — so there is no kill to reach a tree with.

A lone interactive `tsr dev` therefore keeps the inherited group; everything else, including that same run under CI, is contained.

**Interrupts.** `SIGINT`/`SIGTERM` (and `CTRL_C_EVENT` on Windows) abort the run through the same path a failure uses: stop launching, tear down what is running, exit `130` (§10). `--no-bail` does not override it — the user asked to stop. A second interrupt exits immediately, so a wedged child cannot trap the terminal.

### 12.5 `--dry-run`

`tsr <task> --dry-run` walks the dependency graph and prints each leaf's label, directory and command without running anything — the way to read an unfamiliar `tasks.toml` before handing it a shell.

Commands print **as written**, before `$VAR` expansion, so a plan pasted into an issue or a CI log cannot carry what `.env` supplied. The walk is always sequential, even for `parallel = true` batches, so the output is readable. A config that cannot be resolved still fails: a dry run reports the same errors a real one would.

### 12.6 Not guarded

Stated plainly, so the boundary is not mistaken for more than it is:

- **Spawned programs.** Once a child starts it has the user's full privileges. Use a container or a sandbox if that is not acceptable.
- **`delegate` and `run` targets.** Naming a binary is the feature; `tsr` does not decide which binaries are allowed.
- **`node_modules/.bin` on `PATH`** (§9.2). A repo-local binary shadowing a global one is what npm does and what makes `run = "vite"` work.
- **Secrets in a child's output.** Children inherit stdio; what they print is theirs. (`tsr` itself never prints an environment *value* — `--dry-run` prints commands before expansion, and no reporter event carries env — so there is nothing for it to mask.)
- **Resource exhaustion.** There are no CPU/memory/fd limits on a task. `RLIMIT`s would be config-declared, so they would be no defence against a config that simply omits them; use `systemd-run`, `ulimit` or a container.
- **A local attacker racing the run.** The path checks resolve, then act; they are not TOCTOU-hardened. Someone who can create symlinks inside your workspace while a build runs already controls the repository.
- **`[security] allow_paths` against a hostile config.** It is a config key, so a config can widen it; §12.1 is an accident guard, by design.

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
