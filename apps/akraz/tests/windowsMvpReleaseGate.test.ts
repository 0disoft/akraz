import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  evaluateSigningPreflight,
  SIGNING_PREFLIGHT_SCHEMA_VERSION,
} from "../scripts/smoke-signing-preflight.mjs";
import { evaluateUpdaterConfigPreflight } from "../scripts/smoke-updater-config-preflight.mjs";
import {
  WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
  buildWindowsMvpQaPlan,
} from "../scripts/windows-mvp-qa-plan.mjs";
import {
  WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  buildWindowsMvpQaReportTemplate,
} from "../scripts/windows-mvp-qa-report.mjs";
import {
  DEFAULT_DURATION_MS,
  WINDOWS_MVP_SOAK_SCHEMA_VERSION,
  WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
} from "../scripts/windows-mvp-soak-report.mjs";
import {
  WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
  buildWindowsMvpReleaseGateReport,
  exitCodeForWindowsMvpReleaseGate,
  parseWindowsMvpReleaseGateArgs,
  writeWindowsMvpReleaseGateOutputFile,
} from "../scripts/windows-mvp-release-gate.mjs";
import {
  WINDOWS_MVP_RELEASE_GATE_SMOKE_SCHEMA_VERSION,
  buildWindowsMvpReleaseGateSmokeReport,
  exitCodeForWindowsMvpReleaseGateSmoke,
} from "../scripts/smoke-windows-mvp-release-gate.mjs";
import {
  WINDOWS_MVP_RELEASE_BUNDLE_ARTIFACT_INTEGRITY_SCHEMA_VERSION,
  WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION,
  buildWindowsMvpReleaseBundleArtifactIntegrityReport,
  buildWindowsMvpReleaseBundleSmokeReport,
  exitCodeForWindowsMvpReleaseBundleArtifactIntegrity,
  exitCodeForWindowsMvpReleaseBundleSmoke,
  parseWindowsMvpReleaseBundleSmokeArgs,
} from "../scripts/smoke-windows-mvp-release-bundle.mjs";
import {
  WINDOWS_MVP_RELEASE_BUNDLE_FILES,
  WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
  buildWindowsMvpReleaseBundleReport,
  exitCodeForWindowsMvpReleaseBundle,
  parseWindowsMvpReleaseBundleArgs,
  writeWindowsMvpReleaseBundleOutput,
} from "../scripts/windows-mvp-release-bundle.mjs";
import {
  DEFAULT_QA_REPORT_ARTIFACT,
  DEFAULT_SOAK_REPORT_ARTIFACT,
  WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  WINDOWS_MVP_RELEASE_WORKFLOW_INPUTS_SCHEMA_VERSION,
  buildWindowsMvpReleaseWorkflowInputsReport,
  exitCodeForWindowsMvpReleaseWorkflowInputs,
  parseWindowsMvpReleaseWorkflowInputsArgs,
  writeWindowsMvpReleaseWorkflowInputsFile,
} from "../scripts/windows-mvp-release-workflow-inputs.mjs";
import {
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS,
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES,
  buildWindowsMvpReleaseEvidenceSourcesReport,
  exitCodeForWindowsMvpReleaseEvidenceSources,
  parseWindowsMvpReleaseEvidenceSourcesArgs,
  writeWindowsMvpReleaseEvidenceSourcesDispatchInputsFile,
  writeWindowsMvpReleaseEvidenceSourcesFile,
} from "../scripts/windows-mvp-release-evidence-sources.mjs";
import {
  WINDOWS_MVP_RELEASE_RESOLVED_EVIDENCE_SCHEMA_VERSION,
  buildWindowsMvpReleaseResolvedEvidenceReport,
  exitCodeForWindowsMvpReleaseResolvedEvidence,
  parseWindowsMvpReleaseResolvedEvidenceArgs,
} from "../scripts/windows-mvp-release-resolved-evidence.mjs";

