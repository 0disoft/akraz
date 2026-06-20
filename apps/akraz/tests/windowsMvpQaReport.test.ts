import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  buildWindowsMvpQaPlan,
  listWindowsMvpQaCaseIds,
} from "../scripts/windows-mvp-qa-plan.mjs";
import {
  WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  buildWindowsMvpQaReportTemplate,
  evaluateWindowsMvpQaReport,
  exitCodeForWindowsMvpQaReport,
  parseWindowsMvpQaReportArgs,
  updateWindowsMvpQaReportResult,
  writeWindowsMvpQaReportOutputFile,
} from "../scripts/windows-mvp-qa-report.mjs";
import {
  WINDOWS_MVP_QA_WORKFLOW_PAYLOAD_SCHEMA_VERSION,
  buildWindowsMvpQaWorkflowPayload,
  buildWindowsMvpQaWorkflowPayloadReport,
  exitCodeForWindowsMvpQaWorkflowPayload,
  parseWindowsMvpQaWorkflowPayloadArgs,
  writeWindowsMvpQaWorkflowDispatchInputsFile,
  writeWindowsMvpQaWorkflowPayloadOutputFile,
} from "../scripts/windows-mvp-qa-workflow-payload.mjs";

function passingReport() {
  const plan = buildWindowsMvpQaPlan();

  return {
    schemaVersion: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
    planSchemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    executedAt: "2026-06-20T00:00:00.000Z",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "two physical Windows endpoints",
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
  return listWindowsMvpQaCaseIds().length;
}

function replaceResult(report, caseId, result) {
  const index = report.results.findIndex((candidate) => candidate.caseId === caseId);
  if (index === -1) {
    throw new Error(`missing QA result fixture for ${caseId}`);
  }
  report.results[index] = result;
}

function runAppPackageScript(scriptName, args) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Windows MVP QA report evaluation", () => {
  test("accepts a complete sanitized release-blocking pass report", () => {
    const evaluation = evaluateWindowsMvpQaReport(passingReport());

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
      caseIds: ["WIN-002"],
    });
    expect(evaluation.checks.find((check) => check.id === "releaseBlockingCoverage")).toMatchObject(
      {
        status: "missing",
        caseIds: listWindowsMvpQaCaseIds().filter((caseId) => caseId !== "WIN-002"),
      },
    );
    expect(evaluation.nextActions).toEqual([
      {
        id: "dedupeResults",
        action: "keep exactly one result per QA case id",
        caseIds: ["WIN-002"],
      },
      {
        id: "addReleaseBlockingResults",
        action: "add results for every missing release-blocking QA case",
        caseIds: listWindowsMvpQaCaseIds().filter((caseId) => caseId !== "WIN-002"),
      },
    ]);
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
    expect(evaluation.nextActions).toContainEqual({
      id: "addPassEvidence",
      action: "add sanitized evidence before marking the QA case as pass",
      caseId: "WIN-006",
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "removeUnknownCase",
      action: "remove or replace the unknown QA case id",
      caseId: "NOPE-999",
    });
  });

  test("rejects pass results that omit planned evidence items", () => {
    const report = passingReport();
    replaceResult(report, "WIN-006", {
      caseId: "WIN-006",
      result: "pass",
      evidence: ["WIN-006-E1: diagnostics support bundle artifact id"],
    });
    const evaluation = evaluateWindowsMvpQaReport(report);

    expect(evaluation.ready).toBe(false);
    expect(evaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "invalid",
      detail: "passRequiresPlannedEvidence",
      missingEvidence: ["WIN-006-E2"],
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "addPlannedEvidence",
      action: "add sanitized evidence for every planned QA evidence item",
      caseId: "WIN-006",
      missingEvidence: ["WIN-006-E2"],
    });
  });

  test("accepts object evidence entries with planned evidence ids", () => {
    const report = passingReport();
    replaceResult(report, "WIN-006", {
      caseId: "WIN-006",
      result: "pass",
      evidence: [
        { id: "WIN-006-E1", artifactId: "support-bundle-artifact" },
        { id: "WIN-006-E2", artifactId: "soak-report-artifact" },
      ],
    });
    const evaluation = evaluateWindowsMvpQaReport(report);

    expect(evaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "pass",
    });
    expect(evaluation.ready).toBe(true);
  });

  test("rejects object evidence entries without artifact detail", () => {
    const report = passingReport();
    replaceResult(report, "WIN-006", {
      caseId: "WIN-006",
      result: "pass",
      evidence: [{ id: "WIN-006-E1" }, { id: "WIN-006-E2", note: "soak report artifact id" }],
    });
    const evaluation = evaluateWindowsMvpQaReport(report);

    expect(evaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "invalid",
      detail: "passRequiresPlannedEvidence",
      missingEvidence: ["WIN-006-E1"],
    });
  });

  test("requires reason for blocked or failed cases and keeps the report not ready", () => {
    const blockedReport = passingReport();
    replaceResult(blockedReport, "WIN-007", {
      caseId: "WIN-007",
      result: "blocked",
      evidence: ["screen topology artifact id"],
      reason: "mixed DPI monitor unavailable in this run",
    });
    const missingReasonReport = passingReport();
    replaceResult(missingReasonReport, "I18N-001", {
      caseId: "I18N-001",
      result: "fail",
      evidence: ["keyboard layout artifact id"],
    });

    const blockedEvaluation = evaluateWindowsMvpQaReport(blockedReport);
    const missingReasonEvaluation = evaluateWindowsMvpQaReport(missingReasonReport);

    expect(blockedEvaluation.ready).toBe(false);
    expect(blockedEvaluation.summary.blocked).toBe(1);
    expect(blockedEvaluation.checks.find((check) => check.id === "result:WIN-007")).toMatchObject({
      status: "invalid",
      detail: "caseNotPassed",
    });
    expect(blockedEvaluation.nextActions).toEqual([
      {
        id: "rerunOrResolveCase",
        action: "rerun the QA case or resolve the blocker before release",
        caseId: "WIN-007",
      },
    ]);
    expect(
      missingReasonEvaluation.checks.find((check) => check.id === "result:I18N-001"),
    ).toMatchObject({
      status: "invalid",
      detail: "nonPassRequiresReason",
    });
    expect(missingReasonEvaluation.nextActions).toEqual([
      {
        id: "addNonPassReason",
        action: "add a reason for each failed or blocked QA case",
        caseId: "I18N-001",
      },
    ]);
  });

  test("rejects privacy flags and sensitive evidence payloads", () => {
    const privateReport = passingReport();
    privateReport.privacy.includesFullFilePaths = true;
    replaceResult(privateReport, "WIN-006", {
      caseId: "WIN-006",
      result: "pass",
      evidence: ["C:\\Users\\cherr\\Desktop\\qa.json"],
    });
    const typedContentReport = passingReport();
    replaceResult(typedContentReport, "I18N-001", {
      caseId: "I18N-001",
      result: "pass",
      evidence: ["안녕"],
    });

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
    expect(privateEvaluation.nextActions).toContainEqual({
      id: "sanitizePrivacy",
      action: "set every privacy flag to false after removing private report data",
      flags: ["includesFullFilePaths"],
    });
    expect(privateEvaluation.nextActions).toContainEqual({
      id: "sanitizeEvidence",
      action:
        "replace evidence or reason values that contain typed content, secrets, or local paths",
      caseId: "WIN-006",
    });
  });

  test("parses report file argument", () => {
    expect(parseWindowsMvpQaReportArgs(["--report-file", "qa-report.json"])).toEqual({
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
      hardware: undefined,
      environmentNotes: undefined,
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
      environment: {
        sourceOs: null,
        targetOs: null,
        hardware: null,
      },
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
    expect(
      template.results.every(
        (result) => Array.isArray(result.requiredEvidence) && result.requiredEvidence.length > 0,
      ),
    ).toBe(true);
    expect(evaluation.ready).toBe(false);
    expect(evaluation.summary).toEqual({
      total: qaCaseCount(),
      passed: 0,
      failed: 0,
      blocked: qaCaseCount(),
      skipped: 0,
    });
    expect(evaluation.checks.find((check) => check.id === "executionMetadata")).toMatchObject({
      status: "invalid",
      detail: "executedAtMustBeStrictIsoUtc",
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "setExecutionTimestamp",
      action: "set executedAt to a strict ISO UTC timestamp with milliseconds",
    });
    expect(evaluation.nextActions).toContainEqual({
      id: "rerunOrResolveCase",
      action: "rerun the QA case or resolve the blocker before release",
      caseId: "WIN-006",
    });
  });

  test("builds and parses a filtered report template", () => {
    const template = buildWindowsMvpQaReportTemplate({ caseIds: ["WIN-007"] });

    expect(template.results.map((result) => result.caseId)).toEqual(["WIN-007"]);
    expect(parseWindowsMvpQaReportArgs(["--template", "--case-id", "WIN-007"])).toEqual({
      template: true,
      updateResult: false,
      reportFile: undefined,
      outFile: undefined,
      caseIds: ["WIN-007"],
      result: undefined,
      reason: undefined,
      evidence: [],
      executedAt: undefined,
      sourceOs: undefined,
      targetOs: undefined,
      hardware: undefined,
      environmentNotes: undefined,
    });
    expect(() => buildWindowsMvpQaReportTemplate({ caseIds: ["NOPE-999"] })).toThrow(
      "unknown Windows MVP QA case id(s): NOPE-999",
    );
  });

  test("writes a filtered report template through the app package script", () => {
    const result = runAppPackageScript("qa:windows-mvp-report-template", ["--case-id", "WIN-002"]);
    const template = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(template.schemaVersion).toBe(WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION);
    expect(template.generatedFrom).toBe("qa:windows-mvp-report-template");
    expect(template.results).toHaveLength(1);
    expect(template.results[0].caseId).toBe("WIN-002");
    expect(template.privacy.includesTypedContent).toBe(false);
    expect(template.privacy.includesSecretValues).toBe(false);
    expect(template.privacy.includesFullFilePaths).toBe(false);
  });

  test("rejects conflicting template and report evaluation arguments", () => {
    expect(() =>
      parseWindowsMvpQaReportArgs(["--template", "--report-file", "qa-report.json"]),
    ).toThrow("--template cannot be combined with --report-file");
    expect(() => parseWindowsMvpQaReportArgs(["--case-id", "WIN-007"])).toThrow(
      "--case-id can only be used with --template",
    );
    expect(() => parseWindowsMvpQaReportArgs(["--result", "pass"])).toThrow(
      "--result can only be used with --update-result",
    );
  });

  test("requires strict execution timestamp and sanitized environment metadata", () => {
    const missingMetadataReport = passingReport();
    delete missingMetadataReport.environment;
    const invalidTimestampReport = passingReport();
    invalidTimestampReport.executedAt = "2026-06-20";
    const unsafeEnvironmentReport = passingReport();
    unsafeEnvironmentReport.environment.hardware = "C:\\Users\\cherr\\Desktop\\qa.json";

    const missingMetadataEvaluation = evaluateWindowsMvpQaReport(missingMetadataReport);
    const invalidTimestampEvaluation = evaluateWindowsMvpQaReport(invalidTimestampReport);
    const unsafeEnvironmentEvaluation = evaluateWindowsMvpQaReport(unsafeEnvironmentReport);

    expect(
      missingMetadataEvaluation.checks.find((check) => check.id === "executionMetadata"),
    ).toMatchObject({
      status: "missing",
      detail: "environmentMissing",
      fields: ["sourceOs", "targetOs", "hardware"],
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
    expect(missingMetadataEvaluation.nextActions).toEqual([
      {
        id: "setEnvironment",
        action: "record sourceOs, targetOs, and hardware without local paths or secrets",
        fields: ["sourceOs", "targetOs", "hardware"],
      },
    ]);
    expect(invalidTimestampEvaluation.nextActions).toEqual([
      {
        id: "setExecutionTimestamp",
        action: "set executedAt to a strict ISO UTC timestamp with milliseconds",
      },
    ]);
    expect(unsafeEnvironmentEvaluation.nextActions).toEqual([
      {
        id: "sanitizeEnvironment",
        action: "replace environment values that contain typed content, secrets, or local paths",
      },
    ]);
  });

  test("updates one QA result without hand-editing the report JSON", () => {
    const template = buildWindowsMvpQaReportTemplate();
    const updatedReport = updateWindowsMvpQaReportResult(template, {
      caseId: "WIN-006",
      result: "pass",
      evidence: [
        { id: "WIN-006-E1", note: "diagnostics support bundle artifact id" },
        { id: "WIN-006-E2", note: "soak report artifact id" },
      ],
      executedAt: "2026-06-20T01:02:03.004Z",
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "two physical Windows endpoints",
      environmentNotes: "sleep resume qa bench",
    });
    const updatedEvaluation = evaluateWindowsMvpQaReport(updatedReport);

    expect(updatedReport.executedAt).toBe("2026-06-20T01:02:03.004Z");
    expect(updatedReport.environment).toEqual({
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "two physical Windows endpoints",
      notes: "sleep resume qa bench",
    });
    expect(updatedReport.results.find((result) => result.caseId === "WIN-006")).toEqual({
      caseId: "WIN-006",
      result: "pass",
      evidence: [
        { id: "WIN-006-E1", note: "diagnostics support bundle artifact id" },
        { id: "WIN-006-E2", note: "soak report artifact id" },
      ],
    });
    expect(updatedEvaluation.checks.find((check) => check.id === "result:WIN-006")).toMatchObject({
      status: "pass",
    });
    expect(
      updatedEvaluation.checks.find((check) => check.id === "executionMetadata"),
    ).toMatchObject({
      status: "pass",
    });
  });

  test("updates one QA result through the app package script", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-qa-report-cli-"));
    const templateFile = join(tempDirectory, "template.json");
    const updatedFile = join(tempDirectory, "updated.json");

    try {
      const template = buildWindowsMvpQaReportTemplate();
      writeWindowsMvpQaReportOutputFile(templateFile, template);

      const result = runAppPackageScript("qa:windows-mvp-report-update", [
        "--report-file",
        templateFile,
        "--case-id",
        "WIN-006",
        "--result",
        "pass",
        "--evidence-id",
        "WIN-006-E1",
        "--evidence-note",
        "diagnostics support bundle artifact id",
        "--evidence-id",
        "WIN-006-E2",
        "--evidence-note",
        "soak report artifact id",
        "--executed-at",
        "2026-06-20T01:02:03.004Z",
        "--source-os",
        "Windows 11",
        "--target-os",
        "Windows 11",
        "--hardware",
        "two physical Windows endpoints",
        "--out-file",
        updatedFile,
      ]);

      expect(result.status).toBe(0);

      const updatedReport = JSON.parse(readFileSync(updatedFile, "utf8"));
      expect(updatedReport.results.find((candidate) => candidate.caseId === "WIN-006")).toEqual({
        caseId: "WIN-006",
        result: "pass",
        evidence: [
          { id: "WIN-006-E1", note: "diagnostics support bundle artifact id" },
          { id: "WIN-006-E2", note: "soak report artifact id" },
        ],
      });
      expect(JSON.parse(result.stdout)).toEqual(updatedReport);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects unsafe or incomplete QA result updates", () => {
    const template = buildWindowsMvpQaReportTemplate();

    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "WIN-006",
        result: "pass",
        evidence: [],
      }),
    ).toThrow("--result pass requires at least one --evidence value");
    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "WIN-006",
        result: "blocked",
        evidence: [],
      }),
    ).toThrow("--result blocked requires --reason");
    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "NOPE-999",
        result: "pass",
        evidence: ["artifact"],
      }),
    ).toThrow("unknown Windows MVP QA case id");
    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "WIN-006",
        result: "pass",
        evidence: ["WIN-006-E1: diagnostics support bundle artifact id"],
      }),
    ).toThrow("--result pass requires planned evidence for WIN-006: WIN-006-E2");
    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "WIN-006",
        result: "pass",
        evidence: [
          { id: "WIN-006-E1", note: "diagnostics support bundle artifact id" },
          { id: "NOPE-E2", note: "soak report artifact id" },
        ],
      }),
    ).toThrow("unknown planned evidence id for WIN-006: NOPE-E2");
    expect(() =>
      updateWindowsMvpQaReportResult(template, {
        caseId: "WIN-006",
        result: "pass",
        evidence: ["C:\\Users\\cherr\\Desktop\\qa.json"],
      }),
    ).toThrow("updated QA report values contain sensitive or local path data");
  });

  test("parses QA result update mode arguments", () => {
    expect(
      parseWindowsMvpQaReportArgs([
        "--update-result",
        "--report-file",
        "qa-report.json",
        "--case-id",
        "WIN-006",
        "--result",
        "pass",
        "--evidence-id",
        "WIN-006-E1",
        "--evidence-note",
        "support bundle artifact id",
        "--evidence-id",
        "WIN-006-E2",
        "--evidence-note",
        "soak report artifact id",
        "--executed-at",
        "2026-06-20T01:02:03.004Z",
        "--source-os",
        "Windows 11",
        "--target-os",
        "Windows 11",
        "--hardware",
        "two physical Windows endpoints",
        "--out-file",
        "updated-report.json",
      ]),
    ).toEqual({
      template: false,
      updateResult: true,
      reportFile: "qa-report.json",
      outFile: "updated-report.json",
      caseIds: ["WIN-006"],
      result: "pass",
      reason: undefined,
      evidence: [
        { id: "WIN-006-E1", note: "support bundle artifact id" },
        { id: "WIN-006-E2", note: "soak report artifact id" },
      ],
      executedAt: "2026-06-20T01:02:03.004Z",
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "two physical Windows endpoints",
      environmentNotes: undefined,
    });
    expect(() => parseWindowsMvpQaReportArgs(["--update-result", "--result", "pass"])).toThrow(
      "--update-result requires --report-file",
    );
    expect(() =>
      parseWindowsMvpQaReportArgs([
        "--update-result",
        "--report-file",
        "qa-report.json",
        "--case-id",
        "WIN-006",
      ]),
    ).toThrow("--update-result requires --result");
    expect(() =>
      parseWindowsMvpQaReportArgs([
        "--update-result",
        "--report-file",
        "qa-report.json",
        "--case-id",
        "WIN-006",
        "--result",
        "pass",
        "--evidence-id",
        "WIN-006-E1",
      ]),
    ).toThrow("--evidence-id requires a following --evidence-note");
    expect(() =>
      parseWindowsMvpQaReportArgs([
        "--update-result",
        "--report-file",
        "qa-report.json",
        "--case-id",
        "WIN-006",
        "--result",
        "pass",
        "--evidence-note",
        "support bundle artifact id",
      ]),
    ).toThrow("--evidence-note must follow --evidence-id");
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
        updateResult: false,
        reportFile: undefined,
        outFile: templateFile,
        caseIds: ["WIN-007"],
        result: undefined,
        reason: undefined,
        evidence: [],
        executedAt: undefined,
        sourceOs: undefined,
        targetOs: undefined,
        hardware: undefined,
        environmentNotes: undefined,
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

  test("evaluates a completed QA report through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-qa-report-eval-cli-"));
    const reportFile = join(tempDir, "qa-report.json");

    try {
      writeWindowsMvpQaReportOutputFile(reportFile, passingReport());

      const result = runAppPackageScript("qa:windows-mvp-report", ["--report-file", reportFile]);
      const evaluation = JSON.parse(result.stdout);

      expect(result.status).toBe(0);
      expect(evaluation.ready).toBe(true);
      expect(evaluation.summary.total).toBe(qaCaseCount());
      expect(evaluation.summary.passed).toBe(qaCaseCount());
      expect(evaluation.nextActions).toEqual([]);
      expect(evaluation.privacy.includesReportPayload).toBe(false);
      expect(evaluation.privacy.includesSecretValues).toBe(false);
      expect(evaluation.privacy.includesFullFilePaths).toBe(false);
      expect(result.stdout).not.toContain(reportFile);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });
});

