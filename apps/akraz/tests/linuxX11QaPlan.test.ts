import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

import {
  LINUX_X11_QA_PLAN_SCHEMA_VERSION,
  buildLinuxX11QaPlan,
  listLinuxX11QaCaseIds,
  parseLinuxX11QaPlanArgs,
} from "../scripts/linux-x11-qa-plan.mjs";

function runAppPackageScript(scriptName: string, args: string[]) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Linux X11 QA plan", () => {
  test("covers the release-blocking M9 manual QA cases", () => {
    const plan = buildLinuxX11QaPlan();

    expect(plan.schemaVersion).toBe(LINUX_X11_QA_PLAN_SCHEMA_VERSION);
    expect(plan.milestone).toBe("M9");
    expect(plan.target).toBe("Linux X11 alpha evidence gate");
    expect(plan.caseCount).toBe(plan.cases.length);
    expect(plan.releaseBlockingCaseCount).toBe(plan.cases.length);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual([
      "LX11-001",
      "LX11-002",
      "LX11-003",
      "LX11-004",
      "LX11-005",
      "LX11-006",
    ]);
    expect(plan.privacy).toEqual({
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    });
  });

  test("keeps each case actionable without collecting sensitive payloads", () => {
    const plan = buildLinuxX11QaPlan();

    for (const testCase of plan.cases) {
      expect(testCase.priority).toBe("release-blocking");
      expect(testCase.automation).toBe("manual");
      expect(testCase.prerequisites.length).toBeGreaterThan(0);
      expect(testCase.steps.length).toBeGreaterThan(0);
      expect(testCase.expected.length).toBeGreaterThan(0);
      expect(testCase.evidence.length).toBeGreaterThan(0);
      expect(testCase.environment.sourceSession).toBeTruthy();
      expect(testCase.environment.targetSession).toBeTruthy();
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
      expect(JSON.stringify(testCase)).not.toContain("/home/");
    }
  });

  test("uses unique stable evidence requirement ids", () => {
    const plan = buildLinuxX11QaPlan();
    const evidenceIds = plan.cases.flatMap((testCase) =>
      testCase.evidenceRequirements.map((evidence) => evidence.id),
    );

    expect(new Set(evidenceIds).size).toBe(evidenceIds.length);
    expect(evidenceIds.every((id) => /^LX11-\d{3}-E\d+$/.test(id))).toBe(true);
  });

  test("filters by case id and rejects unknown case ids", () => {
    const plan = buildLinuxX11QaPlan({ caseIds: ["LX11-001", "LX11-006"] });

    expect(plan.caseCount).toBe(2);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual(["LX11-001", "LX11-006"]);
    expect(() => buildLinuxX11QaPlan({ caseIds: ["NOPE-999"] })).toThrow(
      "unknown Linux X11 QA case id",
    );
  });

  test("parses list and repeated case id arguments", () => {
    expect(listLinuxX11QaCaseIds()).toEqual([
      "LX11-001",
      "LX11-002",
      "LX11-003",
      "LX11-004",
      "LX11-005",
      "LX11-006",
    ]);
    expect(parseLinuxX11QaPlanArgs(["--list"])).toEqual({ list: true, caseIds: [] });
    expect(parseLinuxX11QaPlanArgs(["--case-id", "LX11-003", "--case-id", "LX11-006"])).toEqual({
      list: false,
      caseIds: ["LX11-003", "LX11-006"],
    });
  });

  test("lists QA case ids through the app package script", () => {
    const result = runAppPackageScript("qa:linux-x11-plan", ["--list"]);
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.cases).toEqual(listLinuxX11QaCaseIds());
  });

  test("filters the QA plan through the app package script", () => {
    const result = runAppPackageScript("qa:linux-x11-plan", [
      "--case-id",
      "LX11-002",
      "--case-id",
      "LX11-003",
    ]);
    const plan = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(plan.schemaVersion).toBe(LINUX_X11_QA_PLAN_SCHEMA_VERSION);
    expect(plan.caseCount).toBe(2);
    expect(plan.cases.map((testCase) => testCase.id)).toEqual(["LX11-002", "LX11-003"]);
    expect(plan.privacy.includesTypedContent).toBe(false);
    expect(plan.privacy.includesSecretValues).toBe(false);
    expect(plan.privacy.includesFullFilePaths).toBe(false);
  });
});
