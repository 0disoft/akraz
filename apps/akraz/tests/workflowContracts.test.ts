import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  WORKFLOW_CONTRACTS_SCHEMA_VERSION,
  buildWorkflowContractsReport,
  exitCodeForWorkflowContracts,
} from "../scripts/verify-workflow-contracts.mjs";

describe("GitHub Actions workflow contracts", () => {
  test("accepts the repository workflows and package scripts", () => {
    const report = buildWorkflowContractsReport();

    expect(report.schemaVersion).toBe(WORKFLOW_CONTRACTS_SCHEMA_VERSION);
    expect(report.ready).toBe(true);
    expect(report.expectedBunVersion).toBe("1.3.14");
    expect(report.workflowFiles).toEqual([
      "check.yml",
      "windows-mvp-qa.yml",
      "windows-mvp-release.yml",
      "windows-mvp-soak.yml",
    ]);
    expect(report.workflowScripts).toContain("release:windows-mvp-bundle");
    expect(report.workflowScripts).toContain("smoke:windows-mvp-soak");
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(report.privacy).toEqual({
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesWorkflowPayloads: false,
    });
    expect(exitCodeForWorkflowContracts(report)).toBe(0);
  });

  test("rejects workflow drift before Actions has to start a runner", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-workflow-contracts-"));

    try {
      writeJson(join(tempDirectory, "package.json"), {
        packageManager: "bun@1.3.14",
        scripts: {
          test: "bun test",
        },
      });
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "check.yml"),
        [
          "name: check",
          "jobs:",
          "  frontend:",
          "    steps:",
          "      - uses: actions/checkout@v5",
          "      - uses: oven-sh/setup-bun@v2",
          "        with:",
          "          bun-version: 1.3.13",
          "      - run: bun run missing:script",
          "",
        ].join("\n"),
        "utf8",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        readFileSync(join(".github", "workflows", "windows-mvp-release.yml"), "utf8"),
        "utf8",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-qa.yml"),
        readFileSync(join(".github", "workflows", "windows-mvp-qa.yml"), "utf8"),
        "utf8",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-soak.yml"),
        readFileSync(join(".github", "workflows", "windows-mvp-soak.yml"), "utf8"),
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id.startsWith("checkoutVersion:check.yml")),
      ).toMatchObject({
        status: "invalid",
        expectedVersion: "v6",
        actualVersion: "v5",
      });
      expect(
        report.checks.find((check) => check.id.startsWith("bunVersion:check.yml")),
      ).toMatchObject({
        status: "invalid",
        expectedBunVersion: "1.3.14",
        actualVersion: "1.3.13",
      });
      expect(
        report.checks.find((check) => check.id === "workflowScript:missing:script"),
      ).toMatchObject({
        status: "invalid",
        detail: "packageScriptMissing",
      });
      expect(report.nextActions).toContainEqual({
        id: "upgradeCheckoutAction",
        action: "use actions/checkout@v6",
        workflowFile: "check.yml",
      });
      expect(report.nextActions).toContainEqual({
        id: "addPackageScript",
        action: "add the workflow-called package script or update the workflow command",
        scriptName: "missing:script",
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });
});

function writeJson(path: string, payload: unknown) {
  writeFileSync(path, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}
