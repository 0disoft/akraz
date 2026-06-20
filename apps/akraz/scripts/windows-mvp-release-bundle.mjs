import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
  buildWindowsMvpReleaseGateReport,
  exitCodeForWindowsMvpReleaseGate,
} from "./windows-mvp-release-gate.mjs";
import {
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS,
  WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
} from "./windows-mvp-release-evidence-sources.mjs";

export const WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION = "akraz.windowsMvpReleaseBundle/v1";

export const WINDOWS_MVP_RELEASE_BUNDLE_FILES = {
  manifest: "windows-mvp-release-bundle.json",
  releaseGate: "windows-mvp-release-gate.json",
  evidenceSources: "windows-mvp-release-evidence-sources.json",
  qaReport: "windows-mvp-qa-report.json",
  soakReport: "windows-mvp-soak-report.json",
  signingPreflight: "windows-mvp-signing-preflight.json",
  updaterConfigPreflight: "windows-mvp-updater-config-preflight.json",
};

const REQUIRED_EVIDENCE_OPTIONS = [
  {
    id: "qaReport",
    optionKey: "qaReportFile",
    source: "qaReportFile",
    fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.qaReport,
  },
  {
    id: "soakReport",
    optionKey: "soakReportFile",
    source: "soakReportFile",
    fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.soakReport,
  },
  {
    id: "signingPreflight",
    optionKey: "signingPreflightFile",
    source: "signingPreflightFile",
    fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.signingPreflight,
  },
  {
    id: "updaterConfigPreflight",
    optionKey: "updaterConfigPreflightFile",
    source: "updaterConfigPreflightFile",
    fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.updaterConfigPreflight,
  },
];

const OPTIONAL_EVIDENCE_OPTIONS = [
  {
    id: "evidenceSources",
    optionKey: "evidenceSourcesFile",
    source: "evidenceSourcesFile",
    fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources,
    expectedSchemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
    expectedSourceBundleMappings: buildExpectedSourceBundleMappings(),
  },
];

function buildExpectedSourceBundleMappings() {
  return Object.entries(WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_BUNDLE_MAPPINGS).map(([id, bundle]) =>
    Object.assign({ id }, bundle),
  );
}

export function buildWindowsMvpReleaseBundleReport(
  options = {},
  workspaceRoot = currentWorkspaceRoot(),
) {
  const releaseGate = buildWindowsMvpReleaseGateReport(options, workspaceRoot);
  const evidenceArtifacts = REQUIRED_EVIDENCE_OPTIONS.map((definition) =>
    buildEvidenceArtifact(definition, options, releaseGate),
  );
  const optionalArtifacts = OPTIONAL_EVIDENCE_OPTIONS.map((definition) =>
    buildOptionalEvidenceArtifact(definition, options),
  );
  const checks = [
    evaluateReleaseGateSchema(releaseGate),
    evaluateReleaseGateReady(releaseGate),
    evaluateBundleEvidenceFiles(evidenceArtifacts),
    evaluateOptionalEvidenceFiles(optionalArtifacts),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_BUNDLE_SCHEMA_VERSION,
    releaseTarget: releaseGate.releaseTarget,
    ready: checks.every((check) => check.status === "pass"),
    bundleFiles: {
      manifest: WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest,
      releaseGate: WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate,
      evidenceSources: WINDOWS_MVP_RELEASE_BUNDLE_FILES.evidenceSources,
    },
    artifacts: [
      {
        id: "releaseGate",
        fileName: WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate,
        status: releaseGate.ready ? "pass" : "invalid",
        schemaVersion: releaseGate.schemaVersion,
        expectedSchemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
        included: true,
      },
      ...evidenceArtifacts,
      ...optionalArtifacts,
    ],
    checks,
    nextActions: buildBundleNextActions(checks, releaseGate.nextActions),
    releaseGateReady: releaseGate.ready,
    privacy: {
      includesQaReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
      includesSourceEvidencePaths: false,
    },
  };
}

