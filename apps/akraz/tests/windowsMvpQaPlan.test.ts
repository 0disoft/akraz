import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

import {
  WINDOWS_MVP_QA_SOAK_EVIDENCE_LABEL_PREFIX,
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  buildWindowsMvpQaPlan,
  listWindowsMvpQaCaseIds,
  listWindowsMvpSoakEvidenceQaCaseIds,
  parseWindowsMvpQaPlanArgs,
} from "../scripts/windows-mvp-qa-plan.mjs";

function runAppPackageScript(scriptName, args) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Windows MVP QA plan", () => {
  test("covers the release-blocking M8 manual QA cases", () => {
    const plan = buildWindowsMvpQaPlan();

    expect(plan.schemaVersion).toBe(WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION);
    expect(plan.milestone).toBe("M8");
    expect(plan.caseCount).toBe(plan.cases.length);
    expect(plan.releaseBlockingCaseCount).toBe(plan.cases.length);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual([
      "WIN-001",
      "WIN-002",
      "WIN-003",
      "WIN-006",
      "WIN-007",
      "WIN-008",
      "WIN-010",
      "WIN-011",
      "I18N-001",
      "I18N-004",
      "REL-001",
    ]);
    expect(plan.privacy).toEqual({
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    });
  });

  test("keeps each case actionable without collecting sensitive payloads", () => {
    const plan = buildWindowsMvpQaPlan();

    for (const testCase of plan.cases) {
      expect(testCase.priority).toBe("release-blocking");
      expect(testCase.automation).toBe("manual");
      expect(testCase.prerequisites.length).toBeGreaterThan(0);
      expect(testCase.steps.length).toBeGreaterThan(0);
      expect(testCase.expected.length).toBeGreaterThan(0);
      expect(testCase.evidence.length).toBeGreaterThan(0);
      expect(testCase.evidenceRequirements).toHaveLength(testCase.evidence.length);
      expect(testCase.evidenceRequirements.map((evidence) => evidence.label)).toEqual(
        testCase.evidence,
      );
      expect(testCase.evidenceRequirements.map((evidence) => evidence.id)).toEqual(
        testCase.evidence.map((_, index) => `${testCase.id}-E${index + 1}`),
      );
      expect(JSON.stringify(testCase)).not.toContain("안녕");
      expect(JSON.stringify(testCase).toLowerCase()).not.toContain("konnichi");
      expect(JSON.stringify(testCase).toLowerCase()).not.toContain("-----begin");
    }
  });

  test("uses unique stable evidence requirement ids", () => {
    const plan = buildWindowsMvpQaPlan();
    const evidenceIds = plan.cases.flatMap((testCase) =>
      testCase.evidenceRequirements.map((evidence) => evidence.id),
    );

    expect(new Set(evidenceIds).size).toBe(evidenceIds.length);
    expect(evidenceIds.every((id) => /^(WIN|I18N|REL)-\d{3}-E\d+$/.test(id))).toBe(true);
  });

  test("derives soak-backed QA case ids from evidence requirements", () => {
    const soakEvidenceCaseIds = listWindowsMvpSoakEvidenceQaCaseIds();
    const plan = buildWindowsMvpQaPlan({ caseIds: soakEvidenceCaseIds });

    expect(soakEvidenceCaseIds).toEqual(["WIN-001", "WIN-002", "WIN-003", "WIN-006", "WIN-008"]);
    expect(plan.cases.every((testCase) => testCase.id.startsWith("WIN-"))).toBe(true);
    expect(
      plan.cases.every((testCase) =>
        testCase.evidence.some((label) =>
          label.startsWith(WINDOWS_MVP_QA_SOAK_EVIDENCE_LABEL_PREFIX),
        ),
      ),
    ).toBe(true);
  });

  test("filters by case id and rejects unknown case ids", () => {
    const plan = buildWindowsMvpQaPlan({ caseIds: ["WIN-008", "I18N-001"] });

    expect(plan.caseCount).toBe(2);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual(["WIN-008", "I18N-001"]);
    expect(() => buildWindowsMvpQaPlan({ caseIds: ["NOPE-999"] })).toThrow(
      "unknown Windows MVP QA case id",
    );
  });

  test("parses list and repeated case id arguments", () => {
    expect(listWindowsMvpQaCaseIds()).toEqual([
      "WIN-001",
      "WIN-002",
      "WIN-003",
      "WIN-006",
      "WIN-007",
      "WIN-008",
      "WIN-010",
      "WIN-011",
      "I18N-001",
      "I18N-004",
      "REL-001",
    ]);
    expect(parseWindowsMvpQaPlanArgs(["--list"])).toEqual({ list: true, caseIds: [] });
    expect(parseWindowsMvpQaPlanArgs(["--case-id", "WIN-006", "--case-id", "REL-001"])).toEqual({
      list: false,
      caseIds: ["WIN-006", "REL-001"],
    });
  });

  test("lists QA case ids through the app package script", () => {
    const result = runAppPackageScript("qa:windows-mvp-plan", ["--list"]);
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.cases).toEqual(listWindowsMvpQaCaseIds());
  });

  test("filters the QA plan through the app package script", () => {
    const result = runAppPackageScript("qa:windows-mvp-plan", [
      "--case-id",
      "WIN-008",
      "--case-id",
      "I18N-001",
    ]);
    const plan = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(plan.schemaVersion).toBe(WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION);
    expect(plan.caseCount).toBe(2);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual(["WIN-008", "I18N-001"]);
    expect(plan.privacy.includesTypedContent).toBe(false);
    expect(plan.privacy.includesSecretValues).toBe(false);
    expect(plan.privacy.includesFullFilePaths).toBe(false);
  });
});
