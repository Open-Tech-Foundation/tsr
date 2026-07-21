// Renders one benchmark scenario as a sorted horizontal bar chart from the
// hyperfine data synced into ./data.json (see tools/sync-bench.mjs). Static —
// no client state. `tsr`'s bar is highlighted; bars are scaled linearly to the
// slowest runner, so the interpreter-based runners (npm, go-task) tower over the
// native ones, which is exactly the point being made.
export default function BenchChart({ scenario }) {
  const results = [...scenario.results].sort((a, b) => a.meanMs - b.meanMs);
  const fastest = results[0].meanMs;
  const slowest = results[results.length - 1].meanMs;

  return (
    <div class="bench">
      <div class="bench-bars">
        {results.map((r) => {
          const pct = Math.max((r.meanMs / slowest) * 100, 1.2);
          const rel = r.meanMs / fastest;
          const isTsr = r.command === "tsr";
          return (
            <div class={isTsr ? "bench-row bench-row-tsr" : "bench-row"}>
              <span class="bench-name">{r.command}</span>
              <div class="bench-track">
                <div class="bench-bar" style={`width:${pct}%`} />
                <span class="bench-val">
                  {r.meanMs.toFixed(1)} ms
                  <span class="bench-rel"> · {rel.toFixed(1)}×</span>
                </span>
              </div>
            </div>
          );
        })}
      </div>
      <p class="bench-caption">
        Mean wall-clock, lower is faster. <code>×</code> is relative to the fastest
        runner ({results[0].command}, {fastest.toFixed(1)} ms).
      </p>
    </div>
  );
}
