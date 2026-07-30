# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Process-tree containment (SPEC §12).** A fail-fast abort used to kill only
  the process `tsr` spawned, so `npm run dev` died while the `vite` it had
  started kept the port. Children that a run may have to kill are now spawned
  into their own process group (unix) or job object (windows), and the whole
  group is torn down — `SIGTERM`, then `SIGKILL` after a 2s grace.

  Isolation costs interactivity on unix: a process group outside the terminal's
  foreground one is stopped by `SIGTTIN` as soon as it reads stdin. So it is
  applied only to runs whose reachable tasks include a `parallel = true` batch —
  exactly the runs where `tsr` can be the one doing the killing. A lone
  `tsr dev` keeps the inherited group and stays interactive.

- **Ctrl-C is handled rather than fatal (SPEC §12).** `SIGINT`/`SIGTERM` (and
  `CTRL_C_EVENT` on Windows) now abort the run through the same path a failure
  uses: stop launching, tear down what is running, exit `130`. Previously `tsr`
  died mid-`wait()` and left its children to init. A second interrupt exits
  immediately, so a wedged child can never trap the terminal.

- **Guarded environment variables (SPEC §12.2).** A config may no longer set the
  variables that decide what code some *other* program loads — `LD_PRELOAD`,
  `LD_AUDIT`, `DYLD_INSERT_LIBRARIES`, `NODE_OPTIONS`, `BASH_ENV`,
  `PYTHONSTARTUP`, `PERL5OPT`, `RUBYOPT`, `GIT_SSH_COMMAND`, `GIT_EXTERNAL_DIFF`,
  `SSH_ASKPASS`, and anything starting `TSR_`. Without this, a `tasks.toml` or a
  committed `.env` that appears to run `cargo test` can execute arbitrary code
  inside an unrelated process.

  `PATH` gets its own rule instead of a ban, since extending it is ordinary: it
  may be set only to a value that still references `$PATH`, so it augments rather
  than replaces — the same "merged, never wiped" principle the env model already
  follows (SPEC §7.1).

  ```toml
  [env]
  PATH = "./bin:$PATH"   # fine
  PATH = "/only/mine"    # rejected — it decides what every bare command resolves to
  ```

  The process environment is untouched: it belongs to whoever ran `tsr`. Only
  `[env]`, task `env`, `env_file` and the root `.env` are checked, over the tasks
  that will actually run, before anything is spawned (exit `64`).

  The opt-in is the CLI flag **`--allow-unsafe-env`**, deliberately not a
  `[security]` key: these guards exist for the case where the `tasks.toml` is
  what you are wary of, and a guard a config could switch off would not survive
  that case.

- **Workspace confinement for everything `tsr` does itself (SPEC §12.1).**
  The in-process builtins (`rm`, `cp`, `mv`, `mkdir`, `touch`, `cat`) now refuse
  an operand that resolves outside the workspace, and `dir`, `env_file`,
  `packages` and `workspace.members` are rejected at **load** time if they point
  outside it (exit `64`). `rm -rf ../..` in a `run` string used to delete
  whatever was there.

  Builtins are where this matters most: `rm` is `tsr` itself, always preferred
  over any binary of the same name, so there is no `PATH`, sandbox or audit to
  fall back on. Resolution is physical — a symlink inside the workspace that
  points out of it is out of it.

  **Breaking** for a config that deliberately reaches outside its own tree.
  Widen the boundary explicitly:

  ```toml
  [security]
  allow_paths = ["../shared-cache"]
  ```

  This guard is about accidents, not malice: a `tasks.toml` can widen it, so it
  is not a defence against a config you do not trust. `tsr` still cannot confine
  the programs it *spawns* — that is what a sandbox is for.

- **`--dry-run` — print the plan, run nothing (SPEC §12).** Walks the dependency
  graph and prints each leaf's label, directory and command, so an unfamiliar
  `tasks.toml` can be read before it is handed a shell. Commands print **as
  written**, before `$VAR` expansion, so a plan pasted into an issue or a CI log
  cannot carry what `.env` supplied. The walk is always sequential, even for
  `parallel = true` batches, so the plan is readable.

### Fixed

- CI (Windows): a `--no-bail` e2e test named `sh` outright without a
  `#[cfg(unix)]` guard, so it would have failed the Windows matrix job.

### Changed

