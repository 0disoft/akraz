import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  RELEASE_METADATA_SCHEMA_VERSION,
  evaluateReleaseMetadataVersions,
  readReleaseMetadata,
} from "./verify-release-metadata.mjs";
import {
  SIGNING_PREFLIGHT_SCHEMA_VERSION,
  evaluateSigningPreflight,
} from "./smoke-signing-preflight.mjs";
import {
  UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION,
  evaluateUpdaterConfigPreflight,
  readTauriConfig,
} from "./smoke-updater-config-preflight.mjs";
import {
  WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  evaluateWindowsMvpQaReport,
} from "./windows-mvp-qa-report.mjs";
import {
  DEFAULT_DURATION_MS,
  WINDOWS_MVP_SOAK_SCHEMA_VERSION,
  WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
  assertSoakSummaryHealthy,
  buildSoakQaEvidence,
} from "./windows-mvp-soak-report.mjs";

export const WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION = "akraz.windowsMvpReleaseGate/v1";

export function buildWindowsMvpReleaseGateReport(
  options = {},
  workspaceRoot = currentWorkspaceRoot(),
) {
  const releaseMetadata = evaluateReleaseMetadataVersions(readReleaseMetadata(workspaceRoot));
  const checks = [
    evaluateReleaseMetadataEvidence(releaseMetadata),
    evaluateQaReportEvidence(readEvidenceJson(options.qaReportFile), options.qaReportFile),
    evaluateSoakReportEvidence(readEvidenceJson(options.soakReportFile), options.soakReportFile),
    evaluateSigningPreflightEvidence(
      options.signingPreflightFile === undefined
        ? evaluateSigningPreflight(process.env)
        : readEvidenceJson(options.signingPreflightFile).payload,
      options.signingPreflightFile,
    ),
    evaluateUpdaterConfigEvidence(
      options.updaterConfigPreflightFile === undefined
        ? evaluateUpdaterConfigPreflight(readTauriConfig(workspaceRoot))
        : readEvidenceJson(options.updaterConfigPreflightFile).payload,
      options.updaterConfigPreflightFile,
    ),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
    releaseTarget: "Windows MVP alpha",
    ready: checks.every((check) => check.status === "pass"),
    requiredSoakDurationMs: DEFAULT_DURATION_MS,
    checks,
    nextActions: buildReleaseGateNextActions(checks),
    privacy: {
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
    },
  };
}

