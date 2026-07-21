import { Link } from "@opentf/web";

const TASKS_TOML = `# tasks.toml
[workspace]
members = ["apps/*", "packages/*"]

[tasks.dev]
run = "vite"
dir = "apps/web"

[tasks.test]
packages = ["apps/*"]        # auto-detect

[tasks.build]
delegate = "turbo"           # → turbo run build

[tasks.ci]
deps = ["lint", "test", "build"]
parallel = true
`;

const INSTALL_SH = `# build the binary
cargo build --release   # → target/release/tsr

# scaffold a config and run
tsr --init
tsr dev
tsr test -- --watch
`;

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
              a command runner, not a build system
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
            <div class="cta-row">
              <Link class="btn btn-primary" href="/docs">
                Get started →
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
                <code>$VAR</code>, <code>&amp;&amp; || ;</code> and quoting are supported;
                pipes, redirects and globs are rejected up front, not half-run.
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
                Configuration reference →
              </Link>
            </div>
          </div>

          <div class="codeblock">
            <pre>{TASKS_TOML}</pre>
          </div>
        </div>
      </section>

      {/* --- install --- */}
      <section class="section">
        <div class="container">
          <h2>Install</h2>
          <p class="sub">A single static binary. Build from source with Cargo.</p>
          <div class="codeblock">
            <pre>{INSTALL_SH}</pre>
          </div>
        </div>
      </section>
    </>
  );
}
