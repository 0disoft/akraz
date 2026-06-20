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
    expect(report.workflowScripts).toContain("release:windows-mvp-evidence-sources");
    expect(report.workflowScripts).toContain("release:windows-mvp-resolved-evidence");
    expect(report.workflowScripts).toContain("smoke:windows-mvp-soak");
    expect(report.checks.find((check) => check.id === "smokeWorkflowCoverage")).toMatchObject({
      status: "pass",
      workflowFile: "check.yml",
      soakReportPath: "apps/akraz/reports/windows-mvp-soak-smoke.json",
    });
    expect(
      report.checks.find((check) => check.id === "releaseEvidenceSourcesWiring"),
    ).toMatchObject({
      status: "pass",
      evidenceSourcesManifestPath:
        "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-evidence-sources.json",
      workflowInputsManifestPath:
        "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-workflow-inputs.json",
    });
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

  test("rejects release workflow drift when evidence sources are not bundled", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-workflow-contracts-"));

    try {
      writeFileSync(
        join(tempDirectory, "package.json"),
        readFileSync("package.json", "utf8"),
        "utf8",
      );
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      for (const workflowFile of ["check.yml", "windows-mvp-qa.yml", "windows-mvp-soak.yml"]) {
        writeFileSync(
          join(tempDirectory, ".github", "workflows", workflowFile),
          readFileSync(join(".github", "workflows", workflowFile), "utf8"),
          "utf8",
        );
      }

      const releaseWorkflowWithoutBundleEvidenceSources = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      ).replace(
        [
          "          bun run release:windows-mvp-bundle -- \\",
          '            --evidence-sources-file "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-evidence-sources.json" \\',
        ].join("\n"),
        "          bun run release:windows-mvp-bundle -- \\",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithoutBundleEvidenceSources,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "releaseEvidenceSourcesWiring"),
      ).toMatchObject({
        status: "invalid",
        detail: "releaseEvidenceSourcesWiringDrifted",
        missingSnippets: ["releaseBundleEvidenceSourcesCommand"],
      });
      expect(report.nextActions).toContainEqual({
        id: "syncReleaseEvidenceSourcesWiring",
        action: "wire the release evidence source manifest generation into the bundle command",
        workflowFile: "windows-mvp-release.yml",
        missingSnippets: ["releaseBundleEvidenceSourcesCommand"],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects release workflow drift when bundle artifact integrity is not smoked", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-bundle-smoke-contracts-"));

    try {
      writeFileSync(
        join(tempDirectory, "package.json"),
        readFileSync("package.json", "utf8"),
        "utf8",
      );
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      for (const workflowFile of ["check.yml", "windows-mvp-qa.yml", "windows-mvp-soak.yml"]) {
        writeFileSync(
          join(tempDirectory, ".github", "workflows", workflowFile),
          readFileSync(join(".github", "workflows", workflowFile), "utf8"),
          "utf8",
        );
      }

      const releaseWorkflowWithoutBundleIntegritySmoke = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      ).replace(
        [
          "      - name: Verify Windows MVP release bundle artifact integrity",
          "        env:",
          "          RELEASE_BUNDLE_DIR: ${{ github.workspace }}/release-bundle",
          "        run: |",
          "          set -euo pipefail",
          "",
          '          bun run smoke:windows-mvp-release-bundle -- --bundle-dir "$RELEASE_BUNDLE_DIR"',
          "",
        ].join("\n"),
        "",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithoutBundleIntegritySmoke,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "releaseEvidenceSourcesWiring"),
      ).toMatchObject({
        status: "invalid",
        detail: "releaseEvidenceSourcesWiringDrifted",
        missingSnippets: [
          "releaseBundleIntegritySmokeScript",
          "releaseBundleIntegrityBundleDirArgument",
        ],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects release workflow drift when resolved evidence filenames are not checked", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-resolved-evidence-contracts-"));

    try {
      writeFileSync(
        join(tempDirectory, "package.json"),
        readFileSync("package.json", "utf8"),
        "utf8",
      );
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      for (const workflowFile of ["check.yml", "windows-mvp-qa.yml", "windows-mvp-soak.yml"]) {
        writeFileSync(
          join(tempDirectory, ".github", "workflows", workflowFile),
          readFileSync(join(".github", "workflows", workflowFile), "utf8"),
          "utf8",
        );
      }

      const releaseWorkflowWithoutResolvedEvidenceCheck = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      ).replace(
        [
          "          bun run release:windows-mvp-resolved-evidence -- \\",
          '            --evidence-sources-file "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-evidence-sources.json" \\',
          '            --qa-report-file "$qa_report_file" \\',
          '            --soak-report-file "$soak_report_file"',
          "",
        ].join("\n"),
        "",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithoutResolvedEvidenceCheck,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "releaseEvidenceSourcesWiring"),
      ).toMatchObject({
        status: "invalid",
        detail: "releaseEvidenceSourcesWiringDrifted",
        missingSnippets: ["resolvedEvidenceCommand"],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects smoke workflow drift when required runtime smoke scripts are removed", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-smoke-workflow-contracts-"));

    try {
      writeFileSync(
        join(tempDirectory, "package.json"),
        readFileSync("package.json", "utf8"),
        "utf8",
      );
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      for (const workflowFile of [
        "windows-mvp-qa.yml",
        "windows-mvp-release.yml",
        "windows-mvp-soak.yml",
      ]) {
        writeFileSync(
          join(tempDirectory, ".github", "workflows", workflowFile),
          readFileSync(join(".github", "workflows", workflowFile), "utf8"),
          "utf8",
        );
      }

      const checkWorkflowWithoutPeerSessionSmoke = readFileSync(
        join(".github", "workflows", "check.yml"),
        "utf8",
      ).replace("      - run: bun run smoke:peer-session\n", "");
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "check.yml"),
        checkWorkflowWithoutPeerSessionSmoke,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "smokeWorkflowCoverage")).toMatchObject({
        status: "invalid",
        detail: "smokeWorkflowCoverageDrifted",
        missingScripts: ["smoke:peer-session"],
        missingSnippets: [],
      });
      expect(report.nextActions).toContainEqual({
        id: "syncSmokeWorkflowCoverage",
        action: "restore the Windows smoke workflow scripts and soak report artifact wiring",
        workflowFile: "check.yml",
        missingScripts: ["smoke:peer-session"],
        missingSnippets: [],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects smoke workflow drift when soak evidence is not uploaded", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-smoke-soak-contracts-"));

    try {
      writeFileSync(
        join(tempDirectory, "package.json"),
        readFileSync("package.json", "utf8"),
        "utf8",
      );
      mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
      for (const workflowFile of [
        "windows-mvp-qa.yml",
        "windows-mvp-release.yml",
        "windows-mvp-soak.yml",
      ]) {
        writeFileSync(
          join(tempDirectory, ".github", "workflows", workflowFile),
          readFileSync(join(".github", "workflows", workflowFile), "utf8"),
          "utf8",
        );
      }

      const checkWorkflowWithoutSoakUpload = readFileSync(
        join(".github", "workflows", "check.yml"),
        "utf8",
      )
        .replace(" --report-file reports/windows-mvp-soak-smoke.json", "")
        .replace("          path: apps/akraz/reports/windows-mvp-soak-smoke.json\n", "");
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "check.yml"),
        checkWorkflowWithoutSoakUpload,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "smokeWorkflowCoverage")).toMatchObject({
        status: "invalid",
        detail: "smokeWorkflowCoverageDrifted",
        missingScripts: [],
        missingSnippets: ["smokeSoakReportFile", "smokeSoakUploadPath"],
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
