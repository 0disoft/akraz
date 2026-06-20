import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  WORKFLOW_CONTRACTS_SCHEMA_VERSION,
  buildWorkflowContractsReport,
  exitCodeForWorkflowContracts,
} from "../scripts/verify-workflow-contracts.mjs";
import {
  TAURI_SIDECAR_EXTERNAL_BIN,
  WINDOWS_CI_SIDECAR_FILE_NAME,
  buildCargoArgs,
  sidecarFileName,
} from "../scripts/prepare-sidecar.mjs";

function runAppPackageScript(scriptName, args = []) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("GitHub Actions workflow contracts", () => {
  test("keeps prepare-sidecar helpers import-safe for contract checks", () => {
    expect(TAURI_SIDECAR_EXTERNAL_BIN).toBe("binaries/akraz-daemon");
    expect(WINDOWS_CI_SIDECAR_FILE_NAME).toBe("akraz-daemon-x86_64-pc-windows-msvc.exe");
    expect(sidecarFileName("x86_64-unknown-linux-gnu", "linux")).toBe(
      "akraz-daemon-x86_64-unknown-linux-gnu",
    );
    expect(buildCargoArgs()).toEqual(["build", "-p", "akraz-daemon"]);
    expect(buildCargoArgs({ release: true })).toEqual(["build", "-p", "akraz-daemon", "--release"]);
  });

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
    expect(
      report.checks.find((check) => check.id === "workspaceAppScript:smoke:tcp-transport"),
    ).toMatchObject({
      status: "pass",
      expectedCommand: "bun run --cwd apps/akraz smoke:tcp-transport",
    });
    expect(
      report.checks.find((check) => check.id === "appPackageScript:smoke:settings-start"),
    ).toMatchObject({
      status: "pass",
      expectedCommand: "bun scripts/smoke-daemon-lifecycle.mjs settings-start",
    });
    expect(report.checks.find((check) => check.id === "tauriSidecarContract")).toMatchObject({
      status: "pass",
      workflowFile: "check.yml",
      externalBin: "binaries/akraz-daemon",
      windowsCiSidecarDestination:
        "apps/akraz/src-tauri/binaries/akraz-daemon-x86_64-pc-windows-msvc.exe",
    });
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
    expect(report.checks.find((check) => check.id === "qaWorkflowInputWiring")).toMatchObject({
      status: "pass",
      workflowFile: "windows-mvp-qa.yml",
      inputNames: ["qa_report_json", "qa_report_base64"],
    });
    expect(report.checks.find((check) => check.id === "releaseWorkflowInputWiring")).toMatchObject({
      status: "pass",
      workflowFile: "windows-mvp-release.yml",
      inputNames: [
        "source_run_id",
        "qa_source_run_id",
        "soak_source_run_id",
        "qa_report_artifact",
        "soak_report_artifact",
      ],
    });
    expect(report.checks.find((check) => check.id === "releaseBundleOutputWiring")).toMatchObject({
      status: "pass",
      workflowFile: "windows-mvp-release.yml",
      artifactName: "windows-mvp-release-bundle",
      uploadPath: "release-bundle/*.json",
      expectedBundleFiles: [
        "windows-mvp-qa-report.json",
        "windows-mvp-release-bundle.json",
        "windows-mvp-release-evidence-sources.json",
        "windows-mvp-release-gate.json",
        "windows-mvp-signing-preflight.json",
        "windows-mvp-soak-report.json",
        "windows-mvp-updater-config-preflight.json",
      ],
    });
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(report.privacy).toEqual({
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesWorkflowPayloads: false,
    });
    expect(exitCodeForWorkflowContracts(report)).toBe(0);
  });

  test("verifies workflow contracts through the app package script", () => {
    const result = runAppPackageScript("verify:workflow-contracts");
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.schemaVersion).toBe(WORKFLOW_CONTRACTS_SCHEMA_VERSION);
    expect(report.ready).toBe(true);
    expect(report.checks.find((check) => check.id === "smokeWorkflowCoverage")).toMatchObject({
      status: "pass",
      workflowFile: "check.yml",
    });
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(report.privacy.includesFullFilePaths).toBe(false);
    expect(report.privacy.includesWorkflowPayloads).toBe(false);
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
      writeCurrentAppPackageFixture(tempDirectory);
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
      writeCurrentPackageFixtures(tempDirectory);
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

  test("rejects release workflow drift when evidence source CLI inputs are not wired", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-evidence-args-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
      copyCurrentWorkflows(tempDirectory);

      const releaseWorkflowWithoutQaArtifactArgument = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      ).replace('            args+=("--qa-report-artifact" "$QA_REPORT_ARTIFACT")\n', "");
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithoutQaArtifactArgument,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "releaseEvidenceSourcesWiring"),
      ).toMatchObject({
        status: "invalid",
        detail: "releaseEvidenceSourcesWiringDrifted",
        missingSnippets: ["evidenceSourcesQaArtifactArgument"],
      });
      expect(report.nextActions).toContainEqual({
        id: "syncReleaseEvidenceSourcesWiring",
        action: "wire the release evidence source manifest generation into the bundle command",
        workflowFile: "windows-mvp-release.yml",
        missingSnippets: ["evidenceSourcesQaArtifactArgument"],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects Tauri sidecar drift before Windows smoke jobs run", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-sidecar-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
      copyCurrentWorkflows(tempDirectory);

      const tauriConfig = JSON.parse(
        readFileSync(join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json"), "utf8"),
      );
      tauriConfig.bundle.externalBin = ["binaries/other-daemon"];
      writeJson(join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json"), tauriConfig);

      const checkWorkflowWithoutSidecarCopy = readFileSync(
        join(".github", "workflows", "check.yml"),
        "utf8",
      ).replace(
        "          Copy-Item target/debug/akraz-daemon.exe apps/akraz/src-tauri/binaries/akraz-daemon-x86_64-pc-windows-msvc.exe",
        "          Copy-Item target/debug/akraz-daemon.exe apps/akraz/src-tauri/binaries/akraz-daemon.exe",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "check.yml"),
        checkWorkflowWithoutSidecarCopy,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "tauriSidecarContract")).toMatchObject({
        status: "invalid",
        detail: "tauriSidecarContractDrifted",
        missingConfigEntries: ["externalBin"],
        missingWorkflowSnippets: ["windowsCopiesSidecarBinary"],
        expected: {
          externalBin: "binaries/akraz-daemon",
          windowsCiSidecarDestination:
            "apps/akraz/src-tauri/binaries/akraz-daemon-x86_64-pc-windows-msvc.exe",
        },
      });
      expect(report.nextActions).toContainEqual({
        id: "syncTauriSidecarContract",
        action: "sync Tauri externalBin, prepare-sidecar hooks, and Windows CI sidecar copy",
        workflowFile: "check.yml",
        missingConfigEntries: ["externalBin"],
        missingWorkflowSnippets: ["windowsCopiesSidecarBinary"],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects QA workflow drift when generated payload input is not wired", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-qa-workflow-input-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
      copyCurrentWorkflows(tempDirectory);

      const qaWorkflowWithoutPayloadMapping = readFileSync(
        join(".github", "workflows", "windows-mvp-qa.yml"),
        "utf8",
      ).replace(
        "          QA_REPORT_BASE64: ${{ inputs.qa_report_base64 }}",
        "          QA_REPORT_BASE64: ${{ inputs.qa_payload_base64 }}",
      );
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-qa.yml"),
        qaWorkflowWithoutPayloadMapping,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "qaWorkflowInputWiring")).toMatchObject({
        status: "invalid",
        detail: "qaWorkflowInputWiringDrifted",
        missingInputDefinitions: [],
        missingEnvMappings: [
          {
            inputName: "qa_report_base64",
            envName: "QA_REPORT_BASE64",
            minReferences: 1,
          },
        ],
      });
      expect(report.nextActions).toContainEqual({
        id: "syncQaWorkflowInputs",
        action: "sync the Windows MVP QA workflow dispatch inputs and environment mappings",
        workflowFile: "windows-mvp-qa.yml",
        missingInputDefinitions: [],
        missingEnvMappings: [
          {
            inputName: "qa_report_base64",
            envName: "QA_REPORT_BASE64",
            minReferences: 1,
          },
        ],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects release workflow drift when dispatch inputs are not wired into both release steps", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-input-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
      copyCurrentWorkflows(tempDirectory);

      const releaseWorkflowWithMissingSourceMapping = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      ).replace("          QA_SOURCE_RUN_ID: ${{ inputs.qa_source_run_id }}\n", "");
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithMissingSourceMapping,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "releaseWorkflowInputWiring"),
      ).toMatchObject({
        status: "invalid",
        detail: "releaseWorkflowInputWiringDrifted",
        missingInputDefinitions: [],
        missingEnvMappings: [
          {
            inputName: "qa_source_run_id",
            envName: "QA_SOURCE_RUN_ID",
            minReferences: 2,
          },
        ],
      });
      expect(report.nextActions).toContainEqual({
        id: "syncReleaseWorkflowInputs",
        action: "sync the Windows MVP release workflow dispatch inputs and environment mappings",
        workflowFile: "windows-mvp-release.yml",
        missingInputDefinitions: [],
        missingEnvMappings: [
          {
            inputName: "qa_source_run_id",
            envName: "QA_SOURCE_RUN_ID",
            minReferences: 2,
          },
        ],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects release workflow drift when bundle artifact integrity is not smoked", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-bundle-smoke-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
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

  test("rejects release workflow drift when bundle output is not uploaded from the canonical directory", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-bundle-output-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
      copyCurrentWorkflows(tempDirectory);

      const releaseWorkflowWithoutCanonicalBundleOutput = readFileSync(
        join(".github", "workflows", "windows-mvp-release.yml"),
        "utf8",
      )
        .replace('            --out-dir "$RELEASE_BUNDLE_DIR"\n', "")
        .replace("          path: release-bundle/*.json", "          path: release-output/*.json");
      writeFileSync(
        join(tempDirectory, ".github", "workflows", "windows-mvp-release.yml"),
        releaseWorkflowWithoutCanonicalBundleOutput,
        "utf8",
      );

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "releaseBundleOutputWiring")).toMatchObject(
        {
          status: "invalid",
          detail: "releaseBundleOutputWiringDrifted",
          missingSnippets: ["releaseBundleOutDirArgument", "releaseBundleUploadPath"],
          expected: {
            artifactName: "windows-mvp-release-bundle",
            uploadPath: "release-bundle/*.json",
            expectedBundleFiles: [
              "windows-mvp-qa-report.json",
              "windows-mvp-release-bundle.json",
              "windows-mvp-release-evidence-sources.json",
              "windows-mvp-release-gate.json",
              "windows-mvp-signing-preflight.json",
              "windows-mvp-soak-report.json",
              "windows-mvp-updater-config-preflight.json",
            ],
          },
        },
      );
      expect(report.nextActions).toContainEqual({
        id: "syncReleaseBundleOutputWiring",
        action: "restore release bundle out-dir, integrity smoke, and artifact upload wiring",
        workflowFile: "windows-mvp-release.yml",
        missingSnippets: ["releaseBundleOutDirArgument", "releaseBundleUploadPath"],
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects release workflow drift when resolved evidence filenames are not checked", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-release-resolved-evidence-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory);
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
      writeCurrentPackageFixtures(tempDirectory);
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
      writeCurrentPackageFixtures(tempDirectory);
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

  test("rejects root package script drift before heavy smoke scripts run", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-workspace-script-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory, {
        rootScripts: {
          "smoke:tcp-transport": "bun run --cwd apps/akraz smoke:peer-session",
        },
      });
      copyCurrentWorkflows(tempDirectory);

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "workspaceAppScript:smoke:tcp-transport"),
      ).toMatchObject({
        status: "invalid",
        detail: "workspaceScriptDelegationDrifted",
        expectedCommand: "bun run --cwd apps/akraz smoke:tcp-transport",
        actualCommand: "bun run --cwd apps/akraz smoke:peer-session",
      });
      expect(report.nextActions).toContainEqual({
        id: "syncWorkspaceAppScript",
        action: "sync the root package script with the app package script",
        scriptName: "smoke:tcp-transport",
        expectedCommand: "bun run --cwd apps/akraz smoke:tcp-transport",
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects app package script command drift before heavy smoke scripts run", () => {
    const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-app-script-contracts-"));

    try {
      writeCurrentPackageFixtures(tempDirectory, {
        appScripts: {
          "smoke:settings-start": "bun scripts/smoke-daemon-lifecycle.mjs",
        },
      });
      copyCurrentWorkflows(tempDirectory);

      const report = buildWorkflowContractsReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(
        report.checks.find((check) => check.id === "appPackageScript:smoke:settings-start"),
      ).toMatchObject({
        status: "invalid",
        detail: "appPackageScriptDrifted",
        expectedCommand: "bun scripts/smoke-daemon-lifecycle.mjs settings-start",
        actualCommand: "bun scripts/smoke-daemon-lifecycle.mjs",
      });
      expect(report.nextActions).toContainEqual({
        id: "syncAppPackageScript",
        action: "restore the expected app package script command",
        scriptName: "smoke:settings-start",
        expectedCommand: "bun scripts/smoke-daemon-lifecycle.mjs settings-start",
      });
      expect(exitCodeForWorkflowContracts(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });
});

function copyCurrentWorkflows(tempDirectory: string) {
  mkdirSync(join(tempDirectory, ".github", "workflows"), { recursive: true });
  for (const workflowFile of [
    "check.yml",
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
}

function writeCurrentPackageFixtures(
  tempDirectory: string,
  overrides: {
    appScripts?: Record<string, string>;
    rootScripts?: Record<string, string>;
  } = {},
) {
  const rootPackage = JSON.parse(readFileSync("package.json", "utf8"));
  rootPackage.scripts = {
    ...rootPackage.scripts,
    ...overrides.rootScripts,
  };
  writeJson(join(tempDirectory, "package.json"), rootPackage);
  writeCurrentAppPackageFixture(tempDirectory, overrides.appScripts);
}

function writeCurrentAppPackageFixture(
  tempDirectory: string,
  scriptOverrides: Record<string, string> = {},
) {
  const appPackage = JSON.parse(readFileSync(join("apps", "akraz", "package.json"), "utf8"));
  appPackage.scripts = {
    ...appPackage.scripts,
    ...scriptOverrides,
  };
  mkdirSync(join(tempDirectory, "apps", "akraz"), { recursive: true });
  writeJson(join(tempDirectory, "apps", "akraz", "package.json"), appPackage);
  mkdirSync(join(tempDirectory, "apps", "akraz", "src-tauri"), { recursive: true });
  writeFileSync(
    join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json"),
    readFileSync(join("apps", "akraz", "src-tauri", "tauri.conf.json"), "utf8"),
    "utf8",
  );
}

function writeJson(path: string, payload: unknown) {
  writeFileSync(path, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}
