# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
- CLI: `tsr <task>`, `--` argument passthrough (SPEC §6), and `tsr list`, plus
  `--help` / `--version`. Exit codes follow SPEC §10: `0`, the failing child's
  exact code, or `64` for any runner-level error.
