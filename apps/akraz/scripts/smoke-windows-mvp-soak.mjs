import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const DEFAULT_DURATION_MS = 120 * 60 * 1000;
const DEFAULT_CYCLE_DELAY_MS = 1000;

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);

const scenarios = [
  {
    name: "loopback-transport",
    script: "smoke-loopback-transport.mjs",
  },
  {
    name: "peer-session",
    script: "smoke-peer-session.mjs",
  },
  {
    name: "peer-session-executor",
    script: "smoke-peer-session-executor.mjs",
  },
  {
    name: "tcp-transport",
    script: "smoke-tcp-transport.mjs",
  },
  {
    name: "session-connect-lifecycle",
    script: "smoke-session-connect-lifecycle.mjs",
  },
];

const options = parseOptions(process.argv.slice(2));

const availableScenarioNames = scenarios.map((scenario) => scenario.name);

if (options.list) {
  console.log(JSON.stringify({ scenarios: availableScenarioNames }, null, 2));
  process.exit(0);
}

const unknownScenarios = [...options.scenarios].filter(
  (scenario) => !availableScenarioNames.includes(scenario),
);
if (unknownScenarios.length > 0) {
  throw new Error(
    `unknown soak scenario(s): ${unknownScenarios.join(
      ", ",
    )}; available scenarios: ${availableScenarioNames.join(", ")}`,
  );
}

const selectedScenarios =
  options.scenarios.size === 0
    ? scenarios
    : scenarios.filter((scenario) => options.scenarios.has(scenario.name));

const startedAt = new Date();
const deadline = Date.now() + options.durationMs;
const cycleDelay = new Int32Array(new SharedArrayBuffer(4));
const failures = [];
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
    });

    if (result.stdout) {
      process.stdout.write(result.stdout);
    }
    if (result.stderr) {
      process.stderr.write(result.stderr);
    }
    if (result.error) {
      throw result.error;
    }

    completedRuns += 1;
    const elapsedMs = Date.now() - started;
    if (result.status !== 0) {
      failures.push({
        cycle: completedCycles,
        scenario: scenario.name,
        exitCode: result.status,
        elapsedMs,
      });
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

const summary = {
  startedAt: startedAt.toISOString(),
  finishedAt: new Date().toISOString(),
  durationMs: options.durationMs,
  maxCycles: Number.isFinite(options.maxCycles) ? options.maxCycles : null,
  cycleDelayMs: options.cycleDelayMs,
  scenarios: selectedScenarios.map((scenario) => scenario.name),
  completedCycles,
  completedRuns,
  failures,
};

console.log(JSON.stringify(summary, null, 2));

if (failures.length > 0) {
  throw new Error(
    `Windows MVP soak failed in cycle ${failures[0].cycle} during ${failures[0].scenario}`,
  );
}

console.log("Windows MVP soak passed.");

function parseOptions(args) {
  const parsedOptions = {
    cycleDelayMs: DEFAULT_CYCLE_DELAY_MS,
    durationMs: DEFAULT_DURATION_MS,
    list: false,
    maxCycles: Number.POSITIVE_INFINITY,
    scenarios: new Set(),
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--cycle-delay-ms":
        parsedOptions.cycleDelayMs = parsePositiveInteger(readValue(args, ++index, arg), arg);
        break;
      case "--duration-minutes":
        parsedOptions.durationMs =
          parsePositiveInteger(readValue(args, ++index, arg), arg) * 60 * 1000;
        break;
      case "--duration-ms":
        parsedOptions.durationMs = parsePositiveInteger(readValue(args, ++index, arg), arg);
        break;
      case "--list":
        parsedOptions.list = true;
        break;
      case "--max-cycles":
        parsedOptions.maxCycles = parsePositiveInteger(readValue(args, ++index, arg), arg);
        break;
      case "--scenario":
        parsedOptions.scenarios.add(readValue(args, ++index, arg));
        break;
      default:
        throw new Error(`unknown soak option: ${arg}`);
    }
  }

  return parsedOptions;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }

  return value;
}

function parsePositiveInteger(value, flag) {
  if (!/^\d+$/.test(value)) {
    throw new Error(`${flag} must be a positive integer`);
  }

  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${flag} must be a positive safe integer`);
  }

  return parsed;
}
