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
import { basename, dirname, resolve } from "node:path";

import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  buildWindowsMvpQaPlan,
} from "./windows-mvp-qa-plan.mjs";

export const WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION = "akraz.windowsMvpQaReport/v1";

const RESULT_STATUSES = new Set(["pass", "fail", "blocked", "skipped"]);
const PRIVACY_FLAGS = ["includesTypedContent", "includesSecretValues", "includesFullFilePaths"];

export function evaluateWindowsMvpQaReport(report, plan = buildWindowsMvpQaPlan()) {
  const checks = [
    evaluateReportSchema(report),
    evaluateReportPlanSchema(report),
    evaluateReportExecutionMetadata(report),
    evaluateReportPrivacy(report),
    ...evaluateReportResults(report, plan),
  ];
  const summary = summarizeReportResults(report);

  return {
    schemaVersion: "akraz.windowsMvpQaReportEvaluation/v1",
    reportSchemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass") && summary.failed === 0,
    summary,
    checks,
    privacy: {
      includesReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function buildWindowsMvpQaReportTemplate(options = {}) {
  const plan = buildWindowsMvpQaPlan({ caseIds: options.caseIds ?? [] });

  return {
    schemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    generatedFrom: "qa:windows-mvp-report-template",
    executedAt: null,
    environment: {
      sourceOs: null,
      targetOs: null,
      hardware: null,
    },
    results: plan.cases.map((testCase) => ({
      caseId: testCase.id,
      result: "blocked",
      reason: "not run yet",
      evidence: [],
    })),
    privacy: {
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function updateWindowsMvpQaReportResult(report, options, plan = buildWindowsMvpQaPlan()) {
  const caseId = hasText(options.caseId) ? options.caseId.trim() : "";
  const planCaseIds = new Set(plan.cases.map((testCase) => testCase.id));

  if (!planCaseIds.has(caseId)) {
    throw new Error(
      `unknown Windows MVP QA case id: ${caseId}; available case ids: ${[...planCaseIds].join(
        ", ",
      )}`,
    );
  }

  if (!RESULT_STATUSES.has(options.result)) {
    throw new Error(`unsupported Windows MVP QA result: ${options.result}`);
  }

  if (options.result === "pass" && options.evidence.length === 0) {
    throw new Error("--result pass requires at least one --evidence value");
  }

  if ((options.result === "fail" || options.result === "blocked") && !hasText(options.reason)) {
    throw new Error(`--result ${options.result} requires --reason`);
  }

  if (
    containsUnsafeReportText([
      options.reason ?? "",
      options.evidence,
      options.sourceOs ?? "",
      options.targetOs ?? "",
      options.hardware ?? "",
      options.environmentNotes ?? "",
    ])
  ) {
    throw new Error("updated QA report values contain sensitive or local path data");
  }

  const nextReport = JSON.parse(JSON.stringify(report));
  nextReport.executedAt = options.executedAt ?? nextReport.executedAt ?? null;
  nextReport.environment ??= {};
  nextReport.environment = {
    ...nextReport.environment,
    ...(options.sourceOs !== undefined ? { sourceOs: options.sourceOs } : {}),
    ...(options.targetOs !== undefined ? { targetOs: options.targetOs } : {}),
    ...(options.hardware !== undefined ? { hardware: options.hardware } : {}),
    ...(options.environmentNotes !== undefined ? { notes: options.environmentNotes } : {}),
  };

  const nextResult = {
    caseId,
    result: options.result,
    evidence: options.evidence,
    ...(hasText(options.reason) ? { reason: options.reason } : {}),
  };
  const results = Array.isArray(nextReport.results) ? nextReport.results : [];
  const existingIndex = results.findIndex((result) => result?.caseId === caseId);

  if (existingIndex === -1) {
    results.push(nextResult);
  } else {
    results[existingIndex] = nextResult;
  }
  nextReport.results = results;

  return nextReport;
}

export function readWindowsMvpQaReport(reportFile) {
  return JSON.parse(readFileSync(reportFile, "utf8"));
}

export function writeWindowsMvpQaReportOutputFile(outFile, payload) {
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

export function parseWindowsMvpQaReportArgs(args) {
  const options = {
    template: false,
    updateResult: false,
    reportFile: undefined,
    outFile: undefined,
    caseIds: [],
    result: undefined,
    reason: undefined,
    evidence: [],
    executedAt: undefined,
    sourceOs: undefined,
    targetOs: undefined,
    hardware: undefined,
    environmentNotes: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--template":
        options.template = true;
        break;
      case "--update-result":
        options.updateResult = true;
        break;
      case "--report-file":
        options.reportFile = readValue(args, ++index, arg);
        break;
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      case "--case-id":
        options.caseIds.push(readValue(args, ++index, arg));
        break;
      case "--result":
        options.result = readValue(args, ++index, arg);
        break;
      case "--reason":
        options.reason = readValue(args, ++index, arg);
        break;
      case "--evidence":
        options.evidence.push(readValue(args, ++index, arg));
        break;
      case "--executed-at":
        options.executedAt = readValue(args, ++index, arg);
        break;
      case "--source-os":
        options.sourceOs = readValue(args, ++index, arg);
        break;
      case "--target-os":
        options.targetOs = readValue(args, ++index, arg);
        break;
      case "--hardware":
        options.hardware = readValue(args, ++index, arg);
        break;
      case "--environment-notes":
        options.environmentNotes = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }

  if (options.template && options.updateResult) {
    throw new Error("--template cannot be combined with --update-result");
  }

  if (options.template && options.reportFile !== undefined) {
    throw new Error("--template cannot be combined with --report-file");
  }

  if (options.updateResult) {
    if (options.reportFile === undefined) {
      throw new Error("--update-result requires --report-file");
    }
    if (options.caseIds.length !== 1) {
      throw new Error("--update-result requires exactly one --case-id");
    }
    if (options.result === undefined) {
      throw new Error("--update-result requires --result");
    }
    return options;
  }

  const updateOnlyArgs = [
    ["--result", options.result],
    ["--reason", options.reason],
    ["--evidence", options.evidence.length > 0 ? options.evidence : undefined],
    ["--executed-at", options.executedAt],
    ["--source-os", options.sourceOs],
    ["--target-os", options.targetOs],
    ["--hardware", options.hardware],
    ["--environment-notes", options.environmentNotes],
  ].filter(([, value]) => value !== undefined);
  if (updateOnlyArgs.length > 0) {
    throw new Error(`${updateOnlyArgs[0][0]} can only be used with --update-result`);
  }

  if (!options.template && options.caseIds.length > 0) {
    throw new Error("--case-id can only be used with --template");
  }

  if (!options.template && options.reportFile === undefined) {
    throw new Error("--report-file is required");
  }

  return options;
}

export function exitCodeForWindowsMvpQaReport(evaluation) {
  return evaluation.ready ? 0 : 1;
}

function evaluateReportSchema(report) {
  if (report?.schemaVersion !== WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION) {
    return buildCheck("reportSchema", "invalid", {
      detail: "schemaVersionMismatch",
      expectedSchemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    });
  }

  return buildCheck("reportSchema", "pass");
}

function evaluateReportPlanSchema(report) {
  if (report?.planSchemaVersion === undefined) {
    return buildCheck("planSchema", "missing", {
      detail: "planSchemaVersionMissing",
      expectedPlanSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    });
  }

  if (report.planSchemaVersion !== WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION) {
    return buildCheck("planSchema", "invalid", {
      detail: "planSchemaVersionMismatch",
      expectedPlanSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    });
  }

  return buildCheck("planSchema", "pass");
}

function evaluateReportExecutionMetadata(report) {
  const environment = report?.environment;

  if (!hasStrictIsoUtcTimestamp(report?.executedAt)) {
    return buildCheck("executionMetadata", "invalid", {
      detail: "executedAtMustBeStrictIsoUtc",
    });
  }

  if (!environment || typeof environment !== "object") {
    return buildCheck("executionMetadata", "missing", {
      detail: "environmentMissing",
      fields: ["sourceOs", "targetOs", "hardware"],
    });
  }

  const missingFields = ["sourceOs", "targetOs", "hardware"].filter(
    (field) => !hasText(environment[field]),
  );
  if (missingFields.length > 0) {
    return buildCheck("executionMetadata", "missing", {
      detail: "environmentFieldsMissing",
      fields: missingFields,
    });
  }

  if (
    containsUnsafeReportText([
      environment.sourceOs,
      environment.targetOs,
      environment.hardware,
      environment.notes,
    ])
  ) {
    return buildCheck("executionMetadata", "invalid", {
      detail: "environmentContainsSensitiveOrLocalPathData",
    });
  }

  return buildCheck("executionMetadata", "pass");
}

function evaluateReportPrivacy(report) {
  const privacy = report?.privacy;
  const invalidFlags = PRIVACY_FLAGS.filter((flag) => privacy?.[flag] !== false);

  if (invalidFlags.length > 0) {
    return buildCheck("privacy", "invalid", {
      detail: "privacyFlagsMustBeFalse",
      flags: invalidFlags,
    });
  }

  return buildCheck("privacy", "pass");
}

function evaluateReportResults(report, plan) {
  const planCases = new Map(plan.cases.map((testCase) => [testCase.id, testCase]));
  const results = Array.isArray(report?.results) ? report.results : [];
  const checks = [];

  if (results.length === 0) {
    checks.push(buildCheck("resultsPresent", "missing", { detail: "resultsMissing" }));
    return checks;
  }

  checks.push(buildCheck("resultsPresent", "pass"));

  const seenCaseIds = new Set();
  const duplicateCaseIds = new Set();
  const reportedCaseIds = new Set();

  for (const result of results) {
    const caseId = typeof result?.caseId === "string" ? result.caseId.trim() : "";
    if (caseId.length === 0) {
      checks.push(buildCheck("resultCaseId", "invalid", { detail: "caseIdMissing" }));
      continue;
    }

    if (seenCaseIds.has(caseId)) {
      duplicateCaseIds.add(caseId);
    }
    seenCaseIds.add(caseId);

    if (!planCases.has(caseId)) {
      checks.push(buildCheck(`result:${caseId}`, "invalid", { detail: "unknownCaseId", caseId }));
      continue;
    }

    reportedCaseIds.add(caseId);
    checks.push(evaluateSingleResult(caseId, result));
  }

  if (duplicateCaseIds.size > 0) {
    checks.push(
      buildCheck("duplicateCaseIds", "invalid", {
        detail: "duplicateCaseIds",
        caseIds: [...duplicateCaseIds],
      }),
    );
  } else {
    checks.push(buildCheck("duplicateCaseIds", "pass"));
  }

  const missingReleaseBlockingCaseIds = plan.cases
    .filter((testCase) => testCase.priority === "release-blocking")
    .map((testCase) => testCase.id)
    .filter((caseId) => !reportedCaseIds.has(caseId));

  if (missingReleaseBlockingCaseIds.length > 0) {
    checks.push(
      buildCheck("releaseBlockingCoverage", "missing", {
        detail: "releaseBlockingCasesMissing",
        caseIds: missingReleaseBlockingCaseIds,
      }),
    );
  } else {
    checks.push(buildCheck("releaseBlockingCoverage", "pass"));
  }

  return checks;
}

function evaluateSingleResult(caseId, result) {
  const status = result?.result;

  if (!RESULT_STATUSES.has(status)) {
    return buildCheck(`result:${caseId}`, "invalid", {
      detail: "unsupportedResultStatus",
      caseId,
    });
  }

  if (status === "pass" && !hasEvidence(result)) {
    return buildCheck(`result:${caseId}`, "invalid", {
      detail: "passRequiresEvidence",
      caseId,
    });
  }

  if ((status === "fail" || status === "blocked") && !hasText(result.reason)) {
    return buildCheck(`result:${caseId}`, "invalid", {
      detail: "nonPassRequiresReason",
      caseId,
    });
  }

  if (containsUnsafeEvidence(result)) {
    return buildCheck(`result:${caseId}`, "invalid", {
      detail: "evidenceContainsSensitiveOrLocalPathData",
      caseId,
    });
  }

  return buildCheck(`result:${caseId}`, status === "pass" ? "pass" : "invalid", {
    detail: status === "pass" ? "casePassed" : "caseNotPassed",
    caseId,
  });
}

function summarizeReportResults(report) {
  const results = Array.isArray(report?.results) ? report.results : [];
  const summary = {
    total: results.length,
    passed: 0,
    failed: 0,
    blocked: 0,
    skipped: 0,
  };

  for (const result of results) {
    switch (result?.result) {
      case "pass":
        summary.passed += 1;
        break;
      case "fail":
        summary.failed += 1;
        break;
      case "blocked":
        summary.blocked += 1;
        break;
      case "skipped":
        summary.skipped += 1;
        break;
      default:
        break;
    }
  }

  return summary;
}

function hasEvidence(result) {
  return Array.isArray(result?.evidence) && result.evidence.some((entry) => hasText(entry));
}

function containsUnsafeEvidence(result) {
  return containsUnsafeReportText([result?.evidence ?? [], result?.reason ?? ""]);
}

function containsUnsafeReportText(values) {
  const serialized = JSON.stringify(values);
  return (
    /[A-Z]:\\/i.test(serialized) ||
    /\\\\[^\\]+\\/i.test(serialized) ||
    /-----BEGIN/i.test(serialized) ||
    /PRIVATE KEY/i.test(serialized) ||
    /안녕|こんにちは|konnichi/i.test(serialized)
  );
}

function hasStrictIsoUtcTimestamp(value) {
  if (typeof value !== "string") {
    return false;
  }

  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/.test(value)) {
    return false;
  }

  const timestamp = new Date(value);
  return !Number.isNaN(timestamp.getTime()) && timestamp.toISOString() === value;
}

function buildCheck(id, status, extra = {}) {
  return {
    id,
    status,
    ...extra,
  };
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpQaReportArgs(process.argv.slice(2));
  if (options.template) {
    const template = buildWindowsMvpQaReportTemplate({ caseIds: options.caseIds });
    writeWindowsMvpQaReportOutputFile(options.outFile, template);
    process.stdout.write(`${JSON.stringify(template, null, 2)}\n`);
    process.exit(0);
  }

  if (options.updateResult) {
    const report = updateWindowsMvpQaReportResult(readWindowsMvpQaReport(options.reportFile), {
      caseId: options.caseIds[0],
      result: options.result,
      reason: options.reason,
      evidence: options.evidence,
      executedAt: options.executedAt,
      sourceOs: options.sourceOs,
      targetOs: options.targetOs,
      hardware: options.hardware,
      environmentNotes: options.environmentNotes,
    });
    writeWindowsMvpQaReportOutputFile(options.outFile, report);
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    process.exit(0);
  }

  const evaluation = evaluateWindowsMvpQaReport(readWindowsMvpQaReport(options.reportFile));
  writeWindowsMvpQaReportOutputFile(options.outFile, evaluation);
  process.stdout.write(`${JSON.stringify(evaluation, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpQaReport(evaluation));
}
