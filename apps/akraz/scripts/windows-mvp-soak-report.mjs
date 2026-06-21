import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, resolve } from "node:path";

export const WINDOWS_MVP_SOAK_SCHEMA_VERSION = "akraz.windowsMvpSoak/v1";
export const SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION =
  "akraz.sessionConnectLifecycleSmoke/v1";
export const DEFAULT_DURATION_MS = 120 * 60 * 1000;
export const DEFAULT_CYCLE_DELAY_MS = 1000;
export const DEFAULT_SCENARIO_TIMEOUT_MS = 10 * 60 * 1000;
export const WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS = ["WIN-002", "WIN-003", "WIN-006", "WIN-008"];

export const windowsMvpSoakScenarios = [
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
    name: "remote-control-loopback",
    script: "smoke-remote-control-loopback.mjs",
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

export function parseSoakOptions(args) {
  const parsedOptions = {
    cycleDelayMs: DEFAULT_CYCLE_DELAY_MS,
    durationMs: DEFAULT_DURATION_MS,
    list: false,
    maxCycles: Number.POSITIVE_INFINITY,
    reportFile: undefined,
    scenarioTimeoutMs: DEFAULT_SCENARIO_TIMEOUT_MS,
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
      case "--report-file":
        parsedOptions.reportFile = parseReportFile(readValue(args, ++index, arg), arg);
        break;
      case "--scenario":
        parsedOptions.scenarios.add(readValue(args, ++index, arg));
        break;
      case "--scenario-timeout-ms":
        parsedOptions.scenarioTimeoutMs = parsePositiveInteger(readValue(args, ++index, arg), arg);
        break;
      default:
        throw new Error(`unknown soak option: ${arg}`);
    }
  }

  return parsedOptions;
}

export function listSoakScenarioNames(scenarios = windowsMvpSoakScenarios) {
  return scenarios.map((scenario) => scenario.name);
}

export function selectSoakScenarios(options, scenarios = windowsMvpSoakScenarios) {
  const availableScenarioNames = listSoakScenarioNames(scenarios);
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

  return options.scenarios.size === 0
    ? scenarios
    : scenarios.filter((scenario) => options.scenarios.has(scenario.name));
}

export function createEmptySoakMetrics() {
  return {
    scenarioPasses: 0,
    scenarioFailures: 0,
    scenarioTimeouts: 0,
    remoteSessionStarts: 0,
    remoteSessionStops: 0,
    forwardedInputCommands: 0,
    forwardedInputOutcomes: 0,
    injectedInputEvents: 0,
    releaseAllCommands: 0,
    releaseAllOutcomes: 0,
    platformReleaseAllCalls: 0,
    sessionConnects: 0,
    sessionDisconnects: 0,
    finalPeerLeaks: 0,
    stuckInputSuspicions: 0,
  };
}

export function mergeSoakMetrics(target, source) {
  for (const [key, value] of Object.entries(source)) {
    target[key] = (target[key] ?? 0) + value;
  }
  return target;
}

export function parseLastJsonObject(output) {
  const text = String(output ?? "");
  for (
    let endIndex = text.lastIndexOf("}");
    endIndex !== -1;
    endIndex = text.lastIndexOf("}", endIndex - 1)
  ) {
    for (
      let startIndex = text.lastIndexOf("{", endIndex);
      startIndex !== -1;
      startIndex = text.lastIndexOf("{", startIndex - 1)
    ) {
      try {
        const value = JSON.parse(text.slice(startIndex, endIndex + 1));
        if (value && typeof value === "object" && !Array.isArray(value)) {
          return value;
        }
      } catch {
        continue;
      }
    }
  }

  return undefined;
}

