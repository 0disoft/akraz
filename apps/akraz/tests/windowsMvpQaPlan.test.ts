import { describe, expect, test } from "bun:test";

import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  buildWindowsMvpQaPlan,
  listWindowsMvpQaCaseIds,
  parseWindowsMvpQaPlanArgs,
} from "../scripts/windows-mvp-qa-plan.mjs";

describe("Windows MVP QA plan", () => {
  test("covers the release-blocking M8 manual QA cases", () => {
    const plan = buildWindowsMvpQaPlan();

    expect(plan.schemaVersion).toBe(WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION);
    expect(plan.milestone).toBe("M8");
    expect(plan.caseCount).toBe(plan.cases.length);
    expect(plan.releaseBlockingCaseCount).toBe(plan.cases.length);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual([
      "WIN-006",
      "WIN-007",
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
      expect(JSON.stringify(testCase)).not.toContain("안녕");
      expect(JSON.stringify(testCase).toLowerCase()).not.toContain("konnichi");
      expect(JSON.stringify(testCase).toLowerCase()).not.toContain("-----begin");
    }
  });

  test("filters by case id and rejects unknown case ids", () => {
    const plan = buildWindowsMvpQaPlan({ caseIds: ["WIN-007", "I18N-001"] });

    expect(plan.caseCount).toBe(2);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual(["WIN-007", "I18N-001"]);
    expect(() => buildWindowsMvpQaPlan({ caseIds: ["NOPE-999"] })).toThrow(
      "unknown Windows MVP QA case id",
    );
  });

  test("parses list and repeated case id arguments", () => {
    expect(listWindowsMvpQaCaseIds()).toEqual([
      "WIN-006",
      "WIN-007",
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
});