- Website: docs version badge set to `v0.6.0`, the release these docs describe.
- CI: the build/test/lint workflow now skips website-only changes
  (`paths-ignore: website/**`) — `website/` is a separate Node project that cargo
  never builds, so a docs commit no longer spends a three-OS matrix. The release
  workflow is deliberately *not* filtered: it gates on a version bump, and
  release commits often touch `website/` too.

## [0.6.0] - 2026-07-27

### Added

- **`--no-bail` — run everything, report every failure (SPEC §5.2).**
  `tsr` still fails fast by default; `--no-bail` runs each batch to completion,
  neither skipping nor killing siblings, so one run tells you everything that is
  broken. The propagated exit code is still the **first** failure's, so CI sees
  the same signal either way. It covers *task* failures only — a runner-level
  error still stops the run, since a missing `delegate` binary will be missing
  for every package too.

- **`--reporter ndjson` and `--reporter-file <path>` — machine-readable output
  (SPEC §6.2).** A `task` event as each unit of work finishes, then a `summary`.

  ```json
  {"durationMs":12.4,"exitCode":null,"label":"build (packages/ui)","status":"ok","type":"task"}
  {"durationMs":48.9,"exitCode":1,"failed":1,"ok":3,"runnerError":null,"skipped":2,"status":"failed","task":"build","type":"summary"}
  ```

  Two independent sinks. `--reporter` chooses the **terminal** format; the
  `ndjson` value writes events to stderr, which is fine to read but **not** safe
  to parse — children inherit stdio, and one that logs JSON (pino, `jest --json`,
  `tracing`) emits lines indistinguishable from reporter events, `type` field and
  all. `--reporter-file <path>` is the sink to script against, because nothing
  else writes to it. It works on its own, so the terminal keeps the human summary
  while the file gets the machine-readable record:

  ```sh
  tsr ci --no-bail --reporter-file results.ndjson
  ```

  A file that cannot be created is a runner error (exit `64`) raised **before any
  task runs**.

- **`--resume-from <pkg>` — carry on from a package (SPEC §9.4).**
  Treats every package ordered before `<pkg>` as already built. The skipped
  prefix stays skipped even when a later package reaches it as an `^task`
  upstream dependency, which is what makes a resume actually skip work. Matched
  by relative path or manifest name; no match is a runner error (exit `64`), and
  `--since` and `--resume-from` compose.

### Fixed

- **Cargo target-specific dependencies were invisible to the package graph.**
  `[target.'cfg(windows)'.dependencies]` and its `dev-`/`build-` siblings were
  never read, so a crate depending on a workspace sibling only on some platforms
  got no edge — producing a silently wrong build order rather than an error.

- **Cargo workspace-inherited dependencies could lose their edge.**
  `dep = { workspace = true }` takes its real crate name from the workspace
  root, so a `package = "…"` rename declared there left the member's key
  pointing at nothing — again a silently wrong build order. Inherited entries
  are now resolved against the nearest ancestor `Cargo.toml` carrying a
  `[workspace]` table, the same search Cargo performs.

- **The `--config` TUI reported a valid `^task` config as broken.** The graph
  preview looked `^build` up as a task key, and since task names can never
  contain `^` the lookup always missed, rendering `● ^build (undefined task)` in
  red. Upstream markers now render as their own node kind.

### Changed

- The "nothing to do" message for a filtered fan-out is now `no packages
  selected` (was `no affected packages`), since `--resume-from` can produce it too.

## [0.5.0] - 2026-07-27

### Added

- **Package dependency graph (SPEC §9, §11) — the foundation of v1.1.**
  `tsr` now reads each workspace package's *declared dependencies* from its
  manifest, in addition to the ecosystem and name it already read, and resolves
  the edges between workspace members. One rule spans all five ecosystems: an
  edge exists exactly when a declared dependency name matches another workspace
  package's manifest name, so `workspace:*`, `path = "../ui"`, `replace`
  directives and plain version ranges all resolve identically, and external
  registry dependencies drop out on their own.
  - npm/bun — `dependencies`, `devDependencies`, `peerDependencies`,
    `optionalDependencies`
  - cargo — `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`,
    following `package = "…"` renames to the real crate name
  - go — `require` and `replace`, in both single-line and block form
  - python — PEP 621 `[project]` (including optional groups), PEP 735
    `[dependency-groups]`, and Poetry's own tables
  
  Package graphs are not required to be acyclic — npm tolerates cycles and real
  repos ship them — so construction never fails; only topological ordering
  reports a cycle, as a runner error (exit `64`). Malformed or absent manifests
  contribute no edges rather than failing discovery.