export function collectScenarioMetrics(scenarioName, report) {
  if (!report || typeof report !== "object") {
    throw new Error(`${scenarioName} did not emit a machine-readable JSON report`);
  }

  const metrics = createEmptySoakMetrics();
  collectCommandMetrics(metrics, report.commands);
  collectOutcomeMetrics(metrics, report.outcomes);

  if (Array.isArray(report.injectedInputs)) {
    metrics.injectedInputEvents += report.injectedInputs.length;
  }
  if (Number.isSafeInteger(report.releaseAllCount) && report.releaseAllCount > 0) {
    metrics.platformReleaseAllCalls += report.releaseAllCount;
  }
  if (report.schemaVersion === SESSION_CONNECT_LIFECYCLE_SMOKE_SCHEMA_VERSION) {
    if (report.connected === true) {
      metrics.sessionConnects += 1;
    }
    if (report.disconnected === true) {
      metrics.sessionDisconnects += 1;
    }
    if (Number.isSafeInteger(report.finalPeerCount) && report.finalPeerCount > 0) {
      metrics.finalPeerLeaks += report.finalPeerCount;
    }
  }

  metrics.stuckInputSuspicions = countStuckInputSuspicions(metrics);

  return metrics;
}

export function buildScenarioFailure({
  cycle,
  elapsedMs,
  errorCode,
  errorMessage,
  exitCode,
  scenario,
  signal,
  timedOut,
}) {
  return {
    cycle,
    scenario,
    exitCode,
    signal,
    elapsedMs,
    timedOut,
    ...(errorCode ? { errorCode } : {}),
    ...(errorMessage ? { errorMessage } : {}),
  };
}

export function buildSoakSummary({
  completedCycles,
  completedRuns,
  failures,
  finishedAt,
  metrics,
  options,
  scenarios,
  startedAt,
}) {
  const elapsedMs = Math.max(0, finishedAt.getTime() - startedAt.getTime());
  const finalMetrics = {
    ...metrics,
    scenarioFailures: failures.length,
    scenarioTimeouts: failures.filter((failure) => failure.timedOut).length,
  };

  return {
    schemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
    startedAt: startedAt.toISOString(),
    finishedAt: finishedAt.toISOString(),
    requestedDurationMs: options.durationMs,
    elapsedMs,
    maxCycles: Number.isFinite(options.maxCycles) ? options.maxCycles : null,
    cycleDelayMs: options.cycleDelayMs,
    scenarioTimeoutMs: options.scenarioTimeoutMs,
    scenarios: scenarios.map((scenario) => scenario.name),
    completedCycles,
    completedRuns,
    metrics: finalMetrics,
    qaEvidence: buildSoakQaEvidence(finalMetrics, failures),
    failures,
  };
}

export function buildSoakQaEvidence(metrics, failures = []) {
  const blockers = [];
  const forwardedInputs =
    metrics.forwardedInputCommands + metrics.forwardedInputOutcomes + metrics.injectedInputEvents;
  const releaseSignals =
    metrics.releaseAllCommands + metrics.releaseAllOutcomes + metrics.platformReleaseAllCalls;

  if (failures.length > 0 || metrics.scenarioFailures > 0) {
    blockers.push("scenarioFailures");
  }
  if (metrics.scenarioTimeouts > 0) {
    blockers.push("scenarioTimeouts");
  }
  if (metrics.stuckInputSuspicions > 0) {
    blockers.push("stuckInputSuspicions");
  }
  if (metrics.finalPeerLeaks > 0) {
    blockers.push("finalPeerLeaks");
  }

  const failed = blockers.length > 0;

  if (metrics.scenarioPasses <= 0) {
    blockers.push("scenarioPassesMissing");
  }
  if (metrics.remoteSessionStarts <= 0) {
    blockers.push("remoteSessionStartMissing");
  }
  if (metrics.remoteSessionStops <= 0) {
    blockers.push("remoteSessionStopMissing");
  }
  if (forwardedInputs <= 0) {
    blockers.push("remoteInputMissing");
  }
  if (releaseSignals <= 0) {
    blockers.push("releaseSignalMissing");
  }

  return {
    supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
    supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
    status: failed ? "failed" : blockers.length === 0 ? "pass" : "insufficient",
    blockers,
  };
}

