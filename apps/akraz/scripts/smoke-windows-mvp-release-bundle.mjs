import {
  WINDOWS_MVP_RELEASE_BUNDLE_FILES,
  WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
  buildWindowsMvpReleaseBundleReport,
  exitCodeForWindowsMvpReleaseBundle,
} from "./windows-mvp-release-bundle.mjs";
import { WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION } from "./windows-mvp-release-gate.mjs";
import {
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS,
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
} from "./windows-mvp-release-evidence-sources.mjs";
import { WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION } from "./windows-mvp-qa-report.mjs";
import { WINDOWS_MVP_SOAK_SCHEMA_VERSION } from "./windows-mvp-soak-report.mjs";
import { SIGNING_PREFLIGHT_SCHEMA_VERSION } from "./smoke-signing-preflight.mjs";
import { UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION } from "./smoke-updater-config-preflight.mjs";

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

export const WINDOWS_MVP_RELEASE_BUNDLE_SMOKE_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseBundleSmoke/v1";
export const WINDOWS_MVP_RELEASE_BUNDLE_ARTIFACT_INTEGRITY_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseBundleArtifactIntegrity/v1";

const EXPECTED_BUNDLE_FILES = Object.values(WINDOWS_MVP_RELEASE_BUNDLE_FILES).toSorted();
const EXPECTED_ARTIFACT_SCHEMAS = {
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest]: WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate]: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources]:
    WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport]: WINDOWS_MVP_QA_REPORT_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.soakReport]: WINDOWS_MVP_SOAK_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.signingPreflight]: SIGNING_PREFLIGHT_SCHEMA_VERSION,
  [WINDOWS_MVP_RELEASE_BUNDLE_FILES.updaterConfigPreflight]:
    UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION,
};
const EXPECTED_MANIFEST_ARTIFACT_FILES = {
  releaseGate: WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate,
  qaReport: WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport,
  soakReport: WINDOWS_MVP_RELEASE_BUNDLE_FILES.soakReport,
  signingPreflight: WINDOWS_MVP_RELEASE_BUNDLE_FILES.signingPreflight,
  updaterConfigPreflight: WINDOWS_MVP_RELEASE_BUNDLE_FILES.updaterConfigPreflight,
  evidenceSources: WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources,
};
const EXPECTED_EVIDENCE_SOURCE_BUNDLE_MAPPINGS = Object.entries(
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS,
).map(([id, bundle]) => Object.assign({ id }, bundle));
const PRIVACY_FLAG_NAMES = [
  "includesQaReportPayload",
  "includesSecretValues",
  "includesFullFilePaths",
  "includesEndpointValues",
  "includesSourceEvidencePaths",
  "includesArtifactPayloads",
];
const SECRET_STRING_PATTERNS = [
  {
    id: "secretSentinel",
    pattern: /super-secret/i,
  },
  {
    id: "privateKeyPem",
    pattern: /-----BEGIN (?:[A-Z0-9]+ )?PRIVATE KEY-----/i,
  },
  {
    id: "githubToken",
    pattern: /\bgh[pousr]_[A-Za-z0-9_]{20,}\b/,
  },
  {
    id: "awsAccessKeyId",
    pattern: /\bAKIA[0-9A-Z]{16}\b/,
  },
  {
    id: "genericSecretToken",
    pattern: /\bsk-[A-Za-z0-9_-]{20,}\b/,
  },
];
const SENSITIVE_FIELD_NAME_PATTERN =
  /secret|password|private[-_]?key|token|certificate[-_]?base64|cert[-_]?base64|pfx|p12/i;

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