export function parseWindowsMvpReleaseBundleArgs(args) {
  const options = {
    outDir: undefined,
    evidenceSourcesFile: undefined,
    qaReportFile: undefined,
    signingPreflightFile: undefined,
    soakReportFile: undefined,
    updaterConfigPreflightFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--out-dir":
        options.outDir = readValue(args, ++index, arg);
        break;
      case "--evidence-sources-file":
        options.evidenceSourcesFile = readValue(args, ++index, arg);
        break;
      case "--qa-report-file":
        options.qaReportFile = readValue(args, ++index, arg);
        break;
      case "--signing-preflight-file":
        options.signingPreflightFile = readValue(args, ++index, arg);
        break;
      case "--soak-report-file":
        options.soakReportFile = readValue(args, ++index, arg);
        break;
      case "--updater-config-preflight-file":
        options.updaterConfigPreflightFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release bundle argument: ${arg}`);
    }
  }

  return options;
}

export function exitCodeForWindowsMvpReleaseBundle(report) {
  return report.ready ? 0 : 1;
}

export function writeWindowsMvpReleaseBundleOutput(outDir, options, bundleReport, gateReport) {
  if (!outDir) {
    throw new Error("--out-dir is required");
  }

  const resolvedOutDir = resolve(outDir);
  mkdirSync(resolvedOutDir, { recursive: true });

  const writtenFiles = [
    writeJsonFileAtomic(
      join(resolvedOutDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.releaseGate),
      gateReport,
    ),
    writeJsonFileAtomic(
      join(resolvedOutDir, WINDOWS_MVP_RELEASE_BUNDLE_FILES.manifest),
      bundleReport,
    ),
  ];

  for (const artifact of bundleReport.artifacts) {
    if (!artifact.included || artifact.id === "releaseGate") {
      continue;
    }

    const definition = [...REQUIRED_EVIDENCE_OPTIONS, ...OPTIONAL_EVIDENCE_OPTIONS].find(
      (candidate) => candidate.id === artifact.id,
    );
    const inputFile = definition ? options[definition.optionKey] : undefined;
    if (inputFile) {
      writtenFiles.push(copyFileAtomic(inputFile, join(resolvedOutDir, artifact.fileName)));
    }
  }

  return {
    directory: resolvedOutDir,
    files: writtenFiles,
  };
}

function buildOptionalEvidenceArtifact(definition, options) {
  const fileProvided = typeof options[definition.optionKey] === "string";
  if (!fileProvided) {
    return {
      id: definition.id,
      source: definition.source,
      fileName: definition.fileName,
      status: "skipped",
      detail: "optionalFileArgumentMissing",
      fileProvided,
      included: false,
      required: false,
    };
  }

  const fileRead = readJsonEvidence(options[definition.optionKey]);
  const sourceBundleMapping = evaluateOptionalEvidenceSourceBundleMapping(fileRead, definition);
  const valid =
    fileRead.ok &&
    fileRead.payload?.schemaVersion === definition.expectedSchemaVersion &&
    fileRead.payload?.ready === true &&
    hasPrivacyFlags(fileRead.payload?.privacy, [
      "includesSecretValues",
      "includesFullFilePaths",
      "includesArtifactPayloads",
    ]) &&
    sourceBundleMapping.status === "pass";

  return {
    id: definition.id,
    source: definition.source,
    fileName: definition.fileName,
    status: valid ? "pass" : "invalid",
    detail: valid ? undefined : optionalEvidenceDetail(fileRead, definition, sourceBundleMapping),
    schemaVersion: fileRead.payload?.schemaVersion,
    expectedSchemaVersion: definition.expectedSchemaVersion,
    fileProvided,
    included: valid,
    required: false,
    ...(sourceBundleMapping.invalidSourceIds
      ? { invalidSourceIds: sourceBundleMapping.invalidSourceIds }
      : {}),
  };
}

function buildEvidenceArtifact(definition, options, releaseGate) {
  const check = releaseGate.checks.find((candidate) => candidate.id === definition.id);
  const fileProvided = typeof options[definition.optionKey] === "string";
  if (!fileProvided) {
    return {
      id: definition.id,
      source: definition.source,
      fileName: definition.fileName,
      status: "missing",
      detail: "fileArgumentMissing",
      fileProvided,
      included: false,
    };
  }

  const included = fileProvided && check?.status === "pass";

  const artifact = {
    id: definition.id,
    source: definition.source,
    fileName: definition.fileName,
    status: check?.status ?? "missing",
    detail: check?.detail,
    schemaVersion: check?.schemaVersion,
    expectedSchemaVersion: check?.expectedSchemaVersion,
    fileProvided,
    included,
  };

  return addEvidenceArtifactSummary(artifact, definition, check);
}

function addEvidenceArtifactSummary(artifact, definition, check) {
  if (definition.id !== "soakReport") {
    return artifact;
  }

  return {
    ...artifact,
    ...(check?.qaEvidence ? { qaEvidence: check.qaEvidence } : {}),
    ...(check?.expectedQaEvidence ? { expectedQaEvidence: check.expectedQaEvidence } : {}),
  };
}

function evaluateReleaseGateSchema(releaseGate) {
  if (releaseGate?.schemaVersion === WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION) {
    return {
      id: "releaseGateSchema",
      status: "pass",
    };
  }

  return {
    id: "releaseGateSchema",
    status: "invalid",
    expectedSchemaVersion: WINDOWS_MVP_RELEASE_GATE_SCHEMA_VERSION,
    actualSchemaVersion:
      typeof releaseGate?.schemaVersion === "string" ? releaseGate.schemaVersion : null,
  };
}

function evaluateReleaseGateReady(releaseGate) {
  if (releaseGate?.ready === true && exitCodeForWindowsMvpReleaseGate(releaseGate) === 0) {
    return {
      id: "releaseGateReady",
      status: "pass",
    };
  }

  return {
    id: "releaseGateReady",
    status: "invalid",
    detail: "releaseGateNotReady",
  };
}

function evaluateBundleEvidenceFiles(artifacts) {
  const missingArtifactIds = artifacts
    .filter((artifact) => !artifact.fileProvided)
    .map((artifact) => artifact.id);
  const excludedArtifactIds = artifacts
    .filter((artifact) => artifact.fileProvided && !artifact.included)
    .map((artifact) => artifact.id);

  if (missingArtifactIds.length === 0 && excludedArtifactIds.length === 0) {
    return {
      id: "bundleEvidenceFiles",
      status: "pass",
    };
  }

  return {
    id: "bundleEvidenceFiles",
    status: "invalid",
    detail: "bundleEvidenceFilesNotReady",
    missingArtifactIds,
    excludedArtifactIds,
  };
}

function evaluateOptionalEvidenceFiles(artifacts) {
  const invalidArtifactIds = artifacts
    .filter((artifact) => artifact.fileProvided && !artifact.included)
    .map((artifact) => artifact.id);

  if (invalidArtifactIds.length === 0) {
    return {
      id: "optionalEvidenceFiles",
      status: "pass",
    };
  }

  return {
    id: "optionalEvidenceFiles",
    status: "invalid",
    detail: "optionalEvidenceFilesNotReady",
    invalidArtifactIds,
  };
}

function buildBundleNextActions(checks, releaseGateNextActions) {
  const bundleActions = checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    switch (check.id) {
      case "bundleEvidenceFiles":
        return [
          ...check.missingArtifactIds.map((artifactId) => ({
            gate: "releaseBundle",
            artifactId,
            action: `provide ${artifactId} evidence file before bundling`,
          })),
          ...check.excludedArtifactIds.map((artifactId) => ({
            gate: "releaseBundle",
            artifactId,
            action: `regenerate passing ${artifactId} evidence before bundling`,
          })),
        ];
      case "releaseGateReady":
        return [
          {
            gate: "releaseBundle",
            action: "resolve the release gate blockers before publishing a bundle",
          },
        ];
      case "optionalEvidenceFiles":
        return check.invalidArtifactIds.map((artifactId) => ({
          gate: "releaseBundle",
          artifactId,
          action: `regenerate passing ${artifactId} evidence before bundling`,
        }));
      default:
        return [
          {
            gate: "releaseBundle",
            action: "regenerate the Windows MVP release bundle with current tooling",
          },
        ];
    }
  });

  return [...bundleActions, ...releaseGateNextActions];
}

function evaluateOptionalEvidenceSourceBundleMapping(fileRead, definition) {
  if (!definition.expectedSourceBundleMappings || !fileRead.ok) {
    return {
      status: "pass",
    };
  }

  const sources = Array.isArray(fileRead.payload?.sources) ? fileRead.payload.sources : [];
  const invalidSourceIds = definition.expectedSourceBundleMappings
    .filter((expected) => {
      const source = sources.find((candidate) => candidate?.id === expected.id);
      return (
        !source ||
        source.expectedFileName !== expected.fileName ||
        source.bundle?.artifactId !== expected.artifactId ||
        source.bundle?.releaseGateCheckId !== expected.releaseGateCheckId ||
        source.bundle?.fileName !== expected.fileName
      );
    })
    .map((expected) => expected.id);

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

function optionalEvidenceDetail(fileRead, definition, sourceBundleMapping) {
  if (!fileRead.ok) {
    return fileRead.detail;
  }

  if (fileRead.payload?.schemaVersion !== definition.expectedSchemaVersion) {
    return "schemaVersionMismatch";
  }

  if (fileRead.payload?.ready !== true) {
    return "evidenceSourcesNotReady";
  }

  if (sourceBundleMapping.status !== "pass") {
    return sourceBundleMapping.detail;
  }

  return "privacyFlagsNotReady";
}

function readJsonEvidence(path) {
  try {
    return {
      ok: true,
      payload: JSON.parse(readFileSync(path, "utf8")),
    };
  } catch {
    return {
      ok: false,
      detail: "fileNotReadableJson",
      payload: undefined,
    };
  }
}

function hasPrivacyFlags(privacy, flagNames) {
  return (
    typeof privacy === "object" &&
    privacy !== null &&
    flagNames.every((flagName) => privacy[flagName] === false)
  );
}

function writeJsonFileAtomic(outFile, payload) {
  return writeTextFileAtomic(outFile, `${JSON.stringify(payload, null, 2)}\n`);
}

function copyFileAtomic(inputFile, outFile) {
  return writeTextFileAtomic(outFile, readFileSync(inputFile, "utf8"));
}

function writeTextFileAtomic(outFile, payload) {
  const resolvedOutFile = resolve(outFile);
  const outDirectory = dirname(resolvedOutFile);
  const tempFile = resolve(
    outDirectory,
    `.${basename(resolvedOutFile)}.${process.pid}.${Date.now()}.tmp`,
  );

  mkdirSync(outDirectory, { recursive: true });

  let fileDescriptor;
  try {
    fileDescriptor = openSync(tempFile, "w", 0o600);
    writeFileSync(fileDescriptor, payload, "utf8");
    fsyncSync(fileDescriptor);
    closeSync(fileDescriptor);
    fileDescriptor = undefined;
    renameSync(tempFile, resolvedOutFile);
  } catch (error) {
    if (fileDescriptor !== undefined) {
      closeSync(fileDescriptor);
    }
    if (existsSync(tempFile)) {
      rmSync(tempFile, { force: true });
    }
    throw error;
  }

  return resolvedOutFile;
}

function currentWorkspaceRoot() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  return join(appRoot, "..", "..");
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseBundleArgs(process.argv.slice(2));
  const gateReport = buildWindowsMvpReleaseGateReport(options);
  const bundleReport = buildWindowsMvpReleaseBundleReport(options);
  if (options.outDir !== undefined) {
    writeWindowsMvpReleaseBundleOutput(options.outDir, options, bundleReport, gateReport);
  }
  process.stdout.write(`${JSON.stringify(bundleReport, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseBundle(bundleReport));
}
