import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertSoakSummaryHealthy,
  buildScenarioFailure,
  buildSoakSummary,
  collectScenarioMetrics,
  createEmptySoakMetrics,
  listSoakScenarioNames,
  mergeSoakMetrics,
  parseLastJsonObject,
  parseSoakOptions,
  selectSoakScenarios,
  writeSoakSummaryReportFile,
} from "./windows-mvp-soak-report.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const options = parseSoakOptions(process.argv.slice(2));

if (options.list) {
  console.log(JSON.stringify({ scenarios: listSoakScenarioNames() }, null, 2));
  process.exit(0);
}

const selectedScenarios = selectSoakScenarios(options);
const startedAt = new Date();
const deadline = Date.now() + options.durationMs;
const cycleDelay = new Int32Array(new SharedArrayBuffer(4));
const failures = [];
const metrics = createEmptySoakMetrics();
let completedCycles = 0;
let completedRuns = 0;

console.log(
  `Windows MVP soak started for ${options.durationMs}ms across ${selectedScenarios.length} scenario(s).`,
);

while (completedCycles < options.maxCycles && (completedCycles === 0 || Date.now() < deadline)) {
  completedCycles += 1;
  console.log(`Soak cycle ${completedCycles} started.`);

  for (const scenario of selectedScenarios) {
    const started = Date.now();
    const result = spawnSync("bun", [join(scriptDir, scenario.script)], {
      cwd: appRoot,
      encoding: "utf8",
      killSignal: "SIGTERM",
      timeout: options.scenarioTimeoutMs,
    });

    if (result.stdout) {
      process.stdout.write(result.stdout);
    }
    if (result.stderr) {
      process.stderr.write(result.stderr);
    }
    completedRuns += 1;
    if (result.error) {
      failures.push(
        buildScenarioFailure({
          cycle: completedCycles,
          scenario: scenario.name,
          exitCode: result.status,
          signal: result.signal,
          elapsedMs: Date.now() - started,
          timedOut: result.error.code === "ETIMEDOUT",
          errorCode: result.error.code,
          errorMessage: result.error.message,
        }),
      );
      break;
    }

    const elapsedMs = Date.now() - started;
    if (result.status !== 0) {
      failures.push(
        buildScenarioFailure({
          cycle: completedCycles,
          scenario: scenario.name,
          exitCode: result.status,
          signal: result.signal,
          elapsedMs,
          timedOut: false,
        }),
      );
      break;
    }

    try {
      const report = parseLastJsonObject(result.stdout);
      mergeSoakMetrics(metrics, collectScenarioMetrics(scenario.name, report));
      metrics.scenarioPasses += 1;
    } catch (error) {
      failures.push(
        buildScenarioFailure({
          cycle: completedCycles,
          scenario: scenario.name,
          exitCode: result.status,
          signal: result.signal,
          elapsedMs,
          timedOut: false,
          errorMessage: error.message,
        }),
      );
      break;
    }

    console.log(`Soak scenario ${scenario.name} passed in ${elapsedMs}ms.`);
  }

  if (failures.length > 0) {
    break;
  }
  if (Date.now() >= deadline || completedCycles >= options.maxCycles) {
    break;
  }

  Atomics.wait(cycleDelay, 0, 0, options.cycleDelayMs);
}

const summary = buildSoakSummary({
  completedCycles,
  completedRuns,
  failures,
  finishedAt: new Date(),
  metrics,
  options,
  scenarios: selectedScenarios,
  startedAt,
});

console.log(JSON.stringify(summary, null, 2));
const reportFile = writeSoakSummaryReportFile(options.reportFile, summary);
if (reportFile) {
  console.log(`Windows MVP soak report written to ${reportFile}.`);
}
assertSoakSummaryHealthy(summary);
console.log("Windows MVP soak passed.");
