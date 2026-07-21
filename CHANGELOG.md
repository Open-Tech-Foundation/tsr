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