describe("Windows MVP QA workflow payload", () => {
  test("encodes only a complete QA report for workflow dispatch input", () => {
    const report = passingReport();
    const payload = buildWindowsMvpQaWorkflowPayload(report);
    const payloadReport = buildWindowsMvpQaWorkflowPayloadReport(report, { payloadWritten: true });

    expect(JSON.parse(Buffer.from(payload, "base64").toString("utf8"))).toEqual(report);
    expect(payloadReport).toMatchObject({
      schemaVersion: WINDOWS_MVP_QA_WORKFLOW_PAYLOAD_SCHEMA_VERSION,
      ready: true,
      inputName: "qa_report_base64",
      payloadEncoding: "base64",
      payloadWritten: true,
      dispatchInputsWritten: false,
      payloadLength: payload.length,
      nextActions: [],
      privacy: {
        includesReportPayload: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
      },
    });
    expect(JSON.stringify(payloadReport)).not.toContain(payload);
    expect(payloadReport.evaluation.ready).toBe(true);
    expect(exitCodeForWindowsMvpQaWorkflowPayload(payloadReport)).toBe(0);
  });

  test("rejects incomplete QA reports without creating a payload", () => {
    const payloadReport = buildWindowsMvpQaWorkflowPayloadReport(buildWindowsMvpQaReportTemplate());

    expect(payloadReport.ready).toBe(false);
    expect(payloadReport.payloadWritten).toBe(false);
    expect(payloadReport.dispatchInputsWritten).toBe(false);
    expect(payloadReport.payloadLength).toBe(0);
    expect(payloadReport.nextActions.length).toBeGreaterThan(0);
    expect(payloadReport.evaluation.ready).toBe(false);
    expect(exitCodeForWindowsMvpQaWorkflowPayload(payloadReport)).toBe(1);
  });

  test("parses workflow payload arguments", () => {
    expect(
      parseWindowsMvpQaWorkflowPayloadArgs([
        "--report-file",
        "qa-report.json",
        "--out-file",
        "qa-report.base64.txt",
        "--dispatch-inputs-file",
        "qa-workflow-inputs.json",
        "--evaluation-out-file",
        "qa-evaluation.json",
      ]),
    ).toEqual({
      reportFile: "qa-report.json",
      outFile: "qa-report.base64.txt",
      dispatchInputsFile: "qa-workflow-inputs.json",
      evaluationOutFile: "qa-evaluation.json",
    });
    expect(
      parseWindowsMvpQaWorkflowPayloadArgs([
        "--report-file",
        "qa-report.json",
        "--dispatch-inputs-file",
        "qa-workflow-inputs.json",
      ]),
    ).toEqual({
      reportFile: "qa-report.json",
      outFile: undefined,
      dispatchInputsFile: "qa-workflow-inputs.json",
      evaluationOutFile: undefined,
    });
    expect(() => parseWindowsMvpQaWorkflowPayloadArgs(["--out-file", "payload.txt"])).toThrow(
      "--report-file is required",
    );
    expect(() => parseWindowsMvpQaWorkflowPayloadArgs(["--report-file", "qa-report.json"])).toThrow(
      "at least one of --out-file or --dispatch-inputs-file is required",
    );
    expect(() => parseWindowsMvpQaWorkflowPayloadArgs(["--unknown"])).toThrow(
      "unknown Windows MVP QA workflow payload argument: --unknown",
    );
  });

  test("writes workflow payload text and dispatch inputs atomically", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-qa-workflow-payload-"));
    const payloadFile = join(tempDirectory, "nested", "qa-report.base64.txt");
    const dispatchInputsFile = join(tempDirectory, "nested", "qa-workflow-inputs.json");
    const report = passingReport();
    const payload = buildWindowsMvpQaWorkflowPayload(report);

    try {
      expect(writeWindowsMvpQaWorkflowPayloadOutputFile(payloadFile, payload)).toBe(payloadFile);
      expect(readFileSync(payloadFile, "utf8")).toBe(`${payload}\n`);
      expect(writeWindowsMvpQaWorkflowDispatchInputsFile(dispatchInputsFile, payload)).toBe(
        dispatchInputsFile,
      );
      expect(JSON.parse(readFileSync(dispatchInputsFile, "utf8"))).toEqual({
        qa_report_base64: payload,
      });
      expect(readFileSync(dispatchInputsFile, "utf8").endsWith("\n")).toBe(true);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("writes workflow payload files through the app package script", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-qa-workflow-payload-cli-"));
    const reportFile = join(tempDirectory, "qa-report.json");
    const payloadFile = join(tempDirectory, "nested", "qa-report.base64.txt");
    const dispatchInputsFile = join(tempDirectory, "nested", "qa-workflow-inputs.json");
    const evaluationFile = join(tempDirectory, "nested", "qa-evaluation.json");
    const report = passingReport();

    try {
      writeWindowsMvpQaReportOutputFile(reportFile, report);

      const result = runAppPackageScript("qa:windows-mvp-workflow-payload", [
        "--report-file",
        reportFile,
        "--out-file",
        payloadFile,
        "--dispatch-inputs-file",
        dispatchInputsFile,
        "--evaluation-out-file",
        evaluationFile,
      ]);

      expect(result.status).toBe(0);

      const payload = readFileSync(payloadFile, "utf8").trim();
      const payloadReport = JSON.parse(result.stdout);

      expect(JSON.parse(Buffer.from(payload, "base64").toString("utf8"))).toEqual(report);
      expect(JSON.parse(readFileSync(dispatchInputsFile, "utf8"))).toEqual({
        qa_report_base64: payload,
      });
      expect(JSON.parse(readFileSync(evaluationFile, "utf8"))).toEqual(payloadReport.evaluation);
      expect(payloadReport).toMatchObject({
        schemaVersion: WINDOWS_MVP_QA_WORKFLOW_PAYLOAD_SCHEMA_VERSION,
        ready: true,
        payloadWritten: true,
        dispatchInputsWritten: true,
      });
      expect(JSON.stringify(payloadReport)).not.toContain(payload);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });
});
