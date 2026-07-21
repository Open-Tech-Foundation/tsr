// Sync benchmark data into the website.
//
// Reads hyperfine's JSON exports (`benches/results/<scenario>.json`, produced by
// `benches/run.sh`) and writes a slim, committed snapshot the benchmarks page
// imports: `app/docs/benchmarks/data.json`. The numbers come straight from the
// benchmark — this script only drops the per-run `times` arrays and converts
// seconds → milliseconds. Re-run it after `benches/run.sh` to refresh the page.
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const benchDir = join(here, "../../benches/results");
const outFile = join(here, "../app/docs/benchmarks/data.json");

// Scenario key → human label. Keys match benches/results/<key>.json.
const SCENARIOS = [
  ["startup", "startup — one task that spawns `true`"],
  ["shell", "shell one-liner — `echo $HOME && echo done` ($VAR + &&)"],
  ["steps5", "in-task steps — one task, 5 sequential commands"],
  ["graph5", "dependency graph — one task, 5 dependencies"],
  ["graph10", "dependency graph — one task, 10 dependencies"],
];

function load(scenario) {
  const raw = JSON.parse(readFileSync(join(benchDir, `${scenario}.json`), "utf8"));
  return raw.results.map((r) => ({
    command: r.command,
    meanMs: r.mean * 1000,
    stddevMs: r.stddev * 1000,
    minMs: r.min * 1000,
    maxMs: r.max * 1000,
  }));
}

const scenarios = {};
for (const [key, label] of SCENARIOS) {
  scenarios[key] = { label, results: load(key) };
}

const data = {
  meta: {
    generatedAt: new Date().toISOString(),
    source: "benches/results/*.json (hyperfine --export-json)",
    harness: "hyperfine, 20 warmup runs; graph scenarios chain npm/bun with &&",
  },
  scenarios,
};

mkdirSync(dirname(outFile), { recursive: true });
writeFileSync(outFile, JSON.stringify(data, null, 2) + "\n");
console.log(`wrote ${outFile} (${Object.keys(scenarios).length} scenarios)`);
