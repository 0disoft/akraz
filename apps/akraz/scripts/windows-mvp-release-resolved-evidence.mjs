import { readFileSync } from "node:fs";
import { basename } from "node:path";

import { WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION } from "./windows-mvp-release-evidence-sources.mjs";

export const WINDOWS_MVP_RELEASE_RESOLVED_EVIDENCE_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseResolvedEvidence/v1";

const RESOLVED_EVIDENCE_FILES = [
  {
    id: "qaReport",
    optionKey: "qaReportFile",
    flag: "--qa-report-file",
  },
  {
    id: "soakReport",
    optionKey: "soakReportFile",
    flag: "--soak-report-file",
  },
];

export function buildWindowsMvpReleaseResolvedEvidenceReport(options = {}) {
  const evidenceSourcesRead = readJsonEvidence(options.evidenceSourcesFile);
  const evidenceSources = evidenceSourcesRead.payload;
  const resolvedFiles = RESOLVED_EVIDENCE_FILES.map((definition) =>
    buildResolvedEvidenceFile(definition, options, evidenceSources),
  );
  const checks = [
    evaluateEvidenceSourcesManifest(evidenceSourcesRead),
    evaluateResolvedEvidenceFiles(resolvedFiles),
  ];

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_RESOLVED_EVIDENCE_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    evidenceSourcesFileProvided: typeof options.evidenceSourcesFile === "string",
    resolvedFiles,
    checks,
    nextActions: buildNextActions(checks, resolvedFiles),
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesArtifactPayloads: false,
    },
  };
}

export function parseWindowsMvpReleaseResolvedEvidenceArgs(args) {
  const options = {
    evidenceSourcesFile: undefined,
    qaReportFile: undefined,
    soakReportFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--evidence-sources-file":
        options.evidenceSourcesFile = readValue(args, ++index, arg);
        break;
      case "--qa-report-file":
        options.qaReportFile = readValue(args, ++index, arg);
        break;
      case "--soak-report-file":
        options.soakReportFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release resolved evidence argument: ${arg}`);
    }
  }

  for (const requiredOption of [
    ["--evidence-sources-file", options.evidenceSourcesFile],
    ["--qa-report-file", options.qaReportFile],
    ["--soak-report-file", options.soakReportFile],
  ]) {
    if (requiredOption[1] === undefined) {
      throw new Error(`${requiredOption[0]} is required`);
    }
  }

  return options;
}

export function exitCodeForWindowsMvpReleaseResolvedEvidence(report) {
  return report.ready ? 0 : 1;
}

function buildResolvedEvidenceFile(definition, options, evidenceSources) {
  const source = findEvidenceSource(evidenceSources, definition.id);
  const filePath = options[definition.optionKey];
  const fileProvided = typeof filePath === "string";
  const actualFileName = fileProvided ? basename(filePath) : null;
  const expectedFileName =
    typeof source?.expectedFileName === "string" ? source.expectedFileName : null;
  const status = resolvedEvidenceFileStatus(fileProvided, source, actualFileName, expectedFileName);

  return {
    id: definition.id,
    sourceId: definition.id,
    fileProvided,
    fileName: actualFileName,
    expectedFileName,
    status: status.status,
    ...(status.detail ? { detail: status.detail } : {}),
  };
}

function resolvedEvidenceFileStatus(fileProvided, source, actualFileName, expectedFileName) {
  if (!fileProvided) {
    return {
      status: "missing",
      detail: "fileArgumentMissing",
    };
  }

  if (!source) {
    return {
      status: "invalid",
      detail: "evidenceSourceMissing",
    };
  }

  if (!expectedFileName || actualFileName !== expectedFileName) {
    return {
      status: "invalid",
      detail: "fileNameMismatch",
    };
  }

  return {
    status: "pass",
  };
}

function evaluateEvidenceSourcesManifest(evidenceSourcesRead) {
  if (!evidenceSourcesRead.provided) {
    return {
      id: "evidenceSourcesManifest",
      status: "missing",
      detail: "fileArgumentMissing",
    };
  }

  if (!evidenceSourcesRead.ok) {
    return {
      id: "evidenceSourcesManifest",
      status: "invalid",
      detail: evidenceSourcesRead.detail,
    };
  }

  const payload = evidenceSourcesRead.payload;
  if (payload?.schemaVersion !== WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION) {
    return {
      id: "evidenceSourcesManifest",
      status: "invalid",
      detail: "schemaVersionMismatch",
      expectedSchemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
      actualSchemaVersion:
        typeof payload?.schemaVersion === "string" ? payload.schemaVersion : null,
    };
  }

  if (payload.ready !== true) {
    return {
      id: "evidenceSourcesManifest",
      status: "invalid",
      detail: "evidenceSourcesNotReady",
    };
  }

  if (
    !hasPrivacyFlags(payload.privacy, [
      "includesSecretValues",
      "includesFullFilePaths",
      "includesArtifactPayloads",
    ])
  ) {
    return {
      id: "evidenceSourcesManifest",
      status: "invalid",
      detail: "privacyFlagsNotReady",
    };
  }

  return {
    id: "evidenceSourcesManifest",
    status: "pass",
    expectedSchemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
  };
}

function evaluateResolvedEvidenceFiles(resolvedFiles) {
  const missingFileIds = resolvedFiles
    .filter((file) => file.status === "missing")
    .map((file) => file.id);
  const invalidFileIds = resolvedFiles
    .filter((file) => file.status === "invalid")
    .map((file) => file.id);

  if (missingFileIds.length === 0 && invalidFileIds.length === 0) {
    return {
      id: "resolvedEvidenceFileNames",
      status: "pass",
    };
  }

  return {
    id: "resolvedEvidenceFileNames",
    status: "invalid",
    detail: "resolvedEvidenceFileNamesNotReady",
    missingFileIds,
    invalidFileIds,
  };
}

function buildNextActions(checks, resolvedFiles) {
  return checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    if (check.id === "evidenceSourcesManifest") {
      return [
        {
          id: "regenerateEvidenceSourcesManifest",
          action: "regenerate the Windows MVP release evidence sources manifest",
        },
      ];
    }

    if (check.id === "resolvedEvidenceFileNames") {
      return resolvedFiles
        .filter((file) => file.status !== "pass")
        .map((file) => ({
          id: `resolve${capitalize(file.id)}File`,
          evidenceSourceId: file.id,
          action: `download ${file.expectedFileName ?? file.id} as the expected release evidence JSON`,
          detail: file.detail,
        }));
    }

    return [
      {
        id: "reviewResolvedEvidence",
        action: "review the failing Windows MVP release resolved evidence check",
        checkId: check.id,
      },
    ];
  });
}

function findEvidenceSource(evidenceSources, sourceId) {
  if (!Array.isArray(evidenceSources?.sources)) {
    return undefined;
  }

  return evidenceSources.sources.find((source) => source?.id === sourceId);
}

function readJsonEvidence(path) {
  if (typeof path !== "string") {
    return {
      provided: false,
      ok: false,
      detail: "fileArgumentMissing",
      payload: undefined,
    };
  }

  try {
    return {
      provided: true,
      ok: true,
      payload: JSON.parse(readFileSync(path, "utf8")),
    };
  } catch {
    return {
      provided: true,
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

function capitalize(value) {
  return `${value.slice(0, 1).toUpperCase()}${value.slice(1)}`;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseResolvedEvidenceArgs(process.argv.slice(2));
  const report = buildWindowsMvpReleaseResolvedEvidenceReport(options);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseResolvedEvidence(report));
}
