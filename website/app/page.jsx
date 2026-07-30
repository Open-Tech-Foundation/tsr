import { Link } from "@opentf/web";

function ArrowRightIcon({ stroke = "currentColor", style, className }) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke={stroke}
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      style={style}
      className={className}
    >
      <path d="M5 12h14" />
      <path d="m12 5 7 7-7 7" />
    </svg>
  );
}

// Capability comparison against the other runners people reach for. Each cell is
// "y" (has it), "p" (partial / needs a plugin or extra tool), or "n" (no). Kept
// deliberately factual — the benchmark page has the speed numbers.
const COMPARE_TOOLS = ["tsr", "npm", "bun", "deno", "just", "go-task", "mise", "Turbo/Nx"];
const COMPARE_ROWS = [
  {
    label: "Auto-detects each package's runner",
    hint: "cargo / go / npm / bun / uv from a bare task",
    cells: ["y", "n", "n", "n", "n", "n", "n", "n"],
  },
  {
    label: "Dependency graph (DAG)",
    cells: ["y", "n", "n", "n", "y", "y", "y", "y"],
  },
  {
    label: "Opt-in parallelism",
    cells: ["y", "p", "p", "p", "n", "y", "y", "y"],
  },
  {
    label: "Monorepo workspace fan-out",
    hint: "run one task across every package",
    cells: ["y", "p", "p", "p", "n", "n", "n", "y"],
  },
  {
    label: "Resolves node_modules/.bin",
    hint: "call vite / eslint like npm run",
    cells: ["y", "y", "y", "y", "n", "n", "n", "y"],
  },
  {
    label: "Built-in shell & coreutils",
    hint: "cross-platform rm, cp, mkdir, $VAR, globs",
    cells: ["y", "n", "p", "p", "n", "p", "n", "n"],
  },
  {
    label: "Declarative env vars & .env",
    hint: "[env] blocks + auto-loaded .env",
    cells: ["y", "p", "p", "p", "y", "y", "y", "y"],
  },
  {
    label: "Native speed, no runtime boot",
    cells: ["y", "n", "p", "p", "y", "p", "p", "n"],
  },
  {
    label: "Single static binary",
    cells: ["y", "n", "y", "y", "y", "y", "y", "n"],
  },
  {
    label: "Content-hash / remote caching",
    hint: "tsr delegates this to Turbo/Nx by design",
    cells: ["d", "n", "n", "n", "n", "p", "n", "y"],
  },
];

const COMPARE_MARK = {
  y: { sym: "✅", cls: "cmp-y", label: "yes" },
  p: { sym: "🟡", cls: "cmp-p", label: "partial" },
  n: { sym: "❌", cls: "cmp-n", label: "no" },
  d: { sym: "🔀", cls: "cmp-d", label: "delegated by design" },
};

