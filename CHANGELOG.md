# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Per-task `env_file`: a task may load one or more `.env`-style files (a string
  or an array), e.g. `env_file = [".env.local", ".env.test"]`. Files are resolved
  relative to the task's `dir` (or the workspace root) and layered into the merge
  **above** the root `.env` and workspace `[env]` but **below** the inline task
  `env` — so it is the way to override the default `.env` for a task (e.g.
  `.env.test` for a test task). Listed order is increasing precedence (later
  overrides earlier); missing files are skipped, like the root `.env`, so an
  optional `.env.local` need not exist. Values honour `$VAR` expansion and the
  load-time undefined-`$VAR` check. Authorable in the `--config` TUI (SPEC §7.2).
- Configless mode: `tasks.toml` is now **optional**. With no config file,
  `tsr <task>` runs repo-aware by treating the task as a bare form-3 auto-detect
  anchored at the nearest ecosystem marker (`package.json`, `Cargo.toml`,
  `go.mod`, `pyproject.toml`) found by walking up — so `tsr dev` runs `npm run dev`,
  `tsr build` runs `cargo build`, etc., with `--` passthrough intact. A present
  `tasks.toml` always takes precedence (no fall-through from a defined config to
  auto-detection, so a mistyped task stays an error); package-qualified names and
  the dependency graph still require a config. When neither a `tasks.toml` nor a
  marker exists, `tsr` exits `64` with a message pointing at `tsr --init`, and
  `tsr --list` reports the detected package instead of erroring (SPEC §2.1).
- `tsr --init`: scaffold a starter `tasks.toml` in the current directory —
  reference comments only, showcasing all three task forms, `[workspace]`,
  `[env]` and the graph, and linking to <https://tsr.opentechf.org/docs>. It
  defines **no** live tasks on purpose: since a present `tasks.toml` takes full
  precedence over auto-detection (SPEC §2.1), a placeholder task would shadow
  what the repo already runs (e.g. hide the real `npm run dev`). Refuses to
  overwrite an existing file (exit `64`).
- Builtins (`--list`, `--config`, `--init`, `--help`, `--version`) are flags only,
  never bare subcommands: the first positional argument is always a task name, so
  a task named `list` or `init` is never shadowed.
- `tsr --config`: an interactive TUI (ratatui) for authoring tasks with every
  option (form, `dir`/`packages`, `deps`, `parallel`, `args`, `env`) instead of
  hand-editing TOML. It opens on a **home menu** of workflows — Add a task, Edit
  a task, Delegate a task, Delete a task, Preview graph, Quit — so there is
  always an obvious next step instead of a blank list; each entry launches its
  own screen and `Esc` returns to the menu. Add/Delegate open the task form
  (Delegate pre-selects the `delegate` type); Edit/Delete open a task picker;
  delete asks for a `y`/`n` confirmation. Changes **autosave**: applying a form
  or confirming a delete writes `tasks.toml` immediately, so there is no unsaved
  state, no dirty marker, and no discard prompt on quit — and because a change is
  validated *before* it is committed, an autosave can never write a broken config
  (an invalid form stays open with the error inline). `⏎` saves a form rather than
  `Ctrl+S`, which editor/IDE terminals grab for "save file" and which is XOFF
  where terminal flow control is on; `Ctrl+S` remains an alias. Edits go through
  the format-preserving `toml_edit` document, so comments and unknown keys
  survive: a **new** task is appended below everything the file already holds
  (including a comment-only `--init` scaffold, whose text is document trailing
  trivia and would otherwise end up *below* the inserted table), **editing** a
  task leaves it exactly where it sits, keeping the comment written above it,
  and **deleting** one leaves every other task in place. Deletion splits the
  removed table's leading comments at the last blank line: the block written
  directly above the task goes with it, while file-level text above that (for
  the first task, the entire file header) is handed to whichever task now
  renders in its place, or back to the document if none does. Starts a new file
  if none exists.
