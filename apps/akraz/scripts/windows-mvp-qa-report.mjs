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
    reportFile: undefined,
    outFile: undefined,
    caseIds: [],
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--template":
        options.template = true;
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
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }

  if (options.template && options.reportFile !== undefined) {
    throw new Error("--template cannot be combined with --report-file");
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

  const evaluation = evaluateWindowsMvpQaReport(readWindowsMvpQaReport(options.reportFile));
  writeWindowsMvpQaReportOutputFile(options.outFile, evaluation);
  process.stdout.write(`${JSON.stringify(evaluation, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpQaReport(evaluation));
}