export function buildWindowsMvpReleaseBundleArtifactIntegrityReport(bundleDir) {
  const directoryRead = readBundleDirectory(bundleDir);
  const artifacts = Object.fromEntries(
    EXPECTED_BUNDLE_FILES.map((fileName) => [fileName, readBundleArtifact(bundleDir, fileName)]),
  );
  const checks = [
    evaluateBundleDirectory(directoryRead),
    evaluateCanonicalFileSet(directoryRead),
    evaluateArtifactSchemas(artifacts),
    evaluateBundleManifest(artifacts[WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest]?.payload),
    evaluateReleaseGateArtifact(artifacts[WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate]?.payload),
    evaluateEvidenceSourcesArtifact(
      artifacts[WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources]?.payload,
    ),
    evaluateArtifactPrivacy(artifacts),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_ARTIFACT_INTEGRITY_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    bundleDirectoryProvided: typeof bundleDir === "string" && bundleDir.trim().length > 0,
    expectedFiles: EXPECTED_BUNDLE_FILES,
    foundFiles: directoryRead.fileNames,
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

export function parseWindowsMvpReleaseBundleSmokeArgs(args) {
  const options = {
    bundleDir: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--bundle-dir":
        options.bundleDir = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release bundle smoke argument: ${arg}`);
    }
  }

  return options;
}

export function exitCodeForWindowsMvpReleaseBundleArtifactIntegrity(report) {
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

function readBundleDirectory(bundleDir) {
  if (typeof bundleDir !== "string" || bundleDir.trim().length === 0) {
    return {
      ok: false,
      detail: "bundleDirMissing",
      fileNames: [],
    };
  }

  try {
    return {
      ok: true,
      fileNames: readdirSync(bundleDir)
        .filter((fileName) => fileName.endsWith(".json"))
        .toSorted(),
    };
  } catch {
    return {
      ok: false,
      detail: "bundleDirUnreadable",
      fileNames: [],
    };
  }
}

function readBundleArtifact(bundleDir, fileName) {
  if (typeof bundleDir !== "string" || bundleDir.trim().length === 0) {
    return {
      ok: false,
      detail: "bundleDirMissing",
      payload: undefined,
    };
  }

  try {
    return {
      ok: true,
      payload: JSON.parse(readFileSync(join(bundleDir, fileName), "utf8")),
    };
  } catch {
    return {
      ok: false,
      detail: "artifactUnreadableJson",
      payload: undefined,
    };
  }
}

function evaluateBundleDirectory(directoryRead) {
  if (directoryRead.ok) {
    return {
      id: "bundleDirectoryReadable",
      status: "pass",
    };
  }

  return {
    id: "bundleDirectoryReadable",
    status: "invalid",
    detail: directoryRead.detail,
  };
}

function evaluateCanonicalFileSet(directoryRead) {
  const missingFiles = EXPECTED_BUNDLE_FILES.filter(
    (fileName) => !directoryRead.fileNames.includes(fileName),
  );
  const extraFiles = directoryRead.fileNames.filter(
    (fileName) => !EXPECTED_BUNDLE_FILES.includes(fileName),
  );

  if (directoryRead.ok && missingFiles.length === 0 && extraFiles.length === 0) {
    return {
      id: "canonicalBundleFiles",
      status: "pass",
    };
  }

  return {
    id: "canonicalBundleFiles",
    status: "invalid",
    detail: "canonicalBundleFileSetMismatch",
    missingFiles,
    extraFiles,
  };
}

function evaluateArtifactSchemas(artifacts) {
  const invalidFiles = Object.entries(EXPECTED_ARTIFACT_SCHEMAS)
    .filter(([fileName, expectedSchemaVersion]) => {
      const artifact = artifacts[fileName];
      return !artifact?.ok || artifact.payload?.schemaVersion !== expectedSchemaVersion;
    })
    .map(([fileName]) => fileName);

  if (invalidFiles.length === 0) {
    return {
      id: "bundleArtifactSchemas",
      status: "pass",
    };
  }

  return {
    id: "bundleArtifactSchemas",
    status: "invalid",
    detail: "bundleArtifactSchemaMismatch",
    invalidFiles,
  };
}

function evaluateBundleManifest(manifest) {
  const expectedIncludedIds = [
    "releaseGate",
    "qaReport",
    "soakReport",
    "signingPreflight",
    "updaterConfigPreflight",
    "evidenceSources",
  ];
  const includedArtifacts = Array.isArray(manifest?.artifacts)
    ? manifest.artifacts.filter((artifact) => artifact?.included === true)
    : [];
  const includedArtifactIds = includedArtifacts.map((artifact) => artifact.id).toSorted();
  const missingArtifactIds = expectedIncludedIds.filter(
    (artifactId) => !includedArtifactIds.includes(artifactId),
  );
  const unexpectedArtifactIds = includedArtifactIds.filter(
    (artifactId) => !expectedIncludedIds.includes(artifactId),
  );
  const mismatchedFileNames = includedArtifacts
    .filter(
      (artifact) =>
        Object.hasOwn(EXPECTED_MANIFEST_ARTIFACT_FILES, artifact.id) &&
        EXPECTED_MANIFEST_ARTIFACT_FILES[artifact.id] !== artifact.fileName,
    )
    .map((artifact) => ({
      artifactId: artifact.id,
      expectedFileName: EXPECTED_MANIFEST_ARTIFACT_FILES[artifact.id],
      actualFileName: typeof artifact.fileName === "string" ? artifact.fileName : null,
    }));

  if (
    manifest?.schemaVersion === WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION &&
    manifest?.ready === true &&
    manifest?.releaseGateReady === true &&
    missingArtifactIds.length === 0 &&
    unexpectedArtifactIds.length === 0 &&
    mismatchedFileNames.length === 0
  ) {
    return {
      id: "bundleManifestReady",
      status: "pass",
    };
  }

  return {
    id: "bundleManifestReady",
    status: "invalid",
    detail: "bundleManifestNotReady",
    ready: manifest?.ready ?? null,
    releaseGateReady: manifest?.releaseGateReady ?? null,
    missingArtifactIds,
    unexpectedArtifactIds,
    mismatchedFileNames,
  };
}

function evaluateReleaseGateArtifact(releaseGate) {
  if (
    releaseGate?.schemaVersion === WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION &&
    releaseGate?.ready === true
  ) {
    return {
      id: "releaseGateArtifactReady",
      status: "pass",
    };
  }

  return {
    id: "releaseGateArtifactReady",
    status: "invalid",
    detail: "releaseGateArtifactNotReady",
    ready: releaseGate?.ready ?? null,
  };
}

function evaluateEvidenceSourcesArtifact(evidenceSources) {
  if (evidenceSources?.schemaVersion !== WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION) {
    return {
      id: "evidenceSourcesArtifactReady",
      status: "invalid",
      detail: "evidenceSourcesArtifactNotReady",
      ready: evidenceSources?.ready ?? null,
    };
  }

  if (evidenceSources?.ready !== true) {
    return {
      id: "evidenceSourcesArtifactReady",
      status: "invalid",
      detail: "evidenceSourcesArtifactNotReady",
      ready: evidenceSources?.ready ?? null,
    };
  }

  const sourceBundleMapping = evaluateEvidenceSourcesBundleMapping(evidenceSources);
  if (sourceBundleMapping.status === "pass") {
    return {
      id: "evidenceSourcesArtifactReady",
      status: "pass",
    };
  }

  return {
    id: "evidenceSourcesArtifactReady",
    status: "invalid",
    detail: sourceBundleMapping.detail,
    invalidSourceIds: sourceBundleMapping.invalidSourceIds,
  };
}

function evaluateArtifactPrivacy(artifacts) {
  const findings = Object.entries(artifacts).flatMap(([fileName, artifact]) => {
    if (!artifact.ok) {
      return [];
    }

    const failures = artifactPrivacyFailures(artifact.payload);
    if (failures.length === 0) {
      return [];
    }

    return [
      {
        fileName,
        ...mergePrivacyFailures(failures),
      },
    ];
  });
  const invalidFiles = findings.map((finding) => finding.fileName);

  if (invalidFiles.length === 0) {
    return {
      id: "bundleArtifactPrivacy",
      status: "pass",
    };
  }

  return {
    id: "bundleArtifactPrivacy",
    status: "invalid",
    detail: "bundleArtifactPrivacyNotReady",
    invalidFiles,
    findings,
  };
}

function evaluateEvidenceSourcesBundleMapping(evidenceSources) {
  const sources = Array.isArray(evidenceSources?.sources) ? evidenceSources.sources : [];
  const invalidSourceIds = EXPECTED_EVIDENCE_SOURCE_BUNDLE_MAPPINGS.filter((expected) => {
    const source = sources.find((candidate) => candidate?.id === expected.id);
    return (
      !source ||
      source.expectedFileName !== expected.fileName ||
      source.bundle?.artifactId !== expected.artifactId ||
      source.bundle?.releaseGateCheckId !== expected.releaseGateCheckId ||
      source.bundle?.fileName !== expected.fileName
    );
  }).map((expected) => expected.id);

  if (invalidSourceIds.length === 0) {
    return {
      status: "pass",
    };
  }

  return {
    status: "invalid",
    detail: "evidenceSourceBundleMappingDrift",
    invalidSourceIds,
  };
}

function artifactPrivacyFailures(payload) {
  return [
    ...privacyFlagFailures(payload?.privacy),
    ...secretPatternFailures(payload),
    ...sensitiveFieldFailures(payload),
  ];
}

function privacyFlagFailures(privacy) {
  if (typeof privacy !== "object" || privacy === null || Array.isArray(privacy)) {
    return [];
  }

  const invalidFlags = PRIVACY_FLAG_NAMES.filter(
    (flagName) => Object.hasOwn(privacy, flagName) && privacy[flagName] !== false,
  );
  if (invalidFlags.length === 0) {
    return [];
  }

  return [
    {
      reason: "privacyFlagsNotReady",
      invalidFlags,
    },
  ];
}

function secretPatternFailures(payload) {
  const patternIds = [...new Set(secretPatternIds(payload))].toSorted();
  if (patternIds.length === 0) {
    return [];
  }

  return [
    {
      reason: "secretPatternDetected",
      secretPatterns: patternIds,
    },
  ];
}

function secretPatternIds(value) {
  if (typeof value === "string") {
    return SECRET_STRING_PATTERNS.filter((entry) => entry.pattern.test(value)).map(
      (entry) => entry.id,
    );
  }

  if (Array.isArray(value)) {
    return value.flatMap((item) => secretPatternIds(item));
  }

  if (typeof value === "object" && value !== null) {
    return Object.values(value).flatMap((item) => secretPatternIds(item));
  }

  return [];
}

function sensitiveFieldFailures(payload) {
  const sensitiveFields = sensitiveFieldPaths(payload).toSorted();
  if (sensitiveFields.length === 0) {
    return [];
  }

  return [
    {
      reason: "sensitiveFieldValue",
      sensitiveFields,
    },
  ];
}

function sensitiveFieldPaths(value, path = []) {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) => sensitiveFieldPaths(item, [...path, String(index)]));
  }

  if (typeof value !== "object" || value === null) {
    return [];
  }

  return Object.entries(value).flatMap(([key, child]) => {
    const childPath = [...path, key];
    if (
      typeof child === "string" &&
      SENSITIVE_FIELD_NAME_PATTERN.test(key) &&
      looksLikeSensitiveFieldValue(child)
    ) {
      return [childPath.join(".")];
    }

    return sensitiveFieldPaths(child, childPath);
  });
}

function looksLikeSensitiveFieldValue(value) {
  const trimmed = value.trim();
  return (
    trimmed.length >= 8 &&
    !/^(missing|present|redacted|available|unavailable|configured|notConfigured)$/i.test(trimmed)
  );
}

function mergePrivacyFailures(failures) {
  return {
    reasons: [...new Set(failures.map((failure) => failure.reason))].toSorted(),
    invalidFlags: [
      ...new Set(failures.flatMap((failure) => failure.invalidFlags ?? [])),
    ].toSorted(),
    secretPatterns: [
      ...new Set(failures.flatMap((failure) => failure.secretPatterns ?? [])),
    ].toSorted(),
    sensitiveFields: [
      ...new Set(failures.flatMap((failure) => failure.sensitiveFields ?? [])),
    ].toSorted(),
  };
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseBundleSmokeArgs(process.argv.slice(2));
  const report =
    options.bundleDir === undefined
      ? buildWindowsMvpReleaseBundleSmokeReport()
      : buildWindowsMvpReleaseBundleArtifactIntegrityReport(options.bundleDir);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(
    options.bundleDir === undefined
      ? exitCodeForWindowsMvpReleaseBundleSmoke(report)
      : exitCodeForWindowsMvpReleaseBundleArtifactIntegrity(report),
  );
}
