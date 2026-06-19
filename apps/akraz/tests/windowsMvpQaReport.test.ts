import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  listWindowsMvpQaCaseIds,
} from "../scripts/windows-mvp-qa-plan.mjs";
import {
  WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  buildWindowsMvpQaReportTemplate,
  evaluateWindowsMvpQaReport,
  exitCodeForWindowsMvpQaReport,
  parseWindowsMvpQaReportArgs,
  writeWindowsMvpQaReportOutputFile,
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
      template: false,
      reportFile: "qa-report.json",
      outFile: undefined,
      caseIds: [],
    });
    expect(() => parseWindowsMvpQaReportArgs([])).toThrow("--report-file is required");
  });

  test("builds a sanitized blocked report template for every QA case", () => {
    const template = buildWindowsMvpQaReportTemplate();
    const evaluation = evaluateWindowsMvpQaReport(template);

    expect(template).toMatchObject({
      schemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
      planSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
      generatedFrom: "qa:windows-mvp-report-template",
      executedAt: null,
      privacy: {
        includesTypedContent: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
      },
    });
    expect(template.results.map((result) => result.caseId)).toEqual(listWindowsMvpQaCaseIds());
    expect(template.results.every((result) => result.result === "blocked")).toBe(true);
    expect(template.results.every((result) => result.reason === "not run yet")).toBe(true);
    expect(template.results.every((result) => result.evidence.length === 0)).toBe(true);
    expect(evaluation.ready).toBe(false);
    expect(evaluation.summary).toEqual({
      total: 5,
      passed: 0,
      failed: 0,
      blocked: 5,
      skipped: 0,
    });
  });

  test("builds and parses a filtered report template", () => {
    const template = buildWindowsMvpQaReportTemplate({ caseIds: ["WIN-007"] });

    expect(template.results.map((result) => result.caseId)).toEqual(["WIN-007"]);
    expect(parseWindowsMvpQaReportArgs(["--template", "--case-id", "WIN-007"])).toEqual({
      template: true,
      reportFile: undefined,
      outFile: undefined,
      caseIds: ["WIN-007"],
    });
    expect(() => buildWindowsMvpQaReportTemplate({ caseIds: ["NOPE-999"] })).toThrow(
      "unknown Windows MVP QA case id(s): NOPE-999",
    );
  });

  test("rejects conflicting template and report evaluation arguments", () => {
    expect(() =>
      parseWindowsMvpQaReportArgs(["--template", "--report-file", "qa-report.json"]),
    ).toThrow("--template cannot be combined with --report-file");
    expect(() => parseWindowsMvpQaReportArgs(["--case-id", "WIN-007"])).toThrow(
      "--case-id can only be used with --template",
    );
  });

  test("parses and writes template or evaluation JSON to an output file", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-qa-report-"));
    const templateFile = join(tempDirectory, "nested", "template.json");
    const evaluationFile = join(tempDirectory, "evaluation.json");

    try {
      expect(
        parseWindowsMvpQaReportArgs([
          "--template",
          "--case-id",
          "WIN-007",
          "--out-file",
          templateFile,
        ]),
      ).toEqual({
        template: true,
        reportFile: undefined,
        outFile: templateFile,
        caseIds: ["WIN-007"],
      });

      const template = buildWindowsMvpQaReportTemplate({ caseIds: ["WIN-007"] });
      expect(writeWindowsMvpQaReportOutputFile(templateFile, template)).toBe(templateFile);
      expect(JSON.parse(readFileSync(templateFile, "utf8"))).toEqual(template);
      expect(readFileSync(templateFile, "utf8").endsWith("\n")).toBe(true);

      const evaluation = evaluateWindowsMvpQaReport(passingReport());
      expect(writeWindowsMvpQaReportOutputFile(evaluationFile, evaluation)).toBe(evaluationFile);
      expect(JSON.parse(readFileSync(evaluationFile, "utf8"))).toEqual(evaluation);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });
});
