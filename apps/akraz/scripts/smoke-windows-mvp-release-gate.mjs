import {
  WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
  buildWindowsMvpReleaseGateReport,
  exitCodeForWindowsMvpReleaseGate,
} from "./windows-mvp-release-gate.mjs";

export const WINDOWS_MVP_RELEASE_GATE_SMOKE_SCHEMA_VERSION = "akraz.windowsMvpReleaseGateSmoke/v1";

export function buildWindowsMvpReleaseGateSmokeReport(
  releaseGateReport = buildWindowsMvpReleaseGateReport(),
) {
  const releaseGateExitCode = exitCodeForWindowsMvpReleaseGate(releaseGateReport);
  const checks = [
    evaluateSchema(releaseGateReport),
    evaluateMissingEvidenceFailure(releaseGateReport, releaseGateExitCode),
    evaluateRequiredFileEvidence(releaseGateReport, "qaReport"),
    evaluateRequiredFileEvidence(releaseGateReport, "soakReport"),
    evaluatePrivacy(releaseGateReport),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_GATE_SMOKE_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    releaseGateReady: releaseGateReport.ready === true,
    releaseGateExitCode,
    checkedGateIds: releaseGateReport.checks?.map((check) => check.id) ?? [],
    checks,
    privacy: {
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
    },
  };
}

export function exitCodeForWindowsMvpReleaseGateSmoke(report) {
  return report.ready ? 0 : 1;
}

function evaluateSchema(report) {
  if (report?.schemaVersion !== WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION) {
    return {
      id: "releaseGateSchema",
      status: "invalid",
      expectedSchemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
      actualSchemaVersion: typeof report?.schemaVersion === "string" ? report.schemaVersion : null,
    };
  }

  return {
    id: "releaseGateSchema",
    status: "pass",
  };
}

function evaluateMissingEvidenceFailure(report, exitCode) {
  if (report?.ready === false && exitCode === 1) {
    return {
      id: "releaseGateFailsWithoutEvidence",
      status: "pass",
    };
  }

  return {
    id: "releaseGateFailsWithoutEvidence",
    status: "invalid",
    detail: "releaseGateMustFailClosedWithoutEvidence",
    releaseGateReady: report?.ready === true,
    releaseGateExitCode: exitCode,
  };
}

function evaluateRequiredFileEvidence(report, gateId) {
  const check = report?.checks?.find((candidate) => candidate?.id === gateId);

  if (check?.status === "missing" && check?.detail === "fileArgumentMissing") {
    return {
      id: `${gateId}FileRequired`,
      status: "pass",
    };
  }

  return {
    id: `${gateId}FileRequired`,
    status: "invalid",
    detail: "releaseEvidenceFileMustBeRequired",
    actualStatus: check?.status ?? null,
    actualDetail: check?.detail ?? null,
  };
}

function evaluatePrivacy(report) {
  const privacy = report?.privacy;
  const invalidFlags = [
    "includesQaReportPayload",
    "includesSecretValues",
    "includesFullFilePaths",
    "includesEndpointValues",
  ].filter((flag) => privacy?.[flag] !== false);

  if (invalidFlags.length === 0) {
    return {
      id: "releaseGatePrivacy",
      status: "pass",
    };
  }

  return {
    id: "releaseGatePrivacy",
    status: "invalid",
    detail: "privacyFlagsMustBeFalse",
    flags: invalidFlags,
  };
}

if (import.meta.main) {
  const report = buildWindowsMvpReleaseGateSmokeReport();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseGateSmoke(report));
}
