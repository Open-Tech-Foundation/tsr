# Security Policy

## Reporting a vulnerability

Please report security issues privately, **not** in a public issue.

Use GitHub's [private vulnerability reporting][advisory] on this repository
(Security → Report a vulnerability). Include the version (`tsr --version`), your
platform, and a `tasks.toml` that reproduces the problem if you have one.

[advisory]: https://github.com/Open-Tech-Foundation/tsr/security/advisories/new

We aim to acknowledge a report within a week, and to have a fix or a decision
within 30 days. You will be credited in the release notes unless you would
rather not be.

## Supported versions

Fixes land on the latest release. There are no long-term support branches.

## What `tsr` does and does not defend

Running `tsr build` in a repository is **running that repository's code**, the
same as `npm run build` or `make`. `tsr` does not sandbox the programs it spawns
and it is not intended to make an untrusted repository safe to build. If that is
what you need, use a container.

What `tsr` guards is the part with no process boundary around it — the things it
does *itself*. The full model is [SPEC §12](./SPEC.md#12-security-model); in
short:

| Guard | What it stops |
|-------|---------------|
| **Workspace confinement** | The in-process builtins (`rm`, `cp`, `mv`, …) touching anything outside the workspace. `rm` is `tsr`, not `/bin/rm`, so nothing else can stop it. |
| **Guarded env variables** | A config or a `.env` setting `LD_PRELOAD`, `NODE_OPTIONS`, `JAVA_TOOL_OPTIONS`, `GOFLAGS`, `GIT_SSH_COMMAND`, … — variables that make an *unrelated* program load code of the config's choosing. The list is not exhaustive and cannot be; see SPEC §12.2. |
| **Discovery boundary** | A `tasks.toml` planted above your project (in `/tmp`, in `$HOME`) silently governing it, or a world-writable one deciding what runs. |
| **Process-tree containment** | Orphaned children surviving a failure or a Ctrl-C. |

Two of these can be relaxed, and the difference is deliberate:

- `[security] allow_paths` widens workspace confinement. It is a **config** key,
  so it guards against accidents, not against a `tasks.toml` you distrust.
- `--allow-unsafe-env` lifts the environment guards. It is a **CLI flag** and
  has no config equivalent, because those guards exist for exactly the case
  where the config is what you are wary of.

### Inspecting a config before running it

```sh
tsr <task> --dry-run
```

prints every command the run would execute, in order, **before** `$VAR`
expansion — so the plan is safe to paste into an issue and cannot leak what
`.env` holds.

## Out of scope

Reports about the following are not treated as vulnerabilities, since they
describe the tool working as designed:

- A `tasks.toml` running arbitrary commands. That is the entire purpose.
- A spawned program doing anything the invoking user could do.
- `node_modules/.bin` shadowing a global binary on `PATH` — npm's behaviour, and
  what makes `run = "vite"` work.
- `[security] allow_paths` being widened by the config that it constrains.
- The absence of resource limits on a task. Limits declared in `tasks.toml` would
  be no defence against a config that omits them; use a container or `ulimit`.
- A local attacker racing the run — creating a symlink inside the workspace
  while a build executes. Anyone who can do that already controls the repository.
- A guarded-variable name that is not on the list. Please still report it: the
  list is meant to grow with known vectors, it just cannot be complete.

## Verifying a release

Release archives ship a `checksums.txt`; both installers verify it
automatically. Releases also carry [build provenance attestations][attest]:

```sh
gh attestation verify tsr-<target>.tar.gz --repo Open-Tech-Foundation/tsr
```

[attest]: https://docs.github.com/actions/security-guides/using-artifact-attestations
