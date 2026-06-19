import { readFileSync } from "node:fs";

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

export function readWindowsMvpQaReport(reportFile) {
  return JSON.parse(readFileSync(reportFile, "utf8"));
}

export function parseWindowsMvpQaReportArgs(args) {
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--report-file") {
      return { reportFile: readValue(args, ++index, arg) };
    }

    throw new Error(`unknown argument: ${arg}`);
  }

  throw new Error("--report-file is required");
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
  const serialized = JSON.stringify({
    evidence: result?.evidence ?? [],
    reason: result?.reason ?? "",
  });

  return (
    /[A-Z]:\\/i.test(serialized) ||
    /\\\\[^\\]+\\/i.test(serialized) ||
    /-----BEGIN/i.test(serialized) ||
    /PRIVATE KEY/i.test(serialized) ||
    /안녕|こんにちは|konnichi/i.test(serialized)
  );
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
  const evaluation = evaluateWindowsMvpQaReport(readWindowsMvpQaReport(options.reportFile));
  process.stdout.write(`${JSON.stringify(evaluation, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpQaReport(evaluation));
}
