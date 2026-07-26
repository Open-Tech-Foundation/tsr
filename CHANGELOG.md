# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **Windows: `run = "vite"` / auto-detected `npm` could not be spawned at all**,
  failing with `cannot run 'npm': program not found` (exit `64`). The tools this
  targets are batch shims — `npm` is `npm.cmd`, `node_modules/.bin` holds
  `vite.cmd` — but when the OS searches `PATH` for a bare name it appends only
  `.exe`, so the shim was never found. `tsr` now resolves the program against
  the job's `PATH` using `PATHEXT` before spawning (SPEC §9.2), so a
  project-local tool still wins over a global one. A name written with an
  extension, or any path with a separator, is used as given, as a shell would.

### Changed

- Website: bumped the OTF Web framework — `@opentf/web` 0.20.0 → 0.24.0,
  `@opentf/web-docs` 0.17.0 → 0.21.0, `@opentf/web-cli` 1.18.0 → 1.22.0 — and set
  the docs version badge to `v0.2.0`, the release it documents.
- CI: the fake-runner tests now run on Windows too. `shim` writes whichever form
  the platform actually ships — an executable script on Unix, a `.cmd` on
  Windows — so ecosystem auto-detection, configless mode and builtin shadowing
  are covered end to end there. That is the coverage the `PATHEXT` bug slipped
  through: 39 of the 44 e2e tests now run on every platform, up from 33.

## [0.2.0] - 2026-07-26

### Added

- Mini-shell: **glob expansion** in `run` strings (`*`, `?`, `[...]`), matched
  against the filesystem relative to the task's `dir` and following `sh` rules —
  `*` does not cross a `/` or match a leading dot, and an unmatched pattern stays
  literal (SPEC §8.1). Matches come back with `/` separators on every platform,
  so one pattern yields one argv on Linux, macOS and Windows alike. Quoted text
  and expanded variable values are never globbed, so a `*` that arrives via
  `$VAR` stays a literal `*`. Globs resolve
  when their command runs rather than when the task is planned, so a pattern in
  `build && rm dist/*.map` sees the files `build` just produced.
- Mini-shell: **built-in commands** — `rm`, `cp`, `mv`, `mkdir`, `touch`, `cat`,
  `echo` and `pwd` are implemented in-process and always win over a binary of the
  same name on `PATH`, so `run = "rm -rf dist/*"` behaves identically on Linux,
  macOS and Windows (SPEC §8.5). Short options bundle (`-rf`), `--` ends option
  parsing, and relative paths resolve against the task's `dir`. Builtins apply to
  `run` strings only, never to `delegate` or an auto-detected native runner.
- Undefined-`$VAR` errors now underline the offending reference with a caret,
  as SPEC §7.3 has always specified.
- Builtins `true` and `false`, so `cmd || true` — the standard "this step must
  not fail the build" idiom — works on Windows, which has no `/bin/true` to fall
  back on. Both ignore their operands, as POSIX specifies.
- `{a,b}` brace expansion is now rejected at load time instead of being passed
  through as literal text. `rm -rf dist/{js,css}` previously did nothing at all
  and, thanks to `-f`, said nothing about it. Detection matches `sh`'s own
  trigger — only a `{...}` group containing a comma — so `find . -exec rm {} +`
  and `--define:{json}` keep working, and quoting still passes braces through.
- Documented and covered `**` in glob patterns, which the implementation already
  supported: it spans directories, including zero of them, so `a/**/*.js`
  matches `a/x.js` as well as `a/b/c.js`.

### Changed

- Benchmark suite & documentation: updated hyperfine benchmark scenario results across all task runners and synchronized website benchmark dataset. Re-measured on the reference machine after the mini-shell/builtins work; `benches/README.md`'s summary table and the `startup`/`graph10` ratios in its prose were stale against the committed exports and now match them.
- Website landing page: added syntax highlighting for `tasks.toml`, updated tagline theme and modern pill button shapes, updated Safe mini-shell section details, repositioned benchmark speed numbers link, and added Built-in shell & coreutils row to comparison table.
- `run` strings are parsed into an **AST** (program → command → word → part)
  instead of straight into argv. Retaining the structure is what lets expansion
  distinguish a `*` typed in the `run` string from one that arrives via a quote
  or a variable, and it makes direct-vs-mini-shell classification a structural
  property rather than a lexer side effect — so a quoted-but-otherwise-static
  string such as `run = "echo 'a b'"` now takes the fast direct-spawn path.
- `${...}` now accepts a plain variable name only. Parameter expansion
  (`${VAR:-default}`, `${#VAR}`, …) is rejected at load time with a specific
  message instead of failing later as an undefined variable named `VAR:-default`.
- Globs are no longer rejected at load time; the SPEC §8.2 rejection table now
  lists background `&` and subshells `( )` explicitly in their place.
- The `&&`/`||`/`;` sequencing rule now has a single definition (`Sep::proceeds`)
  shared by the executor, replacing the duplicate copy the executor carried.
- CI: the end-to-end suite is no longer Unix-only. Now that the builtins make a
  `run` string portable, 33 of its 43 tests drive the real binary on Windows and
  macOS too — including globbing, builtin chains and `dir`-relative patterns,
  which is where separator bugs actually surface. Only the tests needing `sh` or
  a POSIX-executable fake runner stay `#[cfg(unix)]`.
- CI: cutting a release now requires the cross-platform test suite to pass.
  `release.yml` triggers on the push rather than on CI finishing, so a red
  Windows run could previously not stop a tag being cut.

### Fixed

- Windows installer: `install.ps1` downloaded `tsr-win32-<arch>.zip`, but the
  release ships `tsr-windows-<arch>.zip`, so `irm …/install.ps1 | iex` failed
  with a 404 for the whole 0.1.0 release.

### Changed

- Installers resolve the latest version from the `/releases/latest` redirect
  (the tag is the last segment of the URL it lands on) instead of calling
  `api.github.com`, which is rate-limited per IP.

## [0.1.0] - 2026-07-23

### Added

- Installation & Documentation: added platform-native install scripts (`install.sh` for Linux/macOS/FreeBSD and `install.ps1` for Windows) supporting SHA-256 checksum verification, updated `README.md` with production site URL (`https://tsr.opentechf.org`), and updated the website docs installation guide (`website/app/docs/page.mdx`).
- Website: added Home link (`/`) to site navbar and set site version badge (`version: "v0.1.0"`) in `website/otfw.config.js`, set `lockfileVersion: 1` in `website/bun.lock` for Bun 1.2.x deployment runner compatibility, and added Cloudflare Workers deployment config (`website/wrangler.jsonc`) for static assets hosting.
- CI/CD: added release workflow (`.github/workflows/release.yml`) and release configuration (`release.toml`) using `otf-release`.
- Website: integrated OTF org site footer (`SiteFooter`, `BuiltWithBadge`, OTF logo mark) with dark background, MIT license, OTF logo favicon, and custom OTF (black) / Web (brand orange `rgb(255, 133, 27)`) badge styling.
- Benchmark suite & website: added Deno (`deno task`) to the cross-runner benchmark harness (`benches/gen-workspace.sh`, `benches/run.sh`), updated benchmark docs, landing page comparison table, and synced snapshot data on website.

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
