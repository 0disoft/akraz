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

import { LINUX_X11_QA_PLAN_SCHEMA_VERSION, buildLinuxX11QaPlan } from "./linux-x11-qa-plan.mjs";

export const LINUX_X11_QA_REPORT_SCHEMA_VERSION = "akraz.linuxX11QaReport/v1";

const RESULT_STATUSES = new Set(["pass", "fail", "blocked", "skipped"]);
const PRIVACY_FLAGS = ["includesTypedContent", "includesSecretValues", "includesFullFilePaths"];
const REQUIRED_ENVIRONMENT_FIELDS = [
  "sourceOs",
  "targetOs",
  "sourceSession",
  "targetSession",
  "hardware",
];

export function evaluateLinuxX11QaReport(report, plan = buildLinuxX11QaPlan()) {
  const checks = [
    evaluateReportSchema(report),
    evaluateReportPlanSchema(report),
    evaluateReportExecutionMetadata(report),
    evaluateReportPrivacy(report),
    ...evaluateReportResults(report, plan),
  ];
  const summary = summarizeReportResults(report);

  return {
    schemaVersion: "akraz.linuxX11QaReportEvaluation/v1",
    reportSchemaVersion: LINUX_X11_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass") && summary.failed === 0,
    summary,
    nextActions: buildLinuxX11QaNextActions(checks),
    checks,
    privacy: {
      includesReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function buildLinuxX11QaReportTemplate(options = {}) {
  const plan = buildLinuxX11QaPlan({ caseIds: options.caseIds ?? [] });

  return {
    schemaVersion: LINUX_X11_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
    generatedFrom: "qa:linux-x11-report-template",
    executedAt: null,
    environment: {
      sourceOs: null,
      targetOs: null,
      sourceSession: null,
      targetSession: null,
      hardware: null,
    },
    results: plan.cases.map((testCase) => ({
      caseId: testCase.id,
      result: "blocked",
      reason: "not run yet",
      evidence: [],
      requiredEvidence: testCase.evidenceRequirements,
    })),
    privacy: {
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function updateLinuxX11QaReportResult(report, options, plan = buildLinuxX11QaPlan()) {
  const caseId = hasText(options.caseId) ? options.caseId.trim() : "";
  const planCaseIds = new Set(plan.cases.map((testCase) => testCase.id));

  if (!planCaseIds.has(caseId)) {
    throw new Error(
      `unknown Linux X11 QA case id: ${caseId}; available case ids: ${[...planCaseIds].join(", ")}`,
    );
  }

  if (!RESULT_STATUSES.has(options.result)) {
    throw new Error(`invalid Linux X11 QA result status: ${options.result}`);
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
      options.sourceSession ?? "",
      options.targetSession ?? "",
      options.hardware ?? "",
      options.environmentNotes ?? "",
    ])
  ) {
    throw new Error("updated Linux X11 QA report values contain sensitive or local path data");
  }

  const planCase = plan.cases.find((testCase) => testCase.id === caseId);
  validateStructuredEvidenceIds(options.evidence, planCase, caseId);

  if (options.result === "pass") {
    const missingEvidence = findMissingPlannedEvidence({ evidence: options.evidence }, planCase);
    if (missingEvidence.length > 0) {
      throw new Error(
        `--result pass requires planned evidence for ${caseId}: ${missingEvidence.join(", ")}`,
      );
    }
  }

  const nextReport = JSON.parse(JSON.stringify(report));
  nextReport.executedAt = options.executedAt ?? nextReport.executedAt ?? null;
  nextReport.environment ??= {};
  nextReport.environment = {
    ...nextReport.environment,
    ...(options.sourceOs !== undefined ? { sourceOs: options.sourceOs } : {}),
    ...(options.targetOs !== undefined ? { targetOs: options.targetOs } : {}),
    ...(options.sourceSession !== undefined ? { sourceSession: options.sourceSession } : {}),
    ...(options.targetSession !== undefined ? { targetSession: options.targetSession } : {}),
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

export function readLinuxX11QaReport(reportFile) {
  return JSON.parse(readFileSync(reportFile, "utf8"));
}

export function writeLinuxX11QaReportOutputFile(outFile, payload) {
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

export function parseLinuxX11QaReportArgs(args) {
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
    sourceSession: undefined,
    targetSession: undefined,
    hardware: undefined,
    environmentNotes: undefined,
  };
  let firstEvidenceFlag;

  let index = 0;
  while (index < args.length) {
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
        firstEvidenceFlag ??= arg;
        options.evidence.push(readValue(args, ++index, arg));
        break;
      case "--evidence-id": {
        firstEvidenceFlag ??= arg;
        const id = readValue(args, ++index, arg);
        if (args[index + 1] !== "--evidence-note") {
          throw new Error("--evidence-id requires a following --evidence-note");
        }
        const note = readValue(args, index + 2, "--evidence-note");
        options.evidence.push({ id, note });
        index += 2;
        break;
      }
      case "--evidence-note":
        throw new Error("--evidence-note must follow --evidence-id");
      case "--executed-at":
        options.executedAt = readValue(args, ++index, arg);
        break;
      case "--source-os":
        options.sourceOs = readValue(args, ++index, arg);
        break;
      case "--target-os":
        options.targetOs = readValue(args, ++index, arg);
        break;
      case "--source-session":
        options.sourceSession = readValue(args, ++index, arg);
        break;
      case "--target-session":
        options.targetSession = readValue(args, ++index, arg);
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
    index += 1;
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
    [firstEvidenceFlag ?? "--evidence", options.evidence.length > 0 ? options.evidence : undefined],
    ["--executed-at", options.executedAt],
    ["--source-os", options.sourceOs],
    ["--target-os", options.targetOs],
    ["--source-session", options.sourceSession],
    ["--target-session", options.targetSession],
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

export function exitCodeForLinuxX11QaReport(evaluation) {
  return evaluation.ready ? 0 : 1;
}

function evaluateReportSchema(report) {
  if (report?.schemaVersion !== LINUX_X11_QA_REPORT_SCHEMA_VERSION) {
    return buildCheck("reportSchema", "invalid", {
      detail: "schemaVersionMismatch",
      expectedSchemaVersion: LINUX_X11_QA_REPORT_SCHEMA_VERSION,
    });
  }

  return buildCheck("reportSchema", "pass");
}

function evaluateReportPlanSchema(report) {
  if (report?.planSchemaVersion === undefined) {
    return buildCheck("planSchema", "missing", {
      detail: "planSchemaVersionMissing",
      expectedPlanSchemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
    });
  }

  if (report.planSchemaVersion !== LINUX_X11_QA_PLAN_SCHEMA_VERSION) {
    return buildCheck("planSchema", "invalid", {
      detail: "planSchemaVersionMismatch",
      expectedPlanSchemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
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
      fields: REQUIRED_ENVIRONMENT_FIELDS,
    });
  }

  const missingFields = REQUIRED_ENVIRONMENT_FIELDS.filter((field) => !hasText(environment[field]));
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
      environment.sourceSession,
      environment.targetSession,
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
    checks.push(evaluateSingleResult(caseId, result, planCases.get(caseId)));
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

function evaluateSingleResult(caseId, result, testCase) {
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

  const missingEvidence = status === "pass" ? findMissingPlannedEvidence(result, testCase) : [];
  if (missingEvidence.length > 0) {
    return buildCheck(`result:${caseId}`, "invalid", {
      detail: "passRequiresPlannedEvidence",
      caseId,
      missingEvidence,
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

function buildLinuxX11QaNextActions(checks) {
  const actions = [];
  const pushedActionKeys = new Set();

  for (const check of checks) {
    const action = buildLinuxX11QaNextAction(check);
    if (!action) {
      continue;
    }

    const actionKey = `${action.id}:${action.caseId ?? ""}:${action.action}`;
    if (pushedActionKeys.has(actionKey)) {
      continue;
    }

    actions.push(action);
    pushedActionKeys.add(actionKey);
  }

  return actions;
}

function buildLinuxX11QaNextAction(check) {
  if (!check || check.status === "pass") {
    return undefined;
  }

  switch (check.detail) {
    case "executedAtMustBeStrictIsoUtc":
      return {
        id: "setExecutionTimestamp",
        action: "set executedAt to a strict ISO UTC timestamp with milliseconds",
      };
    case "environmentMissing":
    case "environmentFieldsMissing":
      return {
        id: "setEnvironment",
        action:
          "record sourceOs, targetOs, sourceSession, targetSession, and hardware without local paths or secrets",
        fields: check.fields ?? REQUIRED_ENVIRONMENT_FIELDS,
      };
    case "environmentContainsSensitiveOrLocalPathData":
      return {
        id: "sanitizeEnvironment",
        action: "replace environment values that contain typed content, secrets, or local paths",
      };
    case "privacyFlagsMustBeFalse":
      return {
        id: "sanitizePrivacy",
        action: "set every privacy flag to false after removing private report data",
        flags: check.flags ?? [],
      };
    case "resultsMissing":
      return {
        id: "addResults",
        action: "add QA results for every release-blocking Linux X11 case",
      };
    case "caseIdMissing":
      return {
        id: "fixResultCaseId",
        action: "set a valid caseId on every QA result entry",
      };
    case "unknownCaseId":
      return {
        id: "removeUnknownCase",
        action: "remove or replace the unknown QA case id",
        caseId: check.caseId,
      };
    case "duplicateCaseIds":
      return {
        id: "dedupeResults",
        action: "keep exactly one result per QA case id",
        caseIds: check.caseIds ?? [],
      };
    case "releaseBlockingCasesMissing":
      return {
        id: "addReleaseBlockingResults",
        action: "add results for every missing release-blocking QA case",
        caseIds: check.caseIds ?? [],
      };
    case "unsupportedResultStatus":
      return {
        id: "fixResultStatus",
        action: "set result to pass, fail, blocked, or skipped",
        caseId: check.caseId,
      };
    case "passRequiresEvidence":
      return {
        id: "addPassEvidence",
        action: "add sanitized evidence before marking the QA case as pass",
        caseId: check.caseId,
      };
    case "passRequiresPlannedEvidence":
      return {
        id: "addPlannedEvidence",
        action: "add sanitized evidence for every planned QA evidence item",
        caseId: check.caseId,
        missingEvidence: check.missingEvidence ?? [],
      };
    case "nonPassRequiresReason":
      return {
        id: "addNonPassReason",
        action: "add a reason for each failed or blocked QA case",
        caseId: check.caseId,
      };
    case "evidenceContainsSensitiveOrLocalPathData":
      return {
        id: "sanitizeEvidence",
        action:
          "replace evidence or reason values that contain typed content, secrets, or local paths",
        caseId: check.caseId,
      };
    case "caseNotPassed":
      return {
        id: "rerunOrResolveCase",
        action: "rerun the Linux X11 QA case or resolve the blocker before release",
        caseId: check.caseId,
      };
    default:
      return {
        id: "reviewQaReportCheck",
        action: "review the failing QA report check",
        checkId: check.id,
      };
  }
}

function hasEvidence(result) {
  return Array.isArray(result?.evidence) && result.evidence.some(hasEvidenceEntry);
}

function hasEvidenceEntry(entry) {
  if (typeof entry === "string") {
    return hasText(entry);
  }

  return (
    entry &&
    typeof entry === "object" &&
    !Array.isArray(entry) &&
    hasText(entry.id) &&
    hasStructuredEvidenceDetail(entry)
  );
}

function findMissingPlannedEvidence(result, testCase) {
  const plannedEvidence = Array.isArray(testCase?.evidenceRequirements)
    ? testCase.evidenceRequirements
    : [];
  if (plannedEvidence.length === 0) {
    return [];
  }

  const evidence = Array.isArray(result?.evidence) ? result.evidence : [];

  return plannedEvidence
    .filter((requiredEvidence) => {
      return !evidence.some((entry) => evidenceEntryMatchesRequirement(entry, requiredEvidence));
    })
    .map((requiredEvidence) => requiredEvidence.id);
}

function evidenceEntryMatchesRequirement(entry, requiredEvidence) {
  if (!requiredEvidence?.id) {
    return false;
  }

  if (entry && typeof entry === "object" && !Array.isArray(entry)) {
    return entry.id === requiredEvidence.id && hasStructuredEvidenceDetail(entry);
  }

  if (typeof entry !== "string") {
    return false;
  }

  const normalizedEntry = normalizeEvidenceText(entry);
  const normalizedId = normalizeEvidenceText(requiredEvidence.id);
  return (
    normalizedEntry === normalizedId ||
    normalizedEntry.startsWith(`${normalizedId}:`) ||
    normalizedEntry.startsWith(`${normalizedId} `)
  );
}

function normalizeEvidenceText(value) {
  return String(value ?? "")
    .toLowerCase()
    .replace(/\s+/g, " ")
    .trim();
}

function validateStructuredEvidenceIds(evidence, testCase, caseId) {
  const plannedEvidenceIds = new Set(
    Array.isArray(testCase?.evidenceRequirements)
      ? testCase.evidenceRequirements.map((requiredEvidence) => requiredEvidence.id)
      : [],
  );
  const unknownEvidenceIds = (Array.isArray(evidence) ? evidence : [])
    .filter((entry) => entry && typeof entry === "object" && !Array.isArray(entry))
    .map((entry) => entry.id)
    .filter((id) => hasText(id) && !plannedEvidenceIds.has(id));

  if (unknownEvidenceIds.length > 0) {
    throw new Error(
      `unknown planned evidence id for ${caseId}: ${[...new Set(unknownEvidenceIds)].join(", ")}`,
    );
  }
}

function hasStructuredEvidenceDetail(entry) {
  return Object.entries(entry).some(([key, value]) => key !== "id" && hasText(value));
}

function containsUnsafeEvidence(result) {
  return containsUnsafeReportText([result?.evidence ?? [], result?.reason ?? ""]);
}

function containsUnsafeReportText(values) {
  const serialized = JSON.stringify(values);
  return (
    /[A-Z]:\\/i.test(serialized) ||
    /\\\\[^\\]+\\/i.test(serialized) ||
    /(?:^|["'\s])\/(?:home|Users|tmp|var|mnt|media)\/[^"'\s]+/i.test(serialized) ||
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
  const options = parseLinuxX11QaReportArgs(process.argv.slice(2));
  if (options.template) {
    const template = buildLinuxX11QaReportTemplate({ caseIds: options.caseIds });
    writeLinuxX11QaReportOutputFile(options.outFile, template);
    process.stdout.write(`${JSON.stringify(template, null, 2)}\n`);
    process.exit(0);
  }

  if (options.updateResult) {
    const report = updateLinuxX11QaReportResult(readLinuxX11QaReport(options.reportFile), {
      caseId: options.caseIds[0],
      result: options.result,
      reason: options.reason,
      evidence: options.evidence,
      executedAt: options.executedAt,
      sourceOs: options.sourceOs,
      targetOs: options.targetOs,
      sourceSession: options.sourceSession,
      targetSession: options.targetSession,
      hardware: options.hardware,
      environmentNotes: options.environmentNotes,
    });
    writeLinuxX11QaReportOutputFile(options.outFile, report);
    process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    process.exit(0);
  }

  const evaluation = evaluateLinuxX11QaReport(readLinuxX11QaReport(options.reportFile));
  writeLinuxX11QaReportOutputFile(options.outFile, evaluation);
  process.stdout.write(`${JSON.stringify(evaluation, null, 2)}\n`);
  process.exit(exitCodeForLinuxX11QaReport(evaluation));
}