export function parseWindowsMvpReleaseGateArgs(args) {
  const options = {
    outFile: undefined,
    qaReportFile: undefined,
    signingPreflightFile: undefined,
    soakReportFile: undefined,
    updaterConfigPreflightFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      case "--qa-report-file":
        options.qaReportFile = readValue(args, ++index, arg);
        break;
      case "--signing-preflight-file":
        options.signingPreflightFile = readValue(args, ++index, arg);
        break;
      case "--soak-report-file":
        options.soakReportFile = readValue(args, ++index, arg);
        break;
      case "--updater-config-preflight-file":
        options.updaterConfigPreflightFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release gate argument: ${arg}`);
    }
  }

  return options;
}

export function exitCodeForWindowsMvpReleaseGate(report) {
  return report.ready ? 0 : 1;
}

export function writeWindowsMvpReleaseGateOutputFile(outFile, payload) {
  if (!outFile) {
    return undefined;
  }

  const resolvedOutFile = resolve(outFile);
  const outDirectory = dirname(resolvedOutFile);
  const tempFile = resolve(
    outDirectory,
    `.${basename(resolvedOutFile)}.${process.pid}.${Date.now()}.tmp`,
  );
  const serializedPayload = `${JSON.stringify(payload, null, 2)}\n`;

  mkdirSync(outDirectory, { recursive: true });

  let fileDescriptor;
  try {
    fileDescriptor = openSync(tempFile, "w", 0o600);
    writeFileSync(fileDescriptor, serializedPayload, "utf8");
    fsyncSync(fileDescriptor);
    closeSync(fileDescriptor);
    fileDescriptor = undefined;
    renameSync(tempFile, resolvedOutFile);
  } catch (error) {
    if (fileDescriptor !== undefined) {
      closeSync(fileDescriptor);
    }
    if (existsSync(tempFile)) {
      rmSync(tempFile, { force: true });
    }
    throw error;
  }

  return resolvedOutFile;
}

function evaluateReleaseMetadataEvidence(report) {
  return {
    id: "releaseMetadata",
    source: "workspace",
    status: report.ready ? "pass" : "invalid",
    schemaVersion: report.schemaVersion,
    expectedSchemaVersion: RELEASE_METADATA_SCHEMA_VERSION,
    expectedVersion: report.expectedVersion,
    failingCheckIds: report.checks
      .filter((check) => check.status !== "pass")
      .map((check) => check.id),
  };
}

function evaluateQaReportEvidence(fileRead, filePath) {
  const fileCheck = evaluateEvidenceFile(fileRead, "qaReportFile");
  if (fileCheck.status !== "pass") {
    return fileCheck;
  }

  const report = fileRead.payload;
  if (report?.schemaVersion !== WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION) {
    return schemaMismatchCheck(
      "qaReport",
      "qaReportFile",
      WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
      report,
    );
  }

  const evaluation = evaluateWindowsMvpQaReport(report);
  return {
    id: "qaReport",
    source: "qaReportFile",
    status: evaluation.ready ? "pass" : "invalid",
    fileProvided: Boolean(filePath),
    schemaVersion: report.schemaVersion,
    expectedSchemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    summary: evaluation.summary,
    failingCheckIds: evaluation.checks
      .filter((check) => check.status !== "pass")
      .map((check) => check.id),
    nextActions: evaluation.nextActions,
  };
}

function evaluateSoakReportEvidence(fileRead, filePath) {
  const fileCheck = evaluateEvidenceFile(fileRead, "soakReportFile");
  if (fileCheck.status !== "pass") {
    return fileCheck;
  }

  const report = fileRead.payload;
  if (report?.schemaVersion !== WINDOWS_MVP_SOAK_SCHEMA_VERSION) {
    return schemaMismatchCheck(
      "soakReport",
      "soakReportFile",
      WINDOWS_MVP_SOAK_SCHEMA_VERSION,
      report,
    );
  }

  const durationStatus = evaluateSoakDuration(report);
  if (durationStatus.status !== "pass") {
    return {
      ...durationStatus,
      fileProvided: Boolean(filePath),
      schemaVersion: report.schemaVersion,
      expectedSchemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
    };
  }

  try {
    assertSoakSummaryHealthy(report);
  } catch (error) {
    return {
      id: "soakReport",
      source: "soakReportFile",
      status: "invalid",
      detail: "soakSummaryUnhealthy",
      fileProvided: Boolean(filePath),
      schemaVersion: report.schemaVersion,
      expectedSchemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
      failureCount: Array.isArray(report.failures) ? report.failures.length : null,
      metrics: sanitizeSoakMetrics(report.metrics),
      reason: error instanceof Error ? error.message : "Windows MVP soak summary is unhealthy",
    };
  }

  const qaEvidenceStatus = evaluateSoakQaEvidence(report);
  if (qaEvidenceStatus.status !== "pass") {
    return {
      id: "soakReport",
      source: "soakReportFile",
      status: "invalid",
      detail: qaEvidenceStatus.detail,
      fileProvided: Boolean(filePath),
      schemaVersion: report.schemaVersion,
      expectedSchemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
      requestedDurationMs: report.requestedDurationMs,
      elapsedMs: report.elapsedMs,
      completedRuns: report.completedRuns,
      completedCycles: report.completedCycles,
      metrics: sanitizeSoakMetrics(report.metrics),
      qaEvidence: qaEvidenceStatus.qaEvidence,
      expectedQaEvidence: qaEvidenceStatus.expectedQaEvidence,
    };
  }

  return {
    id: "soakReport",
    source: "soakReportFile",
    status: "pass",
    fileProvided: Boolean(filePath),
    schemaVersion: report.schemaVersion,
    expectedSchemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
    requestedDurationMs: report.requestedDurationMs,
    elapsedMs: report.elapsedMs,
    completedRuns: report.completedRuns,
    completedCycles: report.completedCycles,
    metrics: sanitizeSoakMetrics(report.metrics),
    qaEvidence: qaEvidenceStatus.qaEvidence,
  };
}

function evaluateSigningPreflightEvidence(report, filePath) {
  if (report?.schemaVersion !== SIGNING_PREFLIGHT_SCHEMA_VERSION) {
    return schemaMismatchCheck(
      "signingPreflight",
      filePath ? "signingPreflightFile" : "environment",
      SIGNING_PREFLIGHT_SCHEMA_VERSION,
      report,
      {
        fileProvided: Boolean(filePath),
      },
    );
  }

  return {
    id: "signingPreflight",
    source: filePath ? "signingPreflightFile" : "environment",
    status: report.ready ? "pass" : "invalid",
    fileProvided: Boolean(filePath),
    schemaVersion: report.schemaVersion,
    expectedSchemaVersion: SIGNING_PREFLIGHT_SCHEMA_VERSION,
    failingCheckIds: report.checks
      .filter((check) => check.status !== "pass")
      .map((check) => check.id),
    privacy: report.privacy,
  };
}

function evaluateUpdaterConfigEvidence(report, filePath) {
  if (report?.schemaVersion !== UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION) {
    return schemaMismatchCheck(
      "updaterConfigPreflight",
      filePath ? "updaterConfigPreflightFile" : "workspaceConfig",
      UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION,
      report,
      {
        fileProvided: Boolean(filePath),
      },
    );
  }

  return {
    id: "updaterConfigPreflight",
    source: filePath ? "updaterConfigPreflightFile" : "workspaceConfig",
    status: report.ready ? "pass" : "invalid",
    fileProvided: Boolean(filePath),
    schemaVersion: report.schemaVersion,
    expectedSchemaVersion: UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION,
    failingCheckIds: report.checks
      .filter((check) => check.status !== "pass")
      .map((check) => check.id),
    privacy: report.privacy,
  };
}

function evaluateEvidenceFile(fileRead, source) {
  if (!fileRead.pathProvided) {
    return {
      id: source.replace(/File$/, ""),
      source,
      status: "missing",
      detail: "fileArgumentMissing",
    };
  }

  if (fileRead.status !== "pass") {
    return {
      id: source.replace(/File$/, ""),
      source,
      status: "invalid",
      detail: fileRead.detail,
      errorCode: fileRead.errorCode,
    };
  }

  return {
    id: source.replace(/File$/, ""),
    source,
    status: "pass",
  };
}

function readEvidenceJson(path) {
  if (path === undefined) {
    return {
      pathProvided: false,
      status: "missing",
    };
  }

  try {
    return {
      pathProvided: true,
      status: "pass",
      payload: JSON.parse(readFileSync(path, "utf8")),
    };
  } catch (error) {
    return {
      pathProvided: true,
      status: "invalid",
      detail: error instanceof SyntaxError ? "jsonParseFailed" : "fileReadFailed",
      errorCode: typeof error?.code === "string" ? error.code : undefined,
    };
  }
}

function evaluateSoakDuration(report) {
  if (
    !Number.isSafeInteger(report.requestedDurationMs) ||
    !Number.isSafeInteger(report.elapsedMs)
  ) {
    return {
      id: "soakReport",
      source: "soakReportFile",
      status: "invalid",
      detail: "durationFieldsInvalid",
      requiredSoakDurationMs: DEFAULT_DURATION_MS,
    };
  }

  if (report.requestedDurationMs < DEFAULT_DURATION_MS || report.elapsedMs < DEFAULT_DURATION_MS) {
    return {
      id: "soakReport",
      source: "soakReportFile",
      status: "invalid",
      detail: "durationBelowReleaseMinimum",
      requiredSoakDurationMs: DEFAULT_DURATION_MS,
      requestedDurationMs: report.requestedDurationMs,
      elapsedMs: report.elapsedMs,
    };
  }

  return {
    status: "pass",
  };
}

function evaluateSoakQaEvidence(report) {
  const expectedQaEvidence = buildSoakQaEvidence(
    report.metrics ?? {},
    Array.isArray(report.failures) ? report.failures : [],
  );
  const qaEvidence = sanitizeSoakQaEvidence(report.qaEvidence);

  if (
    !report.qaEvidence ||
    typeof report.qaEvidence !== "object" ||
    Array.isArray(report.qaEvidence)
  ) {
    return {
      status: "invalid",
      detail: "soakQaEvidenceMissing",
      qaEvidence,
      expectedQaEvidence,
    };
  }

  if (
    !sameStringArray(qaEvidence.supportedCaseIds, WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS) ||
    qaEvidence.supportedCaseCount !== WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length
  ) {
    return {
      status: "invalid",
      detail: "soakQaEvidenceCoverageMismatch",
      qaEvidence,
      expectedQaEvidence,
    };
  }

  if (
    qaEvidence.status !== expectedQaEvidence.status ||
    !sameStringArray(qaEvidence.blockers, expectedQaEvidence.blockers)
  ) {
    return {
      status: "invalid",
      detail: "soakQaEvidenceDrift",
      qaEvidence,
      expectedQaEvidence,
    };
  }

  if (qaEvidence.status !== "pass") {
    return {
      status: "invalid",
      detail: "soakQaEvidenceNotPassing",
      qaEvidence,
      expectedQaEvidence,
    };
  }

  return {
    status: "pass",
    qaEvidence,
    expectedQaEvidence,
  };
}

function sanitizeSoakMetrics(metrics) {
  return {
    scenarioPasses: safeMetric(metrics?.scenarioPasses),
    scenarioFailures: safeMetric(metrics?.scenarioFailures),
    scenarioTimeouts: safeMetric(metrics?.scenarioTimeouts),
    stuckInputSuspicions: safeMetric(metrics?.stuckInputSuspicions),
    finalPeerLeaks: safeMetric(metrics?.finalPeerLeaks),
  };
}

function sanitizeSoakQaEvidence(qaEvidence) {
  return {
    status: typeof qaEvidence?.status === "string" ? qaEvidence.status : null,
    supportedCaseIds: sanitizeStringArray(qaEvidence?.supportedCaseIds),
    supportedCaseCount: safeMetric(qaEvidence?.supportedCaseCount),
    blockers: sanitizeStringArray(qaEvidence?.blockers),
  };
}

function safeMetric(value) {
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

function sanitizeStringArray(value) {
  return Array.isArray(value) ? value.filter((item) => typeof item === "string") : [];
}

function sameStringArray(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function schemaMismatchCheck(id, source, expectedSchemaVersion, report, extra = {}) {
  return {
    id,
    source,
    status: "invalid",
    detail: "schemaVersionMismatch",
    expectedSchemaVersion,
    actualSchemaVersion: typeof report?.schemaVersion === "string" ? report.schemaVersion : null,
    ...extra,
  };
}

function buildReleaseGateNextActions(checks) {
  return checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    if (check.id === "qaReport" && Array.isArray(check.nextActions)) {
      return check.nextActions.map((action) => ({
        gate: "qaReport",
        ...action,
      }));
    }

    return [
      {
        gate: check.id,
        action: releaseGateActionFor(check),
      },
    ];
  });
}

function releaseGateActionFor(check) {
  switch (check.detail) {
    case "fileArgumentMissing":
      return `provide --${dashCase(check.source)} with a release evidence JSON file`;
    case "fileReadFailed":
    case "jsonParseFailed":
      return `replace ${check.source} with a readable JSON evidence file`;
    case "schemaVersionMismatch":
      return `regenerate ${check.id} using the current Akraz evidence script`;
    case "durationBelowReleaseMinimum":
      return "run the Windows MVP soak for the full release minimum duration";
    case "durationFieldsInvalid":
      return "regenerate the Windows MVP soak report with duration fields";
    case "soakSummaryUnhealthy":
      return "fix the soak failure and rerun Windows MVP soak evidence";
    case "soakQaEvidenceMissing":
      return "regenerate the Windows MVP soak report with QA evidence fields";
    case "soakQaEvidenceCoverageMismatch":
    case "soakQaEvidenceDrift":
      return "regenerate the Windows MVP soak report instead of editing evidence by hand";
    case "soakQaEvidenceNotPassing":
      return "rerun Windows MVP soak until QA evidence status is pass";
    default:
      if (check.id === "signingPreflight") {
        return "run release signing preflight in the release environment until every check passes";
      }
      if (check.id === "updaterConfigPreflight") {
        return "fix updater config preflight until every check passes";
      }
      if (check.id === "releaseMetadata") {
        return "synchronize release metadata versions across package, Tauri, Cargo, and lock files";
      }
      return "review and regenerate the failing Windows MVP release evidence";
  }
}

function dashCase(value) {
  return String(value)
    .replace(/([a-z])([A-Z])/g, "$1-$2")
    .toLowerCase();
}

function currentWorkspaceRoot() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  return join(appRoot, "..", "..");
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseGateArgs(process.argv.slice(2));
  const report = buildWindowsMvpReleaseGateReport(options);
  writeWindowsMvpReleaseGateOutputFile(options.outFile, report);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseGate(report));
}
