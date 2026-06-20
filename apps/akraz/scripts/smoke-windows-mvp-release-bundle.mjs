import {
  WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
  buildWindowsMvpReleaseBundleReport,
  exitCodeForWindowsMvpReleaseBundle,
} from "./windows-mvp-release-bundle.mjs";

export const WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseBundleSmoke/v1";

export function buildWindowsMvpReleaseBundleSmokeReport(
  bundleReport = buildWindowsMvpReleaseBundleReport(),
) {
  const bundleExitCode = exitCodeForWindowsMvpReleaseBundle(bundleReport);
  const checks = [
    evaluateSchema(bundleReport),
    evaluateMissingEvidenceFailure(bundleReport, bundleExitCode),
    evaluateRequiredArtifact(bundleReport, "qaReport"),
    evaluateRequiredArtifact(bundleReport, "soakReport"),
    evaluateRequiredArtifact(bundleReport, "signingPreflight"),
    evaluateRequiredArtifact(bundleReport, "updaterConfigPreflight"),
    evaluateOptionalArtifact(bundleReport, "evidenceSources"),
    evaluatePrivacy(bundleReport),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    releaseBundleReady: bundleReport.ready === true,
    releaseBundleExitCode: bundleExitCode,
    checkedArtifactIds: bundleReport.artifacts?.map((artifact) => artifact.id) ?? [],
    checks,
    privacy: {
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
      includesSourceEvidencePaths: false,
    },
  };
}

export function exitCodeForWindowsMvpReleaseBundleSmoke(report) {
  return report.ready ? 0 : 1;
}

function evaluateSchema(report) {
  if (report?.schemaVersion !== WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION) {
    return {
      id: "releaseBundleSchema",
      status: "invalid",
      expectedSchemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
      actualSchemaVersion: typeof report?.schemaVersion === "string" ? report.schemaVersion : null,
    };
  }

  return {
    id: "releaseBundleSchema",
    status: "pass",
  };
}

function evaluateMissingEvidenceFailure(report, exitCode) {
  if (report?.ready === false && exitCode === 1) {
    return {
      id: "releaseBundleFailsWithoutEvidence",
      status: "pass",
    };
  }

  return {
    id: "releaseBundleFailsWithoutEvidence",
    status: "invalid",
    detail: "releaseBundleMustFailClosedWithoutEvidence",
    releaseBundleReady: report?.ready === true,
    releaseBundleExitCode: exitCode,
  };
}

function evaluateRequiredArtifact(report, artifactId) {
  const artifact = report?.artifacts?.find((candidate) => candidate?.id === artifactId);

  if (
    artifact?.status === "missing" &&
    artifact?.detail === "fileArgumentMissing" &&
    artifact?.included === false
  ) {
    return {
      id: `${artifactId}FileRequired`,
      status: "pass",
    };
  }

  return {
    id: `${artifactId}FileRequired`,
    status: "invalid",
    detail: "releaseBundleEvidenceFileMustBeRequired",
    actualStatus: artifact?.status ?? null,
    actualDetail: artifact?.detail ?? null,
    included: artifact?.included ?? null,
  };
}

function evaluateOptionalArtifact(report, artifactId) {
  const artifact = report?.artifacts?.find((candidate) => candidate?.id === artifactId);

  if (
    artifact?.status === "skipped" &&
    artifact?.detail === "optionalFileArgumentMissing" &&
    artifact?.included === false
  ) {
    return {
      id: `${artifactId}OptionalWhenMissing`,
      status: "pass",
    };
  }

  return {
    id: `${artifactId}OptionalWhenMissing`,
    status: "invalid",
    detail: "optionalEvidenceMustStayOptionalWhenMissing",
    actualStatus: artifact?.status ?? null,
    actualDetail: artifact?.detail ?? null,
    included: artifact?.included ?? null,
  };
}

function evaluatePrivacy(report) {
  const privacy = report?.privacy;
  const invalidFlags = [
    "includesQaReportPayload",
    "includesSecretValues",
    "includesFullFilePaths",
    "includesEndpointValues",
    "includesSourceEvidencePaths",
  ].filter((flag) => privacy?.[flag] !== false);

  if (invalidFlags.length === 0) {
    return {
      id: "releaseBundlePrivacy",
      status: "pass",
    };
  }

  return {
    id: "releaseBundlePrivacy",
    status: "invalid",
    detail: "privacyFlagsMustBeFalse",
    flags: invalidFlags,
  };
}

if (import.meta.main) {
  const report = buildWindowsMvpReleaseBundleSmokeReport();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseBundleSmoke(report));
}