- **Topological deps — the `^task` upstream marker (SPEC §4.2, §5.0).**
  `deps = ["^build"]` now means "run `build` in every package this one depends
  on, first". A `packages` fan-out becomes a walk of the package graph instead
  of a flat batch.

  ```toml
  [tasks.build]
  packages = ["apps/*", "packages/*"]
  deps = ["^build"]
  ```

  - `packages` is **required**: `^` is relative to the package it runs in, and
    only a fan-out supplies one. Otherwise it is a config error (exit `64`).
  - Upstream packages are visited **even when the pattern did not select them** —
    building `apps/*` builds the libraries those apps import.
  - `^name` may name a task other than the one declaring it (`^codegen`).
  - Each `(task, package)` pair runs at most once, so a library shared by several
    dependents is built once.
  - Ordinary `deps` alongside `^` deps still run once, globally, before the
    fan-out; `parallel` and fail-fast behave exactly as they do for any batch.
  - A cycle in the package graph is a runner error (exit `64`).

- **Affected detection — `tsr <task> --since <ref>` (SPEC §6.1, §9.3).**
  Restricts every `packages` fan-out to the packages affected by changes since a
  git ref: the packages the changed files live in, plus every package that
  transitively depends on them.

  ```sh
  tsr build --since main
  ```

  - Changes are read from git — commits, unstaged edits **and** untracked files,
    since a brand-new package exists only as untracked files.
  - A changed file **outside every package** (root config, lockfile, CI
    workflow) runs everything: it could affect anything, and skipping work that
    should have run is worse than repeating work that need not have.
  - Only the *selection* narrows. Upstream `^task` dependencies are still built
    whether or not they changed, so a filtered run stays correct, not just fast.
  - Nothing affected is a clean exit `0`. An unmatched `packages` *pattern*
    remains an error, because that is a typo.
  - A missing repository, unknown ref, or missing git binary is a runner error
    (exit `64`) rather than a silent full or empty run.
  - `--since` is the only option that may follow a task name; the bare-word
    namespace still belongs entirely to task names.

  With this, **all three v1.1 capabilities are implemented**.

- **Benchmark coverage for the topological fan-out**, plus `nub` in the
  comparison set. New `topo10`/`topo50`/`topo200` scenarios build a real
  multi-package workspace in dependency order, comparing only the runners that
  actually order packages by dependency — `tsr` (`^build`), `pnpm` (`-r`) and
  `bun` (`--filter`). `tsr`'s marginal cost measures **~56–62 µs per package**
  and falls slightly as the workspace grows, so building the package graph stays
  off the critical path. Turbo/Nx are excluded by design (they exist to cache,
  which `tsr` delegates), as is `npm --workspaces` (it orders packages by name,
  not by dependency). Results and rationale: [`benches/README.md`](benches/README.md).

### Changed

- **`package.json` is now parsed with `serde_json`** instead of the hand-rolled
  depth-1 key scanner, which could not read nested dependency objects. Names
  are now read from strictly-valid JSON only — a malformed `package.json` no
  longer yields a name by accident.

## [0.4.0] - 2026-07-26

### Changed

- **Simplified `tsr --init` starter template (`tasks.toml`).**
  Removed redundant explanatory prose comments from the scaffold while keeping clean, commented TOML examples for `[workspace]`, `[env]`, `[tasks.<name>]` (run, delegate, and auto-detect), and dependency graphs.

### Fixed

- **Windows: `PATH` was looked up case-sensitively, so it was never found.**
  Windows compares environment names case-insensitively and conventionally
  spells the variable `Path`, but `tsr` carries the environment in a `HashMap`,
  whose lookup is exact — so `get("PATH")` returned nothing. Two consequences,
  both Windows-only:
  - The 0.3.0 `PATHEXT` fix could not do its job: with no `PATH` to search it
    fell straight back to the old behaviour, leaving `cannot run 'npm': program
    not found` exactly as before.
  - `node_modules/.bin` prepending (SPEC §9.2) inserted a *second* `PATH`
    variable instead of extending `Path`, so a job's environment lost every
    system directory. A local tool that shelled out to anything else on `PATH`
    would fail.

  Environment names are now compared the way the platform does, and values are
  written back under the key already in use.

## [0.3.0] - 2026-07-26

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