export function assertSoakSummaryHealthy(summary) {
  if (summary.failures.length > 0) {
    const [failure] = summary.failures;
    throw new Error(`Windows MVP soak failed in cycle ${failure.cycle} during ${failure.scenario}`);
  }
  if (summary.completedRuns <= 0) {
    throw new Error("Windows MVP soak did not run any scenario");
  }
  if (summary.metrics.stuckInputSuspicions > 0) {
    throw new Error(
      `Windows MVP soak reported ${summary.metrics.stuckInputSuspicions} stuck input suspicion(s)`,
    );
  }
  if (summary.metrics.finalPeerLeaks > 0) {
    throw new Error(`Windows MVP soak reported ${summary.metrics.finalPeerLeaks} leaked peer(s)`);
  }
}

export function writeSoakSummaryReportFile(reportFile, summary) {
  if (!reportFile) {
    return undefined;
  }

  const resolvedReportFile = resolve(reportFile);
  const reportDirectory = dirname(resolvedReportFile);
  const tempFile = resolve(
    reportDirectory,
    `.${basename(resolvedReportFile)}.${process.pid}.${Date.now()}.tmp`,
  );
  const payload = `${JSON.stringify(summary, null, 2)}\n`;

  mkdirSync(reportDirectory, { recursive: true });

  let fileDescriptor;
  try {
    fileDescriptor = openSync(tempFile, "w", 0o600);
    writeFileSync(fileDescriptor, payload, "utf8");
    fsyncSync(fileDescriptor);
    closeSync(fileDescriptor);
    fileDescriptor = undefined;
    renameSync(tempFile, resolvedReportFile);
  } catch (error) {
    if (fileDescriptor !== undefined) {
      closeSync(fileDescriptor);
    }
    if (existsSync(tempFile)) {
      rmSync(tempFile, { force: true });
    }
    throw error;
  }

  return resolvedReportFile;
}

function collectCommandMetrics(metrics, commands) {
  if (!Array.isArray(commands)) {
    return;
  }

  for (const command of commands) {
    switch (command?.kind) {
      case "startRemoteSession":
        metrics.remoteSessionStarts += 1;
        break;
      case "forwardInput":
        metrics.forwardedInputCommands += 1;
        break;
      case "releaseAllInputs":
        metrics.releaseAllCommands += 1;
        break;
      case "stopRemoteSession":
        metrics.remoteSessionStops += 1;
        break;
    }
  }
}

function collectOutcomeMetrics(metrics, outcomes) {
  if (!Array.isArray(outcomes)) {
    return;
  }

  for (const outcome of outcomes) {
    switch (outcome?.kind) {
      case "remoteSessionStarted":
        metrics.remoteSessionStarts += 1;
        break;
      case "inputForwarded":
        metrics.forwardedInputOutcomes += 1;
        break;
      case "inputsReleased":
        metrics.releaseAllOutcomes += 1;
        break;
      case "remoteSessionStopped":
        metrics.remoteSessionStops += 1;
        break;
    }
  }
}

function countStuckInputSuspicions(metrics) {
  const forwardedInputs =
    metrics.forwardedInputCommands + metrics.forwardedInputOutcomes + metrics.injectedInputEvents;
  const releaseSignals =
    metrics.releaseAllCommands + metrics.releaseAllOutcomes + metrics.platformReleaseAllCalls;
  let suspicionCount = 0;

  if (metrics.remoteSessionStarts > 0 && releaseSignals === 0) {
    suspicionCount += 1;
  }
  if (forwardedInputs > 0 && releaseSignals === 0) {
    suspicionCount += 1;
  }
  if (metrics.sessionConnects > metrics.sessionDisconnects) {
    suspicionCount += metrics.sessionConnects - metrics.sessionDisconnects;
  }
  if (metrics.finalPeerLeaks > 0) {
    suspicionCount += metrics.finalPeerLeaks;
  }

  return suspicionCount;
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

function parseReportFile(value, flag) {
  if (value.trim().length === 0) {
    throw new Error(`${flag} requires a non-empty path`);
  }

  return value;
}
