import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  LINUX_X11_QA_PLAN_SCHEMA_VERSION,
  buildLinuxX11QaPlan,
  listLinuxX11QaCaseIds,
} from "../scripts/linux-x11-qa-plan.mjs";
import {
  LINUX_X11_QA_REPORT_SCHEMA_VERSION,
  buildLinuxX11QaReportTemplate,
  evaluateLinuxX11QaReport,
  exitCodeForLinuxX11QaReport,
  parseLinuxX11QaReportArgs,
  updateLinuxX11QaReportResult,
  writeLinuxX11QaReportOutputFile,
} from "../scripts/linux-x11-qa-report.mjs";

function passingReport() {
  const plan = buildLinuxX11QaPlan();

  return {
    schemaVersion: LINUX_X11_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
    executedAt: "2026-06-23T00:00:00.000Z",
    environment: {
      sourceOs: "Windows 11 and Linux desktop",
      targetOs: "Linux X11 and Windows 11",
      sourceSession: "Windows desktop or X11",
      targetSession: "X11 or Windows desktop",
      hardware: "two physical endpoints on trusted local network",
    },
    results: plan.cases.map((testCase) => ({
      caseId: testCase.id,
      result: "pass",
      evidence: testCase.evidenceRequirements.map((evidence) => `${evidence.id}: artifact id`),
    })),
    privacy: {
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

function qaCaseCount() {
  return listLinuxX11QaCaseIds().length;
}

function replaceResult(report: ReturnType<typeof passingReport>, caseId: string, result: object) {
  const index = report.results.findIndex((candidate) => candidate.caseId === caseId);
  if (index === -1) {
    throw new Error(`missing QA result fixture for ${caseId}`);
  }
  report.results[index] = result as (typeof report.results)[number];
}

function runAppPackageScript(scriptName: string, args: string[]) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Linux X11 QA report evaluation", () => {
  test("accepts a complete sanitized release-blocking pass report", () => {
    const evaluation = evaluateLinuxX11QaReport(passingReport());

    expect(evaluation.ready).toBe(true);
    expect(evaluation.summary).toEqual({
      total: qaCaseCount(),
      passed: qaCaseCount(),
      failed: 0,
      blocked: 0,
      skipped: 0,
    });
    expect(evaluation.nextActions).toEqual([]);
    expect(evaluation.checks.every((check) => check.status === "pass")).toBe(true);
    expect(evaluation.privacy).toEqual({
      includesReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    });
    expect(exitCodeForLinuxX11QaReport(evaluation)).toBe(0);
  });

  test("rejects missing release-blocking cases and duplicate case ids", () => {
    const report = passingReport();
    report.results = [
      report.results[0],
      {
        ...report.results[0],
      },
    ];
    const evaluation = evaluateLinuxX11QaReport(report);

    expect(evaluation.ready).toBe(false);
    expect(evaluation.checks.find((check) => check.id === "duplicateCaseIds")).toMatchObject({
      status: "invalid",
      caseIds: ["LX11-001"],
    });
    expect(evaluation.checks.find((check) => check.id === "releaseBlockingCoverage")).toMatchObject(
      {
        status: "missing",
        caseIds: listLinuxX11QaCaseIds().filter((caseId) => caseId !== "LX11-001"),
      },
    );
    expect(exitCodeForLinuxX11QaReport(evaluation)).toBe(1);
  });

  test("rejects pass results without every planned evidence item", () => {
    const report = passingReport();
    replaceResult(report, "LX11-001", {
      caseId: "LX11-001",
      result: "pass",
      evidence: ["LX11-001-E1: source topology artifact id"],
    });
    const evaluation = evaluateLinuxX11QaReport(report);

    expect(evaluation.ready).toBe(false);
    expect(evaluation.checks.find((check) => check.id === "result:LX11-001")).toMatchObject({
      status: "invalid",
      detail: "passRequiresPlannedEvidence",
      missingEvidence: ["LX11-001-E2", "LX11-001-E3", "LX11-001-E4"],
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "addPlannedEvidence",
      action: "add sanitized evidence for every planned QA evidence item",
      caseId: "LX11-001",
      missingEvidence: ["LX11-001-E2", "LX11-001-E3", "LX11-001-E4"],
    });
  });

  test("accepts object evidence entries with planned evidence ids", () => {
    const report = passingReport();
    replaceResult(report, "LX11-003", {
      caseId: "LX11-003",
      result: "pass",
      evidence: [
        { id: "LX11-003-E1", artifactId: "unsupported-capability-diagnostics" },
        { id: "LX11-003-E2", note: "clear unsupported state shown" },
        { id: "LX11-003-E3", note: "local control stayed available" },
      ],
    });
    const evaluation = evaluateLinuxX11QaReport(report);

    expect(evaluation.checks.find((check) => check.id === "result:LX11-003")).toMatchObject({
      status: "pass",
    });
    expect(evaluation.ready).toBe(true);
  });

  test("requires reason for blocked or failed cases and keeps the report not ready", () => {
    const blockedReport = passingReport();
    replaceResult(blockedReport, "LX11-006", {
      caseId: "LX11-006",
      result: "blocked",
      evidence: ["LX11-006-E1: linux deb artifact id"],
      reason: "fresh Linux X11 install bench unavailable in this run",
    });
    const missingReasonReport = passingReport();
    replaceResult(missingReasonReport, "LX11-003", {
      caseId: "LX11-003",
      result: "fail",
      evidence: ["LX11-003-E1: diagnostics artifact id"],
    });

    const blockedEvaluation = evaluateLinuxX11QaReport(blockedReport);
    const missingReasonEvaluation = evaluateLinuxX11QaReport(missingReasonReport);

    expect(blockedEvaluation.ready).toBe(false);
    expect(blockedEvaluation.summary.blocked).toBe(1);
    expect(blockedEvaluation.checks.find((check) => check.id === "result:LX11-006")).toMatchObject({
      status: "invalid",
      detail: "caseNotPassed",
    });
    expect(
      missingReasonEvaluation.checks.find((check) => check.id === "result:LX11-003"),
    ).toMatchObject({
      status: "invalid",
      detail: "nonPassRequiresReason",
    });
  });

  test("rejects privacy flags, typed content, secrets, and local paths", () => {
    const privateReport = passingReport();
    privateReport.privacy.includesFullFilePaths = true;
    replaceResult(privateReport, "LX11-004", {
      caseId: "LX11-004",
      result: "pass",
      evidence: ["LX11-004-E1: /home/cherr/qa/topology.json"],
    });
    const typedContentReport = passingReport();
    replaceResult(typedContentReport, "LX11-001", {
      caseId: "LX11-001",
      result: "pass",
      evidence: ["안녕"],
    });
    const secretReport = passingReport();
    replaceResult(secretReport, "LX11-002", {
      caseId: "LX11-002",
      result: "pass",
      evidence: ["-----BEGIN PRIVATE KEY-----"],
    });

    const privateEvaluation = evaluateLinuxX11QaReport(privateReport);
    const typedContentEvaluation = evaluateLinuxX11QaReport(typedContentReport);
    const secretEvaluation = evaluateLinuxX11QaReport(secretReport);

    expect(privateEvaluation.checks.find((check) => check.id === "privacy")).toMatchObject({
      status: "invalid",
      flags: ["includesFullFilePaths"],
    });
    expect(privateEvaluation.checks.find((check) => check.id === "result:LX11-004")).toMatchObject({
      status: "invalid",
      detail: "evidenceContainsSensitiveOrLocalPathData",
    });
    expect(
      typedContentEvaluation.checks.find((check) => check.id === "result:LX11-001"),
    ).toMatchObject({
      status: "invalid",
      detail: "evidenceContainsSensitiveOrLocalPathData",
    });
    expect(secretEvaluation.checks.find((check) => check.id === "result:LX11-002")).toMatchObject({
      status: "invalid",
      detail: "evidenceContainsSensitiveOrLocalPathData",
    });
  });

  test("builds a sanitized blocked report template for every QA case", () => {
    const template = buildLinuxX11QaReportTemplate();
    const evaluation = evaluateLinuxX11QaReport(template);

    expect(template).toMatchObject({
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
      privacy: {
        includesTypedContent: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
      },
    });
    expect(template.results.map((result) => result.caseId)).toEqual(listLinuxX11QaCaseIds());
    expect(template.results.every((result) => result.result === "blocked")).toBe(true);
    expect(template.results.every((result) => result.reason === "not run yet")).toBe(true);
    expect(evaluation.ready).toBe(false);
    expect(evaluation.summary).toEqual({
      total: qaCaseCount(),
      passed: 0,
      failed: 0,
      blocked: qaCaseCount(),
      skipped: 0,
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "setExecutionTimestamp",
      action: "set executedAt to a strict ISO UTC timestamp with milliseconds",
    });
  });

  test("requires strict execution timestamp and Linux session metadata", () => {
    const missingMetadataReport = passingReport();
    delete (missingMetadataReport.environment as Partial<typeof missingMetadataReport.environment>)
      .sourceSession;
    const invalidTimestampReport = passingReport();
    invalidTimestampReport.executedAt = "2026-06-23";
    const unsafeEnvironmentReport = passingReport();
    unsafeEnvironmentReport.environment.hardware = "/tmp/akraz-qa/report.json";

    const missingMetadataEvaluation = evaluateLinuxX11QaReport(missingMetadataReport);
    const invalidTimestampEvaluation = evaluateLinuxX11QaReport(invalidTimestampReport);
    const unsafeEnvironmentEvaluation = evaluateLinuxX11QaReport(unsafeEnvironmentReport);

    expect(
      missingMetadataEvaluation.checks.find((check) => check.id === "executionMetadata"),
    ).toMatchObject({
      status: "missing",
      detail: "environmentFieldsMissing",
      fields: ["sourceSession"],
    });
    expect(
      invalidTimestampEvaluation.checks.find((check) => check.id === "executionMetadata"),
    ).toMatchObject({
      status: "invalid",
      detail: "executedAtMustBeStrictIsoUtc",
    });
    expect(
      unsafeEnvironmentEvaluation.checks.find((check) => check.id === "executionMetadata"),
    ).toMatchObject({
      status: "invalid",
      detail: "environmentContainsSensitiveOrLocalPathData",
    });
  });

  test("updates one QA result without hand-editing the report JSON", () => {
    const template = buildLinuxX11QaReportTemplate();
    const updatedReport = updateLinuxX11QaReportResult(template, {
      caseId: "LX11-006",
      result: "pass",
      evidence: [
        { id: "LX11-006-E1", note: "deb install smoke artifact id" },
        { id: "LX11-006-E2", note: "daemon sidecar diagnostics artifact id" },
        { id: "LX11-006-E3", note: "uninstall daemon-not-running pass note" },
      ],
      executedAt: "2026-06-23T01:02:03.004Z",
      sourceOs: "Linux desktop",
      targetOs: "Linux desktop",
      sourceSession: "X11",
      targetSession: "X11",
      hardware: "fresh Linux X11 endpoint",
      environmentNotes: "deb install qa bench",
    });
    const updatedEvaluation = evaluateLinuxX11QaReport(updatedReport);

    expect(updatedReport.executedAt).toBe("2026-06-23T01:02:03.004Z");
    expect(updatedReport.environment).toEqual({
      sourceOs: "Linux desktop",
      targetOs: "Linux desktop",
      sourceSession: "X11",
      targetSession: "X11",
      hardware: "fresh Linux X11 endpoint",
      notes: "deb install qa bench",
    });
    expect(updatedReport.results.find((result) => result.caseId === "LX11-006")).toEqual({
      caseId: "LX11-006",
      result: "pass",
      evidence: [
        { id: "LX11-006-E1", note: "deb install smoke artifact id" },
        { id: "LX11-006-E2", note: "daemon sidecar diagnostics artifact id" },
        { id: "LX11-006-E3", note: "uninstall daemon-not-running pass note" },
      ],
    });
    expect(updatedEvaluation.checks.find((check) => check.id === "result:LX11-006")).toMatchObject({
      status: "pass",
    });
  });

  test("updates one QA result through the app package script", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-linux-x11-qa-report-cli-"));
    const templateFile = join(tempDirectory, "template.json");
    const updatedFile = join(tempDirectory, "updated.json");

    try {
      const template = buildLinuxX11QaReportTemplate();
      writeLinuxX11QaReportOutputFile(templateFile, template);

      const result = runAppPackageScript("qa:linux-x11-report-update", [
        "--report-file",
        templateFile,
        "--case-id",
        "LX11-003",
        "--result",
        "pass",
        "--evidence-id",
        "LX11-003-E1",
        "--evidence-note",
        "unsupported capability diagnostics artifact id",
        "--evidence-id",
        "LX11-003-E2",
        "--evidence-note",
        "clear unsupported state shown",
        "--evidence-id",
        "LX11-003-E3",
        "--evidence-note",
        "local control stayed available",
        "--executed-at",
        "2026-06-23T01:02:03.004Z",
        "--source-os",
        "Windows 11",
        "--target-os",
        "Linux desktop",
        "--source-session",
        "Windows desktop",
        "--target-session",
        "X11",
        "--hardware",
        "two physical endpoints",
        "--out-file",
        updatedFile,
      ]);

      expect(result.status).toBe(0);

      const updatedReport = JSON.parse(readFileSync(updatedFile, "utf8"));
      expect(updatedReport.results.find((candidate) => candidate.caseId === "LX11-003")).toEqual({
        caseId: "LX11-003",
        result: "pass",
        evidence: [
          { id: "LX11-003-E1", note: "unsupported capability diagnostics artifact id" },
          { id: "LX11-003-E2", note: "clear unsupported state shown" },
          { id: "LX11-003-E3", note: "local control stayed available" },
        ],
      });
      expect(JSON.parse(result.stdout)).toEqual(updatedReport);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("parses report, template, and update arguments", () => {
    expect(parseLinuxX11QaReportArgs(["--report-file", "qa-report.json"])).toEqual({
      template: false,
      updateResult: false,
      reportFile: "qa-report.json",
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
    });
    expect(
      parseLinuxX11QaReportArgs([
        "--update-result",
        "--report-file",
        "qa-report.json",
        "--case-id",
        "LX11-003",
        "--result",
        "blocked",
        "--reason",
        "XTEST disabled bench unavailable",
        "--source-session",
        "X11",
        "--target-session",
        "X11",
      ]),
    ).toMatchObject({
      updateResult: true,
      reportFile: "qa-report.json",
      caseIds: ["LX11-003"],
      result: "blocked",
      reason: "XTEST disabled bench unavailable",
      sourceSession: "X11",
      targetSession: "X11",
    });
    expect(() => parseLinuxX11QaReportArgs(["--case-id", "LX11-001"])).toThrow(
      "--case-id can only be used with --template",
    );
    expect(() => parseLinuxX11QaReportArgs(["--source-session", "X11"])).toThrow(
      "--source-session can only be used with --update-result",
    );
  });

  test("writes template and evaluates reports through the app package script", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-linux-x11-qa-eval-cli-"));
    const reportFile = join(tempDirectory, "qa-report.json");
    const templateFile = join(tempDirectory, "nested", "template.json");

    try {
      const templateResult = runAppPackageScript("qa:linux-x11-report-template", [
        "--case-id",
        "LX11-002",
        "--out-file",
        templateFile,
      ]);
      const template = JSON.parse(templateResult.stdout);

      expect(templateResult.status).toBe(0);
      expect(template.generatedFrom).toBe("qa:linux-x11-report-template");
      expect(template.results.map((result) => result.caseId)).toEqual(["LX11-002"]);
      expect(JSON.parse(readFileSync(templateFile, "utf8"))).toEqual(template);

      writeLinuxX11QaReportOutputFile(reportFile, passingReport());
      const evaluationResult = runAppPackageScript("qa:linux-x11-report", [
        "--report-file",
        reportFile,
      ]);
      const evaluation = JSON.parse(evaluationResult.stdout);

      expect(evaluationResult.status).toBe(0);
      expect(evaluation.ready).toBe(true);
      expect(evaluation.summary.total).toBe(qaCaseCount());
      expect(evaluation.privacy.includesReportPayload).toBe(false);
      expect(evaluation.privacy.includesSecretValues).toBe(false);
      expect(evaluation.privacy.includesFullFilePaths).toBe(false);
      expect(evaluationResult.stdout).not.toContain(reportFile);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });
});