- `tsr --config` graph/dry-run view (`g` for the selected task, `G`/`a` for all):
  a read-only, connected dependency tree rendered with box connectors, showing
  each task's **dry-run** command — what `tsr` would execute, resolved by the real
  precedence (`delegate` → `run` → auto-detect; a deps-only task shows "runs its
  deps only" and a `packages` task is annotated with its fan-out). Parallel vs.
  sequential batches are tagged, roots are the tasks nothing depends on, and
  undefined deps or dependency cycles in a mid-edit config are flagged inline.
- Landing page: a "How it compares" capability table (tsr vs npm, bun, just,
  go-task, mise, Turbo/Nx) covering auto-detection, dependency graph, parallelism,
  monorepo fan-out, `node_modules/.bin` resolution, declarative env vars & `.env`,
  native speed, static binary, and caching (marked delegated-by-design for tsr),
  with a link through to the benchmark numbers.
- Website + documentation under `website/`, built with the OTF Web framework and
  `@opentf/web-docs` (`DocsLayout`): a marketing landing page plus a docs section
  (overview with a first-task walkthrough, configuration, task forms, mini-shell,
  environment, graph/parallelism, monorepo, guides, CLI reference, exit codes).
  Builds to a static site with search via `bun run build`. The overview merges the
  former getting-started page and presents the four setup steps with the
  `Steps` stepper; practical how-to recipes live on a dedicated **Guides** page —
  moved to the top of the docs menu (right under Overview) and fronted by a card
  grid indexing all twelve recipes (zero-config runs, passthrough args, migrating
  npm scripts, dependency graphs, monorepo fan-out, local tools, env & per-task
  `.env` files, delegating caching, …). Per-page "next steps" footers were dropped
  in favour of the sidebar.
- Cross-tool benchmark harness under `benches/` (generated by `gen-workspace.sh`,
  driven by hyperfine): six scenarios — `startup`, `shell` (mini-shell `$VAR` +
  `&&`), `localbin` (resolving a `node_modules/.bin` tool), `steps5` (in-task
  sequencing), and `graph5`/`graph10` (dependency graphs) — across tsr, npm, bun,
  just, go-task, make, and mise, with committed reference results and a website
  page that loads the JSON. The graph scenarios show per-invocation overhead
  compounding (chained `npm` ~843 ms for ten no-op tasks vs tsr ~9 ms); `localbin`
  shows tsr ~3.6× faster than `npm run` when calling a project-local tool; `mise`
  sits between the native runners and npm/go-task (~20 ms startup).

- CI: GitHub Actions matrix — build + test on ubuntu, macOS, and **Windows**
  (validating cross-platform behaviour, notably the `node_modules/.bin` PATH
  logic), plus a `fmt --check` + `clippy -D warnings` lint job. Execution tests
  that shell out to Unix coreutils are `#[cfg(unix)]`; Windows runs the
  platform-independent unit tests and a full `cargo build`.

### Fixed

- Windows CI: the `prepends_node_bin_dirs_nearest_first` test now builds its
  expected `node_modules/.bin` paths with the same `.join("node_modules").join(".bin")`
  as `prepend_node_bin`, so path separators match on Windows (a single
  `join("node_modules/.bin")` kept a forward slash and failed only there).
- Auto-detect (form 3) is now actually executed for a **single** bare task. A
  bare `[tasks.<name>]` with no `run`/`delegate`/`packages` and no `deps` was
  wrongly treated as a deps-only aggregator and silently did nothing (exit `0`),
  even though `--list` labelled it `auto` — so `npm run <name>` / `cargo <name>` /
  `go <name>` / `uv run <name>` never ran. It now resolves and spawns the native
  runner (SPEC §3.1). A bare task that still has `deps` remains a pure aggregator
  (SPEC §5.2), and a bare task with no detectable ecosystem is a clear runner
  error (exit `64`) rather than a no-op. Verified end-to-end against shimmed
  npm/bun/cargo/go/uv runners.
- `run` strings now resolve locally-installed binaries: `tsr` prepends
  `node_modules/.bin` to `PATH` (walking up from the task's directory to the
  workspace root, nearest first), the same lookup npm/bun/yarn/pnpm do. Without
  this, `run = "vite"` / `run = "eslint"` could not find a project-local tool, so
  tsr was not actually a drop-in `npm run` replacement (SPEC §9.2).
- Execution: the fixed 15 ms child-poll interval added a full tick of latency to
  every fast task (a no-op measured ~16 ms). Replaced with adaptive backoff
  (`POLL_MIN` 100 µs → `POLL_MAX` 20 ms): fast tasks now finish in ~1.6 ms while
  fail-fast kill latency for long-running siblings stays bounded.

- Config layer: parse `tasks.toml` via `toml_edit` (comments and unknown keys
  survive a round-trip), discover the workspace root by walking up to the
  nearest `tasks.toml`, and structurally validate at load time — rejecting
  `dir`+`packages` together, illegal task-name characters, malformed `#` keys,
  and `^upstream` deps (v1.1) with exit code `64`.
- Error model mapping runner-level failures to exit code `64` and task failures
  to their child's exact exit code.
- Detection layer: identify a package's ecosystem from marker files
  (`package.json` → npm/bun, `Cargo.toml`, `go.mod`, `pyproject.toml`) and map a
  bare task to its native runner convention (`npm run <task>`, `cargo <task>`, …).
- Task-form resolution honouring precedence `delegate` → `run` → auto-detect.
- Mini-shell for `run` strings (SPEC §8): quote-aware lexing classifies a string
  as a direct spawn (no metacharacters) or a mini-shell program supporting
  `$VAR`/`${VAR}` expansion, `&&`/`||`/`;` sequencing, and single/double quoting.
  Unsupported constructs (`|`, `>`/`<`, globs, `$(...)`/backticks, bare `&`,
  subshells) are rejected at load time with exit code `64`.
- Environment model (SPEC §7): merge `task > workspace [env] > root .env >
  process` (lower sources augmented, never wiped), with per-value `$VAR`
  expansion against process env and earlier keys, root `.env` auto-loading, and
  a load-time check that every `$VAR` in a `run` string is defined (else `64`).
- Workspace package discovery (expand `[workspace] members` globs to
  marker-carrying dirs, read manifest names) and `packages` matching by path
  glob or exact manifest name (SPEC §9.1).
- Dependency-graph validation: unknown-task and cycle detection (exit `64`).
- Execution engine (SPEC §5): recursive `deps`-before-task ordering with
  per-task memoisation (diamond-safe), sequential-by-default / opt-in
  `parallel` batches, `packages` fan-out, and fail-fast that stops sequential
  launches and kills running parallel siblings, then prints a summary. The first
  failing child's exact exit code is propagated; runner breakage exits `64`.
- CLI: `tsr <task>`, `--` argument passthrough (SPEC §6), and `tsr --list`, plus
  `--help` / `--version`. Exit codes follow SPEC §10: `0`, the failing child's
  exact code, or `64` for any runner-level error.
- End-to-end test suite driving the compiled binary against temp workspaces, and
  expanded README covering configuration, `run` strings, env, and exit codes.