function passingQaReport() {
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

function passingSoakReport(overrides = {}) {
  return {
    schemaVersion: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
    startedAt: "2026-06-20T00:00:00.000Z",
    finishedAt: "2026-06-20T02:00:00.000Z",
    requestedDurationMs: DEFAULT_DURATION_MS,
    elapsedMs: DEFAULT_DURATION_MS,
    maxCycles: null,
    cycleDelayMs: 1000,
    scenarioTimeoutMs: 600000,
    scenarios: ["peer-session-executor"],
    completedCycles: 1,
    completedRuns: 1,
    metrics: {
      scenarioPasses: 1,
      scenarioFailures: 0,
      scenarioTimeouts: 0,
      remoteSessionStarts: 1,
      remoteSessionStops: 1,
      forwardedInputCommands: 0,
      forwardedInputOutcomes: 1,
      injectedInputEvents: 1,
      releaseAllCommands: 0,
      releaseAllOutcomes: 1,
      platformReleaseAllCalls: 1,
      sessionConnects: 1,
      sessionDisconnects: 1,
      finalPeerLeaks: 0,
      stuckInputSuspicions: 0,
    },
    qaEvidence: {
      supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
      supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
      status: "pass",
      blockers: [],
    },
    failures: [],
    ...overrides,
  };
}

function passingSigningPreflight() {
  return evaluateSigningPreflight({
    TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
    AKRAZ_WINDOWS_SIGNING_CERT_BASE64: "super-secret-cert-data",
    AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
  });
}

function passingUpdaterConfigPreflight() {
  return evaluateUpdaterConfigPreflight({
    bundle: {
      createUpdaterArtifacts: true,
    },
    plugins: {
      updater: {
        pubkey: "akraz-public-updater-key-content",
        endpoints: ["https://updates.example.com/{{target}}/{{arch}}/{{current_version}}"],
        windows: {
          installMode: "passive",
        },
      },
    },
  });
}

function writeJson(path, payload) {
  writeFileSync(path, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
  return path;
}

function runAppPackageScript(scriptName, args) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Windows MVP release gate", () => {
  test("accepts complete sanitized release evidence", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-"));
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");

    try {
      const report = buildWindowsMvpReleaseGateReport({
        qaReportFile: writeJson(qaReportFile, passingQaReport()),
        signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
        soakReportFile: writeJson(soakReportFile, passingSoakReport()),
        updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
      });
      const formatted = JSON.stringify(report);

      expect(report.schemaVersion).toBe(WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION);
      expect(report.ready).toBe(true);
      expect(report.requiredSoakDurationMs).toBe(DEFAULT_DURATION_MS);
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(report.nextActions).toEqual([]);
      expect(report.privacy).toEqual({
        includesQaReportPayload: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesEndpointValues: false,
      });
      expect(formatted).not.toContain(qaReportFile);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain("updates.example.com");
      expect(exitCodeForWindowsMvpReleaseGate(report)).toBe(0);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects missing evidence file arguments with actionable next steps", () => {
    const report = buildWindowsMvpReleaseGateReport();

    expect(report.ready).toBe(false);
    expect(report.checks.find((check) => check.id === "qaReport")).toMatchObject({
      status: "missing",
      detail: "fileArgumentMissing",
    });
    expect(report.nextActions).toContainEqual({
      gate: "qaReport",
      action: "provide --qa-report-file with a release evidence JSON file",
    });
    expect(exitCodeForWindowsMvpReleaseGate(report)).toBe(1);
  });

  test("rejects smoke-length soak evidence for release gating", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-short-soak-"));
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");

    try {
      const report = buildWindowsMvpReleaseGateReport({
        qaReportFile: writeJson(qaReportFile, passingQaReport()),
        signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
        soakReportFile: writeJson(
          soakReportFile,
          passingSoakReport({
            finishedAt: "2026-06-20T00:00:01.000Z",
            requestedDurationMs: 1,
            elapsedMs: 1,
          }),
        ),
        updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
      });

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "soakReport")).toMatchObject({
        status: "invalid",
        detail: "durationBelowReleaseMinimum",
        requiredSoakDurationMs: DEFAULT_DURATION_MS,
        requestedDurationMs: 1,
        elapsedMs: 1,
      });
      expect(report.nextActions).toContainEqual({
        gate: "soakReport",
        action: "run the Windows MVP soak for the full release minimum duration",
      });
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects incomplete or hand-edited soak QA evidence", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-soak-qa-evidence-"));
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const baseOptions = {
      qaReportFile: writeJson(qaReportFile, passingQaReport()),
      signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
      updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
    };

    try {
      const missingQaEvidenceReport = buildWindowsMvpReleaseGateReport({
        ...baseOptions,
        soakReportFile: writeJson(
          soakReportFile,
          passingSoakReport({
            qaEvidence: undefined,
          }),
        ),
      });

      expect(missingQaEvidenceReport.ready).toBe(false);
      expect(
        missingQaEvidenceReport.checks.find((check) => check.id === "soakReport"),
      ).toMatchObject({
        status: "invalid",
        detail: "soakQaEvidenceMissing",
        expectedQaEvidence: {
          status: "pass",
          supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
        },
      });
      expect(missingQaEvidenceReport.nextActions).toContainEqual({
        gate: "soakReport",
        action: "regenerate the Windows MVP soak report with QA evidence fields",
      });

      const driftedQaEvidenceReport = buildWindowsMvpReleaseGateReport({
        ...baseOptions,
        soakReportFile: writeJson(
          soakReportFile,
          passingSoakReport({
            qaEvidence: {
              supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
              supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
              status: "pass",
              blockers: ["remoteInputMissing"],
            },
          }),
        ),
      });

      expect(driftedQaEvidenceReport.ready).toBe(false);
      expect(
        driftedQaEvidenceReport.checks.find((check) => check.id === "soakReport"),
      ).toMatchObject({
        status: "invalid",
        detail: "soakQaEvidenceDrift",
        qaEvidence: {
          status: "pass",
          blockers: ["remoteInputMissing"],
        },
        expectedQaEvidence: {
          status: "pass",
          blockers: [],
        },
      });

      const insufficientQaEvidenceReport = buildWindowsMvpReleaseGateReport({
        ...baseOptions,
        soakReportFile: writeJson(
          soakReportFile,
          passingSoakReport({
            metrics: {
              ...passingSoakReport().metrics,
              remoteSessionStarts: 0,
              remoteSessionStops: 0,
            },
            qaEvidence: {
              supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
              supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
              status: "insufficient",
              blockers: ["remoteSessionStartMissing", "remoteSessionStopMissing"],
            },
          }),
        ),
      });

      expect(insufficientQaEvidenceReport.ready).toBe(false);
      expect(
        insufficientQaEvidenceReport.checks.find((check) => check.id === "soakReport"),
      ).toMatchObject({
        status: "invalid",
        detail: "soakQaEvidenceNotPassing",
        qaEvidence: {
          status: "insufficient",
          blockers: ["remoteSessionStartMissing", "remoteSessionStopMissing"],
        },
      });
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("parses arguments and writes a gate report atomically", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-write-"));
    const outputFile = join(tempDir, "nested", "gate.json");

    try {
      expect(
        parseWindowsMvpReleaseGateArgs([
          "--qa-report-file",
          "qa.json",
          "--soak-report-file",
          "soak.json",
          "--signing-preflight-file",
          "signing.json",
          "--updater-config-preflight-file",
          "updater.json",
          "--out-file",
          outputFile,
        ]),
      ).toEqual({
        outFile: outputFile,
        qaReportFile: "qa.json",
        signingPreflightFile: "signing.json",
        soakReportFile: "soak.json",
        updaterConfigPreflightFile: "updater.json",
      });

      const written = writeWindowsMvpReleaseGateOutputFile(outputFile, {
        schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
      });

      expect(written).toBe(outputFile);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
      expect(JSON.parse(readFileSync(outputFile, "utf8"))).toEqual({
        schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
      });
      expect(() => parseWindowsMvpReleaseGateArgs(["--qa-report-file"])).toThrow(
        "--qa-report-file requires a non-empty value",
      );
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("writes release gate report through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-cli-"));
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const outputFile = join(tempDir, "nested", "gate.json");

    try {
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());
      writeJson(signingFile, passingSigningPreflight());
      writeJson(updaterFile, passingUpdaterConfigPreflight());

      const result = runAppPackageScript("release:windows-mvp-gate", [
        "--qa-report-file",
        qaReportFile,
        "--soak-report-file",
        soakReportFile,
        "--signing-preflight-file",
        signingFile,
        "--updater-config-preflight-file",
        updaterFile,
        "--out-file",
        outputFile,
      ]);

      expect(result.status).toBe(0);
      expect(existsSync(outputFile)).toBe(true);

      const report = JSON.parse(result.stdout);
      const writtenReport = JSON.parse(readFileSync(outputFile, "utf8"));
      const formatted = JSON.stringify(report);

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
        releaseTarget: "Windows MVP alpha",
        ready: true,
        requiredSoakDurationMs: DEFAULT_DURATION_MS,
        privacy: {
          includesQaReportPayload: false,
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesEndpointValues: false,
        },
      });
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(report.nextActions).toEqual([]);
      expect(writtenReport).toEqual(report);
      expect(result.stdout.endsWith("\n")).toBe(true);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
      expect(formatted).not.toContain(tempDir);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain("updates.example.com");
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects stale evidence schemas without leaking the input file path", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-gate-schema-"));
    const qaReportFile = join(tempDir, "qa-report.json");

    try {
      const report = buildWindowsMvpReleaseGateReport({
        qaReportFile: writeJson(qaReportFile, {
          ...buildWindowsMvpQaReportTemplate(),
          schemaVersion: "akraz.windowsMvpQaReport/v0",
        }),
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "qaReport")).toMatchObject({
        source: "qaReportFile",
        status: "invalid",
        detail: "schemaVersionMismatch",
      });
      expect(formatted).not.toContain(qaReportFile);
      expect(formatted).toContain(SIGNING_PREFLIGHT_SCHEMA_VERSION);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("smoke gate verifies fail-closed behavior without release evidence", () => {
    const releaseGateReport = buildWindowsMvpReleaseGateReport();
    const smokeReport = buildWindowsMvpReleaseGateSmokeReport(releaseGateReport);

    expect(smokeReport.schemaVersion).toBe(WINDOWS_MVP_RELEASE_GATE_SMOKE_SCHEMA_VERSION);
    expect(smokeReport.ready).toBe(true);
    expect(smokeReport.releaseGateReady).toBe(false);
    expect(smokeReport.releaseGateExitCode).toBe(1);
    expect(smokeReport.checkedGateIds).toContain("qaReport");
    expect(smokeReport.checkedGateIds).toContain("soakReport");
    expect(smokeReport.checks.every((check) => check.status === "pass")).toBe(true);
    expect(smokeReport.privacy).toEqual({
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
    });
    expect(exitCodeForWindowsMvpReleaseGateSmoke(smokeReport)).toBe(0);
  });

  test("smoke gate verifies fail-closed behavior through the app package script", () => {
    const result = runAppPackageScript("smoke:windows-mvp-release-gate", []);

    expect(result.status).toBe(0);

    const smokeReport = JSON.parse(result.stdout);

    expect(smokeReport.schemaVersion).toBe(WINDOWS_MVP_RELEASE_GATE_SMOKE_SCHEMA_VERSION);
    expect(smokeReport.ready).toBe(true);
    expect(smokeReport.releaseGateReady).toBe(false);
    expect(smokeReport.releaseGateExitCode).toBe(1);
    expect(smokeReport.checkedGateIds).toContain("qaReport");
    expect(smokeReport.checkedGateIds).toContain("soakReport");
    expect(smokeReport.checks.every((check) => check.status === "pass")).toBe(true);
    expect(smokeReport.privacy).toEqual({
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
    });
    expect(result.stdout.endsWith("\n")).toBe(true);
  });

  test("smoke bundle verifies fail-closed behavior without release evidence", () => {
    const releaseBundleReport = buildWindowsMvpReleaseBundleReport();
    const smokeReport = buildWindowsMvpReleaseBundleSmokeReport(releaseBundleReport);

    expect(smokeReport.schemaVersion).toBe(WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION);
    expect(smokeReport.ready).toBe(true);
    expect(smokeReport.releaseBundleReady).toBe(false);
    expect(smokeReport.releaseBundleExitCode).toBe(1);
    expect(smokeReport.checkedArtifactIds).toContain("qaReport");
    expect(smokeReport.checkedArtifactIds).toContain("soakReport");
    expect(smokeReport.checkedArtifactIds).toContain("signingPreflight");
    expect(smokeReport.checkedArtifactIds).toContain("updaterConfigPreflight");
    expect(smokeReport.checkedArtifactIds).toContain("evidenceSources");
    expect(smokeReport.checks.every((check) => check.status === "pass")).toBe(true);
    expect(smokeReport.privacy).toEqual({
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
      includesSourceEvidencePaths: false,
    });
    expect(exitCodeForWindowsMvpReleaseBundleSmoke(smokeReport)).toBe(0);
  });

  test("smoke bundle verifies fail-closed behavior through the app package script", () => {
    const result = runAppPackageScript("smoke:windows-mvp-release-bundle", []);

    expect(result.status).toBe(0);

    const smokeReport = JSON.parse(result.stdout);

    expect(smokeReport.schemaVersion).toBe(WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION);
    expect(smokeReport.ready).toBe(true);
    expect(smokeReport.releaseBundleReady).toBe(false);
    expect(smokeReport.releaseBundleExitCode).toBe(1);
    expect(smokeReport.checkedArtifactIds).toContain("qaReport");
    expect(smokeReport.checkedArtifactIds).toContain("soakReport");
    expect(smokeReport.checkedArtifactIds).toContain("signingPreflight");
    expect(smokeReport.checkedArtifactIds).toContain("updaterConfigPreflight");
    expect(smokeReport.checkedArtifactIds).toContain("evidenceSources");
    expect(smokeReport.checks.every((check) => check.status === "pass")).toBe(true);
    expect(smokeReport.privacy).toEqual({
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
      includesSourceEvidencePaths: false,
    });
    expect(result.stdout.endsWith("\n")).toBe(true);
  });

  test("builds a privacy-safe Windows MVP release bundle manifest and canonical files", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-"));
    const outDir = join(tempDir, "bundle");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");
    const options = {
      evidenceSourcesFile: writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      ),
      qaReportFile: writeJson(qaReportFile, passingQaReport()),
      signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
      soakReportFile: writeJson(soakReportFile, passingSoakReport()),
      updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
    };

    try {
      const gateReport = buildWindowsMvpReleaseGateReport(options);
      const bundleReport = buildWindowsMvpReleaseBundleReport(options);
      const formatted = JSON.stringify(bundleReport);
      const written = writeWindowsMvpReleaseBundleOutput(outDir, options, bundleReport, gateReport);
      const manifestPath = join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest);
      const gatePath = join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate);

      expect(bundleReport.schemaVersion).toBe(WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION);
      expect(bundleReport.ready).toBe(true);
      expect(bundleReport.releaseGateReady).toBe(true);
      expect(bundleReport.artifacts.every((artifact) => artifact.included)).toBe(true);
      expect(bundleReport.artifacts.find((artifact) => artifact.id === "soakReport")).toMatchObject(
        {
          status: "pass",
          qaEvidence: {
            supportedCaseIds: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS,
            supportedCaseCount: WINDOWS_MVP_SOAK_QA_EVIDENCE_CASE_IDS.length,
            status: "pass",
            blockers: [],
          },
        },
      );
      expect(bundleReport.privacy).toEqual({
        includesQaReportPayload: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesEndpointValues: false,
        includesSourceEvidencePaths: false,
      });
      expect(formatted).not.toContain(tempDir);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain("updates.example.com");
      expect(written.files.length).toBe(7);
      expect(JSON.parse(readFileSync(manifestPath, "utf8"))).toEqual(bundleReport);
      expect(JSON.parse(readFileSync(gatePath, "utf8"))).toEqual(gateReport);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.soakReport))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.signingPreflight))).toBe(
        true,
      );
      expect(
        existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.updaterConfigPreflight)),
      ).toBe(true);
      const integrityReport = buildWindowsMvpReleaseBundleArtifactIntegrityReport(outDir);
      expect(integrityReport).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_ARTIFACT_INTEGRITY_SCHEMA_VERSION,
        ready: true,
        bundleDirectoryProvided: true,
        expectedFiles: [
          "windows-mvp-qa-report.json",
          "windows-mvp-release-bundle.json",
          "windows-mvp-release-evidence-sources.json",
          "windows-mvp-release-gate.json",
          "windows-mvp-signing-preflight.json",
          "windows-mvp-soak-report.json",
          "windows-mvp-updater-config-preflight.json",
        ],
        foundFiles: [
          "windows-mvp-qa-report.json",
          "windows-mvp-release-bundle.json",
          "windows-mvp-release-evidence-sources.json",
          "windows-mvp-release-gate.json",
          "windows-mvp-signing-preflight.json",
          "windows-mvp-soak-report.json",
          "windows-mvp-updater-config-preflight.json",
        ],
      });
      expect(integrityReport.checks.every((check) => check.status === "pass")).toBe(true);
      expect(integrityReport.privacy).toEqual({
        includesQaReportPayload: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesEndpointValues: false,
        includesSourceEvidencePaths: false,
      });
      expect(exitCodeForWindowsMvpReleaseBundle(bundleReport)).toBe(0);
      expect(exitCodeForWindowsMvpReleaseBundleArtifactIntegrity(integrityReport)).toBe(0);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("writes release bundle files through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-cli-"));
    const outDir = join(tempDir, "bundle");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");

    try {
      writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      );
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());
      writeJson(signingFile, passingSigningPreflight());
      writeJson(updaterFile, passingUpdaterConfigPreflight());

      const result = runAppPackageScript("release:windows-mvp-bundle", [
        "--evidence-sources-file",
        evidenceSourcesFile,
        "--qa-report-file",
        qaReportFile,
        "--soak-report-file",
        soakReportFile,
        "--signing-preflight-file",
        signingFile,
        "--updater-config-preflight-file",
        updaterFile,
        "--out-dir",
        outDir,
      ]);

      expect(result.status).toBe(0);

      const report = JSON.parse(result.stdout);
      const manifestPath = join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest);
      const gatePath = join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate);
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
      const gateReport = JSON.parse(readFileSync(gatePath, "utf8"));
      const formatted = JSON.stringify(report);

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
        releaseTarget: "Windows MVP alpha",
        ready: true,
        releaseGateReady: true,
        bundleFiles: {
          manifest: WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest,
          releaseGate: WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate,
          evidenceSources: WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources,
        },
        privacy: {
          includesQaReportPayload: false,
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesEndpointValues: false,
          includesSourceEvidencePaths: false,
        },
      });
      expect(report.artifacts.every((artifact) => artifact.included)).toBe(true);
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(report.nextActions).toEqual([]);
      expect(manifest).toEqual(report);
      expect(gateReport).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
        ready: true,
      });
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.soakReport))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.signingPreflight))).toBe(
        true,
      );
      expect(
        existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.updaterConfigPreflight)),
      ).toBe(true);

      const integrityReport = buildWindowsMvpReleaseBundleArtifactIntegrityReport(outDir);
      expect(integrityReport.ready).toBe(true);
      expect(integrityReport.checks.every((check) => check.status === "pass")).toBe(true);
      expect(result.stdout.endsWith("\n")).toBe(true);
      expect(readFileSync(manifestPath, "utf8").endsWith("\n")).toBe(true);
      expect(readFileSync(gatePath, "utf8").endsWith("\n")).toBe(true);
      expect(formatted).not.toContain(tempDir);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain("updates.example.com");
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("smoke bundle verifies generated bundle artifacts through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-smoke-cli-"));
    const outDir = join(tempDir, "bundle");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");
    const options = {
      evidenceSourcesFile: writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      ),
      qaReportFile: writeJson(qaReportFile, passingQaReport()),
      signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
      soakReportFile: writeJson(soakReportFile, passingSoakReport()),
      updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
    };

    try {
      const gateReport = buildWindowsMvpReleaseGateReport(options);
      const bundleReport = buildWindowsMvpReleaseBundleReport(options);
      writeWindowsMvpReleaseBundleOutput(outDir, options, bundleReport, gateReport);

      const result = runAppPackageScript("smoke:windows-mvp-release-bundle", [
        "--bundle-dir",
        outDir,
      ]);

      expect(result.status).toBe(0);

      const integrityReport = JSON.parse(result.stdout);
      const formatted = JSON.stringify(integrityReport);

      expect(integrityReport).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_ARTIFACT_INTEGRITY_SCHEMA_VERSION,
        ready: true,
        bundleDirectoryProvided: true,
        expectedFiles: [
          "windows-mvp-qa-report.json",
          "windows-mvp-release-bundle.json",
          "windows-mvp-release-evidence-sources.json",
          "windows-mvp-release-gate.json",
          "windows-mvp-signing-preflight.json",
          "windows-mvp-soak-report.json",
          "windows-mvp-updater-config-preflight.json",
        ],
        foundFiles: [
          "windows-mvp-qa-report.json",
          "windows-mvp-release-bundle.json",
          "windows-mvp-release-evidence-sources.json",
          "windows-mvp-release-gate.json",
          "windows-mvp-signing-preflight.json",
          "windows-mvp-soak-report.json",
          "windows-mvp-updater-config-preflight.json",
        ],
      });
      expect(integrityReport.checks.every((check) => check.status === "pass")).toBe(true);
      expect(integrityReport.privacy).toEqual({
        includesQaReportPayload: false,
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesEndpointValues: false,
        includesSourceEvidencePaths: false,
      });
      expect(result.stdout.endsWith("\n")).toBe(true);
      expect(formatted).not.toContain(tempDir);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain("updates.example.com");
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects a release bundle directory with missing or stale canonical artifacts", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-integrity-"));
    const outDir = join(tempDir, "bundle");

    try {
      mkdirSync(outDir, { recursive: true });
      writeJson(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest), {
        schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
        ready: true,
        releaseGateReady: true,
        artifacts: [
          {
            id: "releaseGate",
            included: true,
          },
        ],
      });
      writeJson(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate), {
        schemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
        ready: false,
      });
      writeJson(join(outDir, "unexpected.json"), {
        schemaVersion: "unexpected/v1",
      });

      const report = buildWindowsMvpReleaseBundleArtifactIntegrityReport(outDir);
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "canonicalBundleFiles")).toMatchObject({
        status: "invalid",
        detail: "canonicalBundleFileSetMismatch",
        missingFiles: [
          "windows-mvp-qa-report.json",
          "windows-mvp-release-evidence-sources.json",
          "windows-mvp-signing-preflight.json",
          "windows-mvp-soak-report.json",
          "windows-mvp-updater-config-preflight.json",
        ],
        extraFiles: ["unexpected.json"],
      });
      expect(report.checks.find((check) => check.id === "bundleArtifactSchemas")).toMatchObject({
        status: "invalid",
        detail: "bundleArtifactSchemaMismatch",
      });
      expect(report.checks.find((check) => check.id === "releaseGateArtifactReady")).toMatchObject({
        status: "invalid",
        detail: "releaseGateArtifactNotReady",
        ready: false,
      });
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseBundleArtifactIntegrity(report)).toBe(1);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects a release bundle manifest with stale artifact file names", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-manifest-"));
    const outDir = join(tempDir, "bundle");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, "soak-report.json");
    const signingFile = join(tempDir, "signing.json");
    const updaterFile = join(tempDir, "updater.json");
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");
    const options = {
      evidenceSourcesFile: writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      ),
      qaReportFile: writeJson(qaReportFile, passingQaReport()),
      signingPreflightFile: writeJson(signingFile, passingSigningPreflight()),
      soakReportFile: writeJson(soakReportFile, passingSoakReport()),
      updaterConfigPreflightFile: writeJson(updaterFile, passingUpdaterConfigPreflight()),
    };

    try {
      const gateReport = buildWindowsMvpReleaseGateReport(options);
      const bundleReport = buildWindowsMvpReleaseBundleReport(options);
      writeWindowsMvpReleaseBundleOutput(outDir, options, bundleReport, gateReport);

      const manifestPath = join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest);
      const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
      const qaArtifact = manifest.artifacts.find((artifact) => artifact.id === "qaReport");
      qaArtifact.fileName = "qa-report.json";
      writeJson(manifestPath, manifest);

      const report = buildWindowsMvpReleaseBundleArtifactIntegrityReport(outDir);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "bundleManifestReady")).toMatchObject({
        status: "invalid",
        detail: "bundleManifestNotReady",
        ready: true,
        releaseGateReady: true,
        missingArtifactIds: [],
        unexpectedArtifactIds: [],
        mismatchedFileNames: [
          {
            artifactId: "qaReport",
            expectedFileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport,
            actualFileName: "qa-report.json",
          },
        ],
      });
      expect(report.checks.find((check) => check.id === "canonicalBundleFiles")).toMatchObject({
        status: "pass",
      });
      expect(exitCodeForWindowsMvpReleaseBundleArtifactIntegrity(report)).toBe(1);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects incomplete release bundle inputs without copying missing evidence", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-missing-"));
    const outDir = join(tempDir, "bundle");

    try {
      const report = buildWindowsMvpReleaseBundleReport();
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "bundleEvidenceFiles")).toMatchObject({
        status: "invalid",
        detail: "bundleEvidenceFilesNotReady",
        missingArtifactIds: [
          "qaReport",
          "soakReport",
          "signingPreflight",
          "updaterConfigPreflight",
        ],
      });
      expect(report.nextActions).toContainEqual({
        gate: "releaseBundle",
        artifactId: "qaReport",
        action: "provide qaReport evidence file before bundling",
      });
      expect(report.checks.find((check) => check.id === "optionalEvidenceFiles")).toMatchObject({
        status: "pass",
      });
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseBundle(report)).toBe(1);
      expect(() => writeWindowsMvpReleaseBundleOutput(undefined, {}, report, {})).toThrow(
        "--out-dir is required",
      );
      writeWindowsMvpReleaseBundleOutput(outDir, {}, report, buildWindowsMvpReleaseGateReport());
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest))).toBe(true);
      expect(existsSync(join(outDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport))).toBe(false);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("parses release bundle arguments", () => {
    expect(
      parseWindowsMvpReleaseBundleArgs([
        "--evidence-sources-file",
        "evidence-sources.json",
        "--qa-report-file",
        "qa.json",
        "--soak-report-file",
        "soak.json",
        "--signing-preflight-file",
        "signing.json",
        "--updater-config-preflight-file",
        "updater.json",
        "--out-dir",
        "bundle",
      ]),
    ).toEqual({
      outDir: "bundle",
      evidenceSourcesFile: "evidence-sources.json",
      qaReportFile: "qa.json",
      signingPreflightFile: "signing.json",
      soakReportFile: "soak.json",
      updaterConfigPreflightFile: "updater.json",
    });
    expect(() => parseWindowsMvpReleaseBundleArgs(["--out-dir"])).toThrow(
      "--out-dir requires a non-empty value",
    );
    expect(() => parseWindowsMvpReleaseBundleArgs(["--unknown"])).toThrow(
      "unknown Windows MVP release bundle argument: --unknown",
    );
    expect(parseWindowsMvpReleaseBundleSmokeArgs(["--bundle-dir", "release-bundle"])).toEqual({
      bundleDir: "release-bundle",
    });
    expect(() => parseWindowsMvpReleaseBundleSmokeArgs(["--bundle-dir"])).toThrow(
      "--bundle-dir requires a non-empty value",
    );
    expect(() => parseWindowsMvpReleaseBundleSmokeArgs(["--unknown"])).toThrow(
      "unknown Windows MVP release bundle smoke argument: --unknown",
    );
  });

  test("rejects invalid optional release evidence source manifest before copying it", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-sources-invalid-"));
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");

    try {
      const report = buildWindowsMvpReleaseBundleReport({
        evidenceSourcesFile: writeJson(evidenceSourcesFile, {
          schemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
          ready: false,
          privacy: {
            includesSecretValues: false,
            includesFullFilePaths: false,
            includesArtifactPayloads: false,
          },
        }),
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "optionalEvidenceFiles")).toMatchObject({
        status: "invalid",
        detail: "optionalEvidenceFilesNotReady",
        invalidArtifactIds: ["evidenceSources"],
      });
      expect(report.artifacts.find((artifact) => artifact.id === "evidenceSources")).toMatchObject({
        status: "invalid",
        detail: "evidenceSourcesNotReady",
        included: false,
        required: false,
      });
      expect(report.nextActions).toContainEqual({
        gate: "releaseBundle",
        artifactId: "evidenceSources",
        action: "regenerate passing evidenceSources evidence before bundling",
      });
      expect(formatted).not.toContain(evidenceSourcesFile);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects release evidence source bundle mapping drift before copying it", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-bundle-sources-drift-"));
    const evidenceSourcesFile = join(tempDir, "evidence-sources.json");

    try {
      const evidenceSources = buildWindowsMvpReleaseEvidenceSourcesReport({
        sourceRunId: "27856073522",
        manifestWritten: true,
        dispatchInputsWritten: true,
      });
      const driftedEvidenceSources = structuredClone(evidenceSources);
      const qaSource = driftedEvidenceSources.sources.find((source) => source.id === "qaReport");
      if (qaSource) {
        qaSource.bundle.fileName = "wrong-qa-report.json";
      }
      const report = buildWindowsMvpReleaseBundleReport({
        evidenceSourcesFile: writeJson(evidenceSourcesFile, driftedEvidenceSources),
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "optionalEvidenceFiles")).toMatchObject({
        status: "invalid",
        detail: "optionalEvidenceFilesNotReady",
        invalidArtifactIds: ["evidenceSources"],
      });
      expect(report.artifacts.find((artifact) => artifact.id === "evidenceSources")).toMatchObject({
        status: "invalid",
        detail: "evidenceSourceBundleMappingDrift",
        invalidSourceIds: ["qaReport"],
        included: false,
      });
      expect(formatted).not.toContain(evidenceSourcesFile);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("builds release workflow dispatch inputs from one shared evidence run", () => {
    const report = buildWindowsMvpReleaseWorkflowInputsReport({
      sourceRunId: "27856073522",
      dispatchInputsWritten: true,
    });

    expect(report).toMatchObject({
      schemaVersion: WINDOWS_MVP_RELEASE_WORKFLOW_INPUTS_SCHEMA_VERSION,
      ready: true,
      workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
      dispatchInputsWritten: true,
      inputs: {
        source_run_id: "27856073522",
        qa_source_run_id: "",
        soak_source_run_id: "",
        qa_report_artifact: DEFAULT_QA_REPORT_ARTIFACT,
        soak_report_artifact: DEFAULT_SOAK_REPORT_ARTIFACT,
      },
      resolvedRunIds: {
        qa: "27856073522",
        soak: "27856073522",
      },
      privacy: {
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesArtifactPayloads: false,
      },
    });
    expect(report.checks.find((check) => check.id === "sourceRunIds")).toMatchObject({
      status: "pass",
      mode: "shared",
    });
    expect(report.nextActions).toEqual([]);
    expect(exitCodeForWindowsMvpReleaseWorkflowInputs(report)).toBe(0);
  });

  test("builds release workflow dispatch inputs from separate QA and soak evidence runs", () => {
    const report = buildWindowsMvpReleaseWorkflowInputsReport({
      qaSourceRunId: "27855770983",
      soakSourceRunId: "27855465732",
      qaReportArtifact: "custom-qa-report",
      soakReportArtifact: "custom-soak-report",
    });

    expect(report.ready).toBe(true);
    expect(report.inputs).toEqual({
      source_run_id: "",
      qa_source_run_id: "27855770983",
      soak_source_run_id: "27855465732",
      qa_report_artifact: "custom-qa-report",
      soak_report_artifact: "custom-soak-report",
    });
    expect(report.resolvedRunIds).toEqual({
      qa: "27855770983",
      soak: "27855465732",
    });
    expect(report.checks.find((check) => check.id === "sourceRunIds")).toMatchObject({
      status: "pass",
      mode: "dedicated",
    });
  });

  test("rejects incomplete or malformed release workflow dispatch inputs", () => {
    const missingRunIds = buildWindowsMvpReleaseWorkflowInputsReport({
      qaSourceRunId: "27855770983",
    });
    const invalidRunIds = buildWindowsMvpReleaseWorkflowInputsReport({
      sourceRunId: "run-27856073522",
    });
    const invalidArtifact = buildWindowsMvpReleaseWorkflowInputsReport({
      sourceRunId: "27856073522",
      qaReportArtifact: "release-evidence/qa.json",
    });

    expect(missingRunIds.ready).toBe(false);
    expect(missingRunIds.checks.find((check) => check.id === "sourceRunIds")).toMatchObject({
      status: "missing",
      detail: "sourceRunIdsMissing",
    });
    expect(missingRunIds.nextActions).toContainEqual({
      id: "provideSourceRunIds",
      action: "provide --source-run-id or provide both --qa-source-run-id and --soak-source-run-id",
    });
    expect(invalidRunIds.checks.find((check) => check.id === "sourceRunIds")).toMatchObject({
      status: "invalid",
      detail: "runIdsMustBePositiveIntegers",
      invalidInputs: ["source_run_id"],
    });
    expect(invalidArtifact.checks.find((check) => check.id === "qaReportArtifact")).toMatchObject({
      status: "invalid",
      detail: "artifactNameContainsPathOrControlCharacter",
    });
    expect(exitCodeForWindowsMvpReleaseWorkflowInputs(missingRunIds)).toBe(1);
  });

  test("parses and writes release workflow dispatch inputs", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-workflow-inputs-"));
    const outputFile = join(tempDir, "nested", "release-workflow-inputs.json");

    try {
      expect(
        parseWindowsMvpReleaseWorkflowInputsArgs([
          "--qa-source-run-id",
          "27855770983",
          "--soak-source-run-id",
          "27855465732",
          "--qa-report-artifact",
          "windows-mvp-qa-report",
          "--soak-report-artifact",
          "windows-mvp-soak-report",
          "--out-file",
          outputFile,
        ]),
      ).toEqual({
        sourceRunId: undefined,
        qaSourceRunId: "27855770983",
        soakSourceRunId: "27855465732",
        qaReportArtifact: "windows-mvp-qa-report",
        soakReportArtifact: "windows-mvp-soak-report",
        outFile: outputFile,
      });

      const inputs = buildWindowsMvpReleaseWorkflowInputsReport({
        sourceRunId: "27856073522",
      }).inputs;
      expect(writeWindowsMvpReleaseWorkflowInputsFile(outputFile, inputs)).toBe(outputFile);
      expect(JSON.parse(readFileSync(outputFile, "utf8"))).toEqual(inputs);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
      expect(() =>
        parseWindowsMvpReleaseWorkflowInputsArgs(["--source-run-id", "27856073522"]),
      ).toThrow("--out-file is required");
      expect(() => parseWindowsMvpReleaseWorkflowInputsArgs(["--unknown"])).toThrow(
        "unknown Windows MVP release workflow input argument: --unknown",
      );
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("writes release workflow dispatch inputs through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-workflow-inputs-cli-"));
    const outputFile = join(tempDir, "nested", "release-workflow-inputs.json");

    try {
      const result = runAppPackageScript("release:windows-mvp-workflow-inputs", [
        "--qa-source-run-id",
        "27855770983",
        "--soak-source-run-id",
        "27855465732",
        "--qa-report-artifact",
        "windows-mvp-qa-report",
        "--soak-report-artifact",
        "windows-mvp-soak-report",
        "--out-file",
        outputFile,
      ]);

      expect(result.status).toBe(0);

      const report = JSON.parse(result.stdout);
      const writtenInputs = JSON.parse(readFileSync(outputFile, "utf8"));

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_WORKFLOW_INPUTS_SCHEMA_VERSION,
        ready: true,
        workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
        dispatchInputsWritten: true,
        resolvedRunIds: {
          qa: "27855770983",
          soak: "27855465732",
        },
        privacy: {
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesArtifactPayloads: false,
        },
      });
      expect(report.inputs).toEqual({
        source_run_id: "",
        qa_source_run_id: "27855770983",
        soak_source_run_id: "27855465732",
        qa_report_artifact: "windows-mvp-qa-report",
        soak_report_artifact: "windows-mvp-soak-report",
      });
      expect(writtenInputs).toEqual(report.inputs);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("builds a privacy-safe release evidence source manifest from one shared run", () => {
    const report = buildWindowsMvpReleaseEvidenceSourcesReport({
      sourceRunId: "27856073522",
      manifestWritten: true,
      dispatchInputsWritten: true,
    });

    expect(report).toMatchObject({
      schemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
      ready: true,
      workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
      manifestWritten: true,
      dispatchInputsWritten: true,
      sources: [
        {
          id: "qaReport",
          sourceRunId: "27856073522",
          artifactName: DEFAULT_QA_REPORT_ARTIFACT,
          expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
          bundle: {
            artifactId: "qaReport",
            releaseGateCheckId: "qaReport",
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
          },
        },
        {
          id: "soakReport",
          sourceRunId: "27856073522",
          artifactName: DEFAULT_SOAK_REPORT_ARTIFACT,
          expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
          bundle: {
            artifactId: "soakReport",
            releaseGateCheckId: "soakReport",
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
          },
        },
      ],
      dispatchInputs: {
        source_run_id: "27856073522",
        qa_source_run_id: "",
        soak_source_run_id: "",
        qa_report_artifact: DEFAULT_QA_REPORT_ARTIFACT,
        soak_report_artifact: DEFAULT_SOAK_REPORT_ARTIFACT,
      },
      privacy: {
        includesSecretValues: false,
        includesFullFilePaths: false,
        includesArtifactPayloads: false,
      },
    });
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(report.nextActions).toEqual([]);
    expect(exitCodeForWindowsMvpReleaseEvidenceSources(report)).toBe(0);
  });

  test("accepts resolved release evidence files with canonical manifest filenames", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-resolved-evidence-"));
    const evidenceSourcesFile = join(tempDir, "windows-mvp-release-evidence-sources.json");
    const qaReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport);
    const soakReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport);

    try {
      writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      );
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());

      const report = buildWindowsMvpReleaseResolvedEvidenceReport({
        evidenceSourcesFile,
        qaReportFile,
        soakReportFile,
      });
      const formatted = JSON.stringify(report);

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_RESOLVED_EVIDENCE_SCHEMA_VERSION,
        ready: true,
        evidenceSourcesFileProvided: true,
        resolvedFiles: [
          {
            id: "qaReport",
            fileProvided: true,
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
            expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
            status: "pass",
          },
          {
            id: "soakReport",
            fileProvided: true,
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
            expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
            status: "pass",
          },
        ],
        privacy: {
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesArtifactPayloads: false,
        },
      });
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(report.nextActions).toEqual([]);
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseResolvedEvidence(report)).toBe(0);
      expect(
        parseWindowsMvpReleaseResolvedEvidenceArgs([
          "--evidence-sources-file",
          "sources.json",
          "--qa-report-file",
          "windows-mvp-qa-report.json",
          "--soak-report-file",
          "windows-mvp-soak-report.json",
        ]),
      ).toEqual({
        evidenceSourcesFile: "sources.json",
        qaReportFile: "windows-mvp-qa-report.json",
        soakReportFile: "windows-mvp-soak-report.json",
      });
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("verifies resolved release evidence through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-resolved-evidence-cli-"));
    const evidenceSourcesFile = join(tempDir, "windows-mvp-release-evidence-sources.json");
    const qaReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport);
    const soakReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport);

    try {
      writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      );
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());

      const result = runAppPackageScript("release:windows-mvp-resolved-evidence", [
        "--evidence-sources-file",
        evidenceSourcesFile,
        "--qa-report-file",
        qaReportFile,
        "--soak-report-file",
        soakReportFile,
      ]);

      expect(result.status).toBe(0);

      const report = JSON.parse(result.stdout);
      const formatted = JSON.stringify(report);

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_RESOLVED_EVIDENCE_SCHEMA_VERSION,
        ready: true,
        evidenceSourcesFileProvided: true,
        resolvedFiles: [
          {
            id: "qaReport",
            sourceId: "qaReport",
            fileProvided: true,
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
            expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
            status: "pass",
          },
          {
            id: "soakReport",
            sourceId: "soakReport",
            fileProvided: true,
            fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
            expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
            status: "pass",
          },
        ],
        privacy: {
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesArtifactPayloads: false,
        },
      });
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(report.nextActions).toEqual([]);
      expect(formatted).not.toContain(tempDir);
      expect(result.stdout.endsWith("\n")).toBe(true);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects resolved release evidence filename drift before bundling", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-resolved-evidence-drift-"));
    const evidenceSourcesFile = join(tempDir, "windows-mvp-release-evidence-sources.json");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport);

    try {
      writeJson(
        evidenceSourcesFile,
        buildWindowsMvpReleaseEvidenceSourcesReport({
          sourceRunId: "27856073522",
          manifestWritten: true,
          dispatchInputsWritten: true,
        }),
      );
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());

      const report = buildWindowsMvpReleaseResolvedEvidenceReport({
        evidenceSourcesFile,
        qaReportFile,
        soakReportFile,
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.resolvedFiles.find((file) => file.id === "qaReport")).toMatchObject({
        status: "invalid",
        detail: "fileNameMismatch",
        fileName: "qa-report.json",
        expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
      });
      expect(report.checks.find((check) => check.id === "resolvedEvidenceFileNames")).toMatchObject(
        {
          status: "invalid",
          detail: "resolvedEvidenceFileNamesNotReady",
          missingFileIds: [],
          invalidFileIds: ["qaReport"],
        },
      );
      expect(report.nextActions).toContainEqual({
        id: "resolveQaReportFile",
        evidenceSourceId: "qaReport",
        action: "download windows-mvp-qa-report.json as the expected release evidence JSON",
        detail: "fileNameMismatch",
      });
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseResolvedEvidence(report)).toBe(1);
      expect(() => parseWindowsMvpReleaseResolvedEvidenceArgs(["--qa-report-file"])).toThrow(
        "--qa-report-file requires a non-empty value",
      );
      expect(() =>
        parseWindowsMvpReleaseResolvedEvidenceArgs([
          "--qa-report-file",
          "windows-mvp-qa-report.json",
          "--soak-report-file",
          "windows-mvp-soak-report.json",
        ]),
      ).toThrow("--evidence-sources-file is required");
      expect(() => parseWindowsMvpReleaseResolvedEvidenceArgs(["--unknown"])).toThrow(
        "unknown Windows MVP release resolved evidence argument: --unknown",
      );
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects resolved release evidence source filename drift before bundling", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-resolved-source-drift-"));
    const evidenceSourcesFile = join(tempDir, "windows-mvp-release-evidence-sources.json");
    const qaReportFile = join(tempDir, "qa-report.json");
    const soakReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport);

    try {
      const evidenceSources = buildWindowsMvpReleaseEvidenceSourcesReport({
        sourceRunId: "27856073522",
        manifestWritten: true,
        dispatchInputsWritten: true,
      });
      const qaSource = evidenceSources.sources.find((source) => source.id === "qaReport");
      qaSource.expectedFileName = "qa-report.json";
      writeJson(evidenceSourcesFile, evidenceSources);
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());

      const report = buildWindowsMvpReleaseResolvedEvidenceReport({
        evidenceSourcesFile,
        qaReportFile,
        soakReportFile,
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.resolvedFiles.find((file) => file.id === "qaReport")).toMatchObject({
        status: "invalid",
        detail: "expectedFileNameDrift",
        fileName: "qa-report.json",
        expectedFileName: "qa-report.json",
        canonicalExpectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
      });
      expect(report.checks.find((check) => check.id === "resolvedEvidenceFileNames")).toMatchObject(
        {
          status: "invalid",
          detail: "resolvedEvidenceFileNamesNotReady",
          missingFileIds: [],
          invalidFileIds: ["qaReport"],
        },
      );
      expect(report.nextActions).toContainEqual({
        id: "regenerateQaReportEvidenceSource",
        evidenceSourceId: "qaReport",
        action: "regenerate the Windows MVP release evidence sources manifest",
        detail: "expectedFileNameDrift",
      });
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseResolvedEvidence(report)).toBe(1);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects resolved release evidence source bundle mapping drift before bundling", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-resolved-bundle-drift-"));
    const evidenceSourcesFile = join(tempDir, "windows-mvp-release-evidence-sources.json");
    const qaReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport);
    const soakReportFile = join(tempDir, WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport);

    try {
      const evidenceSources = buildWindowsMvpReleaseEvidenceSourcesReport({
        sourceRunId: "27856073522",
        manifestWritten: true,
        dispatchInputsWritten: true,
      });
      const qaSource = evidenceSources.sources.find((source) => source.id === "qaReport");
      qaSource.bundle = {
        ...qaSource.bundle,
        artifactId: "qaEvidence",
      };
      writeJson(evidenceSourcesFile, evidenceSources);
      writeJson(qaReportFile, passingQaReport());
      writeJson(soakReportFile, passingSoakReport());

      const report = buildWindowsMvpReleaseResolvedEvidenceReport({
        evidenceSourcesFile,
        qaReportFile,
        soakReportFile,
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(false);
      expect(report.resolvedFiles.find((file) => file.id === "qaReport")).toMatchObject({
        status: "invalid",
        detail: "bundleMappingDrift",
        fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
        expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
        canonicalExpectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
        bundle: {
          artifactId: "qaEvidence",
          releaseGateCheckId: "qaReport",
          fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
        },
        canonicalBundle: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS.qaReport,
      });
      expect(report.checks.find((check) => check.id === "resolvedEvidenceFileNames")).toMatchObject(
        {
          status: "invalid",
          detail: "resolvedEvidenceFileNamesNotReady",
          missingFileIds: [],
          invalidFileIds: ["qaReport"],
        },
      );
      expect(report.nextActions).toContainEqual({
        id: "regenerateQaReportEvidenceSource",
        evidenceSourceId: "qaReport",
        action: "regenerate the Windows MVP release evidence sources manifest",
        detail: "bundleMappingDrift",
      });
      expect(formatted).not.toContain(tempDir);
      expect(exitCodeForWindowsMvpReleaseResolvedEvidence(report)).toBe(1);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("writes release evidence sources and dispatch inputs from dedicated runs", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-evidence-sources-"));
    const manifestFile = join(tempDir, "nested", "release-evidence-sources.json");
    const dispatchInputsFile = join(tempDir, "nested", "release-workflow-inputs.json");

    try {
      expect(
        parseWindowsMvpReleaseEvidenceSourcesArgs([
          "--qa-source-run-id",
          "27855770983",
          "--soak-source-run-id",
          "27855465732",
          "--qa-report-artifact",
          "custom-qa-report",
          "--soak-report-artifact",
          "custom-soak-report",
          "--out-file",
          manifestFile,
          "--dispatch-inputs-file",
          dispatchInputsFile,
        ]),
      ).toEqual({
        sourceRunId: undefined,
        qaSourceRunId: "27855770983",
        soakSourceRunId: "27855465732",
        qaReportArtifact: "custom-qa-report",
        soakReportArtifact: "custom-soak-report",
        outFile: manifestFile,
        dispatchInputsFile,
      });

      const report = buildWindowsMvpReleaseEvidenceSourcesReport({
        qaSourceRunId: "27855770983",
        soakSourceRunId: "27855465732",
        qaReportArtifact: "custom-qa-report",
        soakReportArtifact: "custom-soak-report",
      });

      expect(report.ready).toBe(true);
      expect(report.sources.map((source) => source.sourceRunId)).toEqual([
        "27855770983",
        "27855465732",
      ]);
      expect(writeWindowsMvpReleaseEvidenceSourcesFile(manifestFile, report)).toBe(manifestFile);
      expect(
        writeWindowsMvpReleaseEvidenceSourcesDispatchInputsFile(
          dispatchInputsFile,
          report.dispatchInputs,
        ),
      ).toBe(dispatchInputsFile);
      expect(JSON.parse(readFileSync(manifestFile, "utf8"))).toEqual(report);
      expect(JSON.parse(readFileSync(dispatchInputsFile, "utf8"))).toEqual(report.dispatchInputs);
      expect(readFileSync(manifestFile, "utf8").endsWith("\n")).toBe(true);
      expect(readFileSync(dispatchInputsFile, "utf8").endsWith("\n")).toBe(true);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("writes release evidence sources through the app package script", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-release-evidence-sources-cli-"));
    const manifestFile = join(tempDir, "nested", "release-evidence-sources.json");
    const dispatchInputsFile = join(tempDir, "nested", "release-workflow-inputs.json");

    try {
      const result = runAppPackageScript("release:windows-mvp-evidence-sources", [
        "--qa-source-run-id",
        "27855770983",
        "--soak-source-run-id",
        "27855465732",
        "--qa-report-artifact",
        "custom-qa-report",
        "--soak-report-artifact",
        "custom-soak-report",
        "--out-file",
        manifestFile,
        "--dispatch-inputs-file",
        dispatchInputsFile,
      ]);

      expect(result.status).toBe(0);

      const report = JSON.parse(result.stdout);
      const manifest = JSON.parse(readFileSync(manifestFile, "utf8"));
      const dispatchInputs = JSON.parse(readFileSync(dispatchInputsFile, "utf8"));

      expect(report).toMatchObject({
        schemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
        ready: true,
        workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
        manifestWritten: true,
        dispatchInputsWritten: true,
        privacy: {
          includesSecretValues: false,
          includesFullFilePaths: false,
          includesArtifactPayloads: false,
        },
      });
      expect(report.sources.map((source) => source.sourceRunId)).toEqual([
        "27855770983",
        "27855465732",
      ]);
      expect(report.sources.map((source) => source.artifactName)).toEqual([
        "custom-qa-report",
        "custom-soak-report",
      ]);
      expect(report.sources.map((source) => source.expectedFileName)).toEqual([
        WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
        WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
      ]);
      expect(manifest).toEqual(report);
      expect(dispatchInputs).toEqual(report.dispatchInputs);
      expect(readFileSync(manifestFile, "utf8").endsWith("\n")).toBe(true);
      expect(readFileSync(dispatchInputsFile, "utf8").endsWith("\n")).toBe(true);
    } finally {
      rmSync(tempDir, { force: true, recursive: true });
    }
  });

  test("rejects malformed release evidence source inputs before writing dispatch inputs", () => {
    const missingRunIds = buildWindowsMvpReleaseEvidenceSourcesReport({
      qaSourceRunId: "27855770983",
    });
    const invalidArtifact = buildWindowsMvpReleaseEvidenceSourcesReport({
      sourceRunId: "27856073522",
      soakReportArtifact: "release-evidence\\soak.json",
    });

    expect(missingRunIds.ready).toBe(false);
    expect(missingRunIds.checks.find((check) => check.id === "workflowInputs")).toMatchObject({
      status: "invalid",
      detail: "releaseWorkflowInputsNotReady",
      failingChecks: ["sourceRunIds"],
    });
    expect(missingRunIds.checks.find((check) => check.id === "evidenceSources")).toMatchObject({
      status: "missing",
      detail: "evidenceSourcesMissing",
      missingSourceIds: ["soakReport"],
    });
    expect(missingRunIds.nextActions).toContainEqual({
      id: "provideEvidenceSources",
      action: "provide source run IDs and artifact names for every release evidence source",
      missingSourceIds: ["soakReport"],
    });
    expect(invalidArtifact.ready).toBe(false);
    expect(invalidArtifact.nextActions).toContainEqual({
      id: "sanitizeArtifactName",
      action: "use an artifact name, not a file path",
      checkId: "soakReportArtifact",
    });
    expect(exitCodeForWindowsMvpReleaseEvidenceSources(missingRunIds)).toBe(1);
    expect(() => parseWindowsMvpReleaseEvidenceSourcesArgs([])).toThrow(
      "at least one of --out-file or --dispatch-inputs-file is required",
    );
    expect(() => parseWindowsMvpReleaseEvidenceSourcesArgs(["--source-run-id"])).toThrow(
      "--source-run-id requires a non-empty value",
    );
    expect(() => parseWindowsMvpReleaseEvidenceSourcesArgs(["--unknown"])).toThrow(
      "unknown Windows MVP release evidence sources argument: --unknown",
    );
  });
});