// The marketing landing page. Static (no client state) — the live chrome (navbar,
// theme toggle) comes from RootLayout. Internal links use <Link> for client-side
// navigation; the docs section owns its own layout.
export default function Home() {
  return (
    <>
      {/* --- hero --- */}
      <section class="hero">
        <div class="container hero-grid">
          <div>
            <span class="eyebrow">
              <span class="dot" />
              A command runner, not a build system
            </span>
            <h1 class="title">
              One interface over <span class="grad">every runner</span> in your repo.
            </h1>
            <p class="lede">
              <strong>tsr</strong> is a lightweight, polyglot, repo-aware task runner. It
              wraps the native runners you already have — <code>npm</code>, <code>bun</code>,{" "}
              <code>cargo</code>, <code>go</code>, <code>uv</code> — adds a task dependency
              graph and opt-in parallelism, and delegates caching to Turbo/Nx instead of
              reinventing it.
            </p>
            <p class="lede">
              And it is <strong>guarded by default</strong>: no config can delete outside
              your workspace or inject <code>LD_PRELOAD</code> into your build.
            </p>
            <div class="cta-row">
              <Link class="btn btn-primary" href="/docs">
                Get started <ArrowRightIcon stroke="#ffffff" />
              </Link>
              <Link class="btn btn-ghost" href="/docs/security">
                Security model <ArrowRightIcon />
              </Link>
            </div>
          </div>

          <div class="term-window">
            <div class="term-bar">
              <span class="term-dot" />
              <span class="term-dot" />
              <span class="term-dot" />
              <span class="term-title">~/app — tsr ci</span>
            </div>
            <div class="term-body">
              <div>
                <span class="p">$</span> <span class="c">tsr ci</span>
              </div>
              <div class="muted">├─ lint&nbsp;&nbsp;&nbsp;→ cargo clippy</div>
              <div class="muted">├─ test&nbsp;&nbsp;&nbsp;→ npm run test</div>
              <div class="muted">└─ build&nbsp;&nbsp;→ turbo run build</div>
              <div>&nbsp;</div>
              <div>
                <span class="ok">✓ lint</span>&nbsp;&nbsp;&nbsp;&nbsp;ok&nbsp;&nbsp;&nbsp;&nbsp;1.2s
              </div>
              <div>
                <span class="ok">✓ test</span>&nbsp;&nbsp;&nbsp;&nbsp;ok&nbsp;&nbsp;&nbsp;&nbsp;3.4s
              </div>
              <div>
                <span class="ok">✓ build</span>&nbsp;&nbsp;&nbsp;ok&nbsp;&nbsp;&nbsp;&nbsp;0.9s
              </div>
              <div>&nbsp;</div>
              <div>
                <span class="ok">✓ ci passed</span> <span class="muted">— exit 0</span>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* --- features --- */}
      <section class="section">
        <div class="container">
          <h2>Why tsr</h2>
          <p class="sub">A thin unifying layer — predictable by default, delegate by design.</p>
          <div class="grid">
            <div class="card">
              <div class="ico">⚡</div>
              <h3>No startup tax</h3>
              <p>
                Metachar-free <code>run</code> strings are spawned directly (execvp-style) —
                no <code>npm run</code> / Node boot to pay on the common path.
              </p>
            </div>
            <div class="card">
              <div class="ico">🌐</div>
              <h3>Polyglot</h3>
              <p>
                One entry point across every ecosystem. A bare <code>[tasks.test]</code>
                auto-detects each package's runner: cargo, go, npm/bun, uv.
              </p>
            </div>
            <div class="card">
              <div class="ico">🔗</div>
              <h3>Dependency graph</h3>
              <p>
                Declare <code>deps</code> and get a DAG. Sequential by default; opt into
                concurrency with <code>parallel = true</code>. Fail-fast, always.
              </p>
            </div>
            <div class="card">
              <div class="ico">🧩</div>
              <h3>Three task forms</h3>
              <p>
                <code>delegate</code> to a backend, <code>run</code> a command directly, or
                let tsr auto-detect the native runner — resolved by precedence.
              </p>
            </div>
            <div class="card">
              <div class="ico">🐚</div>
              <h3>Safe mini-shell</h3>
              <p>
                In-process <code>$VAR</code> expansion, <code>&amp;&amp; || ;</code>, quoting, and globs
                (<code>*</code>, <code>**</code>) plus cross-platform builtins (<code>rm</code>, <code>cp</code>, <code>mkdir</code>). Pipes &amp; redirects are rejected up front.
              </p>
            </div>
            <div class="card">
              <div class="ico">📦</div>
              <h3>Delegate caching</h3>
              <p>
                Content-hash and remote caching are ceded to Turbo/Nx — never
                reimplemented. tsr stays a lightweight command runner.
              </p>
            </div>
          </div>
        </div>
      </section>

      {/* --- security --- */}
      <section class="section">
        <div class="container">
          <h2>Guarded by default</h2>
          <p class="sub">
            A task runner runs the repo's code — that is the job. What it should not do is
            let a config reach past the commands it visibly declares. tsr guards the parts
            it performs itself, with no flag to turn on.
          </p>

          <div class="two-col">
            <div class="split-copy">
              <ul>
                <li>
                  <strong>Nothing tsr touches leaves the workspace.</strong> The in-process
                  builtins (<code>rm</code>, <code>cp</code>, <code>mv</code>) refuse
                  operands outside it — symlinks included — and <code>dir</code>,{" "}
                  <code>env_file</code> and <code>packages</code> are rejected at load.
                </li>
                <li>
                  <strong>No environment injection.</strong> A config or a committed{" "}
                  <code>.env</code> cannot set <code>LD_PRELOAD</code>,{" "}
                  <code>NODE_OPTIONS</code> or <code>GIT_SSH_COMMAND</code>, and{" "}
                  <code>PATH</code> may be extended but never replaced.
                </li>
                <li>
                  <strong>Discovery stops at your repo.</strong> The walk up to{" "}
                  <code>tasks.toml</code> never climbs past the git root, your home
                  directory, or a filesystem boundary — and a world-writable config is
                  refused outright.
                </li>
                <li>
                  <strong>Nothing outlives the run.</strong> A failure or a Ctrl-C tears
                  down the whole process group, so a killed <code>npm run dev</code> never
                  leaves <code>vite</code> holding the port.
                </li>
              </ul>
              <div class="cta-row" style="margin-top:20px">
                <Link class="btn btn-ghost" href="/docs/security">
                  Read the security model <ArrowRightIcon />
                </Link>
              </div>
            </div>

            <div class="term-window">
              <div class="term-bar">
                <span class="term-dot" />
                <span class="term-dot" />
                <span class="term-dot" />
                <span class="term-title">~/app — guards</span>
              </div>
              <div class="term-body">
                <div>
                  <span class="p">$</span> <span class="c">tsr clean</span>{" "}
                  <span class="muted"># run = "rm -rf ../../build"</span>
                </div>
                <div class="warn">rm: refusing to touch '/home/you/build':</div>
                <div class="warn">
                  &nbsp;&nbsp;&nbsp;&nbsp;outside the workspace at '/home/you/app'
                </div>
                <div>&nbsp;</div>
                <div>
                  <span class="p">$</span> <span class="c">tsr build</span>{" "}
                  <span class="muted"># .env sets LD_PRELOAD</span>
                </div>
                <div class="cross">✗ config error: the root '.env' sets 'LD_PRELOAD',</div>
                <div class="cross">
                  &nbsp;&nbsp;which decides what code an unrelated program loads
                </div>
                <div>&nbsp;</div>
                <div>
                  <span class="p">$</span> <span class="c">tsr ci --dry-run</span>{" "}
                  <span class="muted"># read it before you run it</span>
                </div>
                <div class="muted">· lint&nbsp;&nbsp;&nbsp;dir: .&nbsp;&nbsp;cmd: eslint .</div>
                <div class="muted">
                  · build&nbsp;&nbsp;dir: packages/ui&nbsp;&nbsp;cmd: vite build
                </div>
                <div>
                  <span class="ok">✓ nothing executed</span>{" "}
                  <span class="muted">— exit 0</span>
                </div>
              </div>
            </div>
          </div>

          <div class="grid" style="margin-top:22px">
            <div class="card">
              <div class="ico">🛡️</div>
              <h3>Deny by default</h3>
              <p>
                Every guard is on out of the box. Widening workspace confinement takes an
                explicit <code>[security] allow_paths</code>; lifting the environment
                guards takes <code>--allow-unsafe-env</code> on the command line.
              </p>
            </div>
            <div class="card">
              <div class="ico">🔒</div>
              <h3>No config self-service</h3>
              <p>
                The env guards exist for the case where the <code>tasks.toml</code> is what
                you're wary of — so there is deliberately no config key that can switch
                them off. Only the person typing the command can.
              </p>
            </div>
            <div class="card">
              <div class="ico">👀</div>
              <h3>Inspect before you run</h3>
              <p>
                <code>tsr &lt;task&gt; --dry-run</code> prints every command a run would
                execute, in order — <em>before</em> <code>$VAR</code> expansion, so the
                plan can't leak what your <code>.env</code> holds.
              </p>
            </div>
          </div>
        </div>
      </section>

      {/* --- comparison --- */}
      <section class="section">
        <div class="container">
          <h2>How it compares</h2>
          <p class="sub">
            tsr is a command runner, not a build system — it unifies the runners you have
            and cedes caching to the tools built for it. Here's where it lands next to the
            usual suspects.
          </p>
          <div class="compare-wrap">
            <table class="compare">
              <thead>
                <tr>
                  <th scope="col">Capability</th>
                  {COMPARE_TOOLS.map((t) => (
                    <th scope="col" class={t === "tsr" ? "cmp-self" : ""}>
                      {t}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {COMPARE_ROWS.map((row) => (
                  <tr>
                    <th scope="row">
                      <span class="cmp-label">{row.label}</span>
                      {row.hint ? <span class="cmp-hint">{row.hint}</span> : null}
                    </th>
                    {row.cells.map((c, i) => {
                      const m = COMPARE_MARK[c];
                      return (
                        <td class={COMPARE_TOOLS[i] === "tsr" ? "cmp-self" : ""}>
                          <span class={m.cls} title={m.label} aria-label={m.label}>
                            {m.sym}
                          </span>
                        </td>
                      );
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div class="cmp-legend-row">
            <div class="cmp-legend">
              <span>✅ yes</span>
              <span>🟡 partial / needs a plugin</span>
              <span>🔀 delegated by design</span>
              <span>❌ no</span>
            </div>
            <Link class="cmp-bench-link" href="/docs/benchmarks">
              See the speed numbers <ArrowRightIcon />
            </Link>
          </div>
        </div>
      </section>

      {/* --- example --- */}
      <section class="section">
        <div class="container two-col">
          <div class="split-copy">
            <h2>One file, every task</h2>
            <p class="sub">
              <code>tasks.toml</code> is both the config and the workspace-root anchor. Run{" "}
              <code>tsr &lt;task&gt;</code> from anywhere in the repo.
            </p>
            <ul>
              <li>
                <code>run</code> — spawn a command directly.
              </li>
              <li>
                <code>packages</code> — fan out across a monorepo (glob or manifest name).
              </li>
              <li>
                <code>delegate</code> — hand off to Turbo, Make, or any binary.
              </li>
              <li>
                <code>deps</code> + <code>parallel</code> — the graph, opt-in concurrency.
              </li>
            </ul>
            <div class="cta-row" style="margin-top:20px">
              <Link class="btn btn-ghost" href="/docs/configuration">
                Configuration reference <ArrowRightIcon />
              </Link>
            </div>
          </div>

          <div class="codeblock">
            <pre>
              <span class="cm"># tasks.toml</span>{"\n"}
              <span class="k">[workspace]</span>{"\n"}
              members = [<span class="s">"apps/*"</span>, <span class="s">"packages/*"</span>]{"\n"}
              {"\n"}
              <span class="k">[tasks.dev]</span>{"\n"}
              run = <span class="s">"vite"</span>{"\n"}
              dir = <span class="s">"apps/web"</span>{"\n"}
              {"\n"}
              <span class="k">[tasks.test]</span>{"\n"}
              packages = [<span class="s">"apps/*"</span>]        <span class="cm"># auto-detect</span>{"\n"}
              {"\n"}
              <span class="k">[tasks.build]</span>{"\n"}
              delegate = <span class="s">"turbo"</span>           <span class="cm"># → turbo run build</span>{"\n"}
              {"\n"}
              <span class="k">[tasks.ci]</span>{"\n"}
              deps = [<span class="s">"lint"</span>, <span class="s">"test"</span>, <span class="s">"build"</span>]{"\n"}
              parallel = <span class="t">true</span>
            </pre>
          </div>
        </div>
      </section>

    </>
  );
}
