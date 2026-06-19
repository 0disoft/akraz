import { describe, expect, test } from "bun:test";

import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  listWindowsMvpQaCaseIds,
} from "../scripts/windows-mvp-qa-plan.mjs";
import {
  WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  evaluateWindowsMvpQaReport,
  exitCodeForWindowsMvpQaReport,
  parseWindowsMvpQaReportArgs,
} from "../scripts/windows-mvp-qa-report.mjs";

function passingReport() {
  return {
    schemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    executedAt: "2026-06-20T00:00:00.000Z",
    results: listWindowsMvpQaCaseIds().map((caseId) => ({
      caseId,
      result: "pass",
      evidence: [`${caseId} sanitized artifact id`],
    })),
    privacy: {
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

describe("Windows MVP QA report evaluation", () => {
  test("accepts a complete sanitized release-blocking pass report", () => {
    const evaluation = evaluateWindowsMvpQaReport(passingReport());

    expect(evaluation.ready).toBe(true);
    expect(evaluation.summary).toEqual({
      total: 5,
      passed: 5,
      failed: 0,
      blocked: 0,
      skipped: 0,
    });
    expect(evaluation.checks.every((check) => check.status === "pass")).toBe(true);
    expect(evaluation.privacy).toEqual({
      includesReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    });
    expect(exitCodeForWindowsMvpQaReport(evaluation)).toBe(0);
  });

  test("rejects missing release-blocking cases and duplicate case ids", () => {
    const report = passingReport();
    report.results = [
      report.results[0],
      {
        ...report.results[0],
      },
    ];
    const evaluation = evaluateWindowsMvpQaReport(report);

    expect(evaluation.ready).toBe(false);
    expect(evaluation.checks.find((check) => check.id === "duplicateCaseIds")).toMatchObject({
      status: "invalid",
      caseIds: ["WIN-006"],
    });
    expect(evaluation.checks.find((check) => check.id === "releaseBlockingCoverage")).toMatchObject(
      {
        status: "missing",
        caseIds: ["WIN-007", "I18N-001", "I18N-004", "REL-001"],
      },
    );
    expect(exitCodeForWindowsMvpQaReport(evaluation)).toBe(1);
  });

  test("rejects pass results without evidence and unknown case ids", () => {
    const report = passingReport();
    report.results = [
      { caseId: "WIN-006", result: "pass", evidence: [] },
      { caseId: "NOPE-999", result: "pass", evidence: ["artifact"] },
    ];
    const evaluation = evaluateWindowsMvpQaReport(report);

    expect(evaluation.ready).toBe(false);
    expect(evaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "invalid",
      detail: "passRequiresEvidence",
    });
    expect(evaluation.checks.find((check) => check.id === "result:NOPE-999")).toMatchObject({
      status: "invalid",
      detail: "unknownCaseId",
    });
  });

  test("requires reason for blocked or failed cases and keeps the report not ready", () => {
    const blockedReport = passingReport();
    blockedReport.results[1] = {
      caseId: "WIN-007",
      result: "blocked",
      evidence: ["screen topology artifact id"],
      reason: "mixed DPI monitor unavailable in this run",
    };
    const missingReasonReport = passingReport();
    missingReasonReport.results[2] = {
      caseId: "I18N-001",
      result: "fail",
      evidence: ["keyboard layout artifact id"],
    };

    const blockedEvaluation = evaluateWindowsMvpQaReport(blockedReport);
    const missingReasonEvaluation = evaluateWindowsMvpQaReport(missingReasonReport);

    expect(blockedEvaluation.ready).toBe(false);
    expect(blockedEvaluation.summary.blocked).toBe(1);
    expect(blockedEvaluation.checks.find((check) => check.id === "result:WIN-007")).toMatchObject({
      status: "invalid",
      detail: "caseNotPassed",
    });
    expect(
      missingReasonEvaluation.checks.find((check) => check.id === "result:I18N-001"),
    ).toMatchObject({
      status: "invalid",
      detail: "nonPassRequiresReason",
    });
  });

  test("rejects privacy flags and sensitive evidence payloads", () => {
    const privateReport = passingReport();
    privateReport.privacy.includesFullFilePaths = true;
    privateReport.results[0].evidence = ["C:\\Users\\cherr\\Desktop\\qa.json"];
    const typedContentReport = passingReport();
    typedContentReport.results[2].evidence = ["안녕"];

    const privateEvaluation = evaluateWindowsMvpQaReport(privateReport);
    const typedContentEvaluation = evaluateWindowsMvpQaReport(typedContentReport);

    expect(privateEvaluation.checks.find((check) => check.id === "privacy")).toMatchObject({
      status: "invalid",
      flags: ["includesFullFilePaths"],
    });
    expect(privateEvaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "invalid",
      detail: "evidenceContainsSensitiveOrLocalPathData",
    });
    expect(
      typedContentEvaluation.checks.find((check) => check.id === "result:I18N-001"),
    ).toMatchObject({
      status: "invalid",
      detail: "evidenceContainsSensitiveOrLocalPathData",
    });
  });

  test("parses report file argument", () => {
    expect(parseWindowsMvpQaReportArgs(["--report-file", "qa-report.json"])).toEqual({
      reportFile: "qa-report.json",
    });
    expect(() => parseWindowsMvpQaReportArgs([])).toThrow("--report-file is required");
  });
});
