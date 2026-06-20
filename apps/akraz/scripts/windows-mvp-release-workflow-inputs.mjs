import {
  closeSync,
  existsSync,
  fsyncSync,
  mkdirSync,
  openSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, dirname, resolve } from "node:path";

export const WINDOWS_MVP_RELEASE_WORKFLOW_INPUTS_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseWorkflowInputs/v1";

export const WINDOWS_MVP_RELEASE_WORKFLOW_FILE = "windows-mvp-release.yml";
export const DEFAULT_QA_REPORT_ARTIFACT = "windows-mvp-qa-report";
export const DEFAULT_SOAK_REPORT_ARTIFACT = "windows-mvp-soak-report";
export const WINDOWS_MVP_RELEASE_WORKFLOW_INPUT_NAMES = [
  "source_run_id",
  "qa_source_run_id",
  "soak_source_run_id",
  "qa_report_artifact",
  "soak_report_artifact",
];

export function buildWindowsMvpReleaseWorkflowInputsReport(options = {}) {
  const inputs = buildWorkflowInputs(options);
  const checks = [
    evaluateRunIds(inputs),
    evaluateArtifactName("qaReportArtifact", inputs.qa_report_artifact),
    evaluateArtifactName("soakReportArtifact", inputs.soak_report_artifact),
  ];
  const ready = checks.every((check) => check.status === "pass");

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_WORKFLOW_INPUTS_SCHEMA_VERSION,
    ready,
    workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
    dispatchInputsWritten: Boolean(options.dispatchInputsWritten),
    inputs,
    resolvedRunIds: {
      qa: inputs.qa_source_run_id || inputs.source_run_id || null,
      soak: inputs.soak_source_run_id || inputs.source_run_id || null,
    },
    checks,
    nextActions: buildNextActions(checks),
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesArtifactPayloads: false,
    },
  };
}

export function parseWindowsMvpReleaseWorkflowInputsArgs(args) {
  const options = {
    sourceRunId: undefined,
    qaSourceRunId: undefined,
    soakSourceRunId: undefined,
    qaReportArtifact: DEFAULT_QA_REPORT_ARTIFACT,
    soakReportArtifact: DEFAULT_SOAK_REPORT_ARTIFACT,
    outFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--source-run-id":
        options.sourceRunId = readValue(args, ++index, arg);
        break;
      case "--qa-source-run-id":
        options.qaSourceRunId = readValue(args, ++index, arg);
        break;
      case "--soak-source-run-id":
        options.soakSourceRunId = readValue(args, ++index, arg);
        break;
      case "--qa-report-artifact":
        options.qaReportArtifact = readValue(args, ++index, arg);
        break;
      case "--soak-report-artifact":
        options.soakReportArtifact = readValue(args, ++index, arg);
        break;
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release workflow input argument: ${arg}`);
    }
  }

  if (options.outFile === undefined) {
    throw new Error("--out-file is required");
  }

  return options;
}

export function writeWindowsMvpReleaseWorkflowInputsFile(outFile, inputs) {
  if (!outFile) {
    throw new Error("--out-file is required");
  }

  return writeTextFileAtomic(outFile, `${JSON.stringify(inputs, null, 2)}\n`);
}

export function exitCodeForWindowsMvpReleaseWorkflowInputs(report) {
  return report.ready ? 0 : 1;
}

function buildWorkflowInputs(options) {
  return {
    source_run_id: normalizeText(options.sourceRunId),
    qa_source_run_id: normalizeText(options.qaSourceRunId),
    soak_source_run_id: normalizeText(options.soakSourceRunId),
    qa_report_artifact: normalizeText(options.qaReportArtifact) || DEFAULT_QA_REPORT_ARTIFACT,
    soak_report_artifact: normalizeText(options.soakReportArtifact) || DEFAULT_SOAK_REPORT_ARTIFACT,
  };
}

function evaluateRunIds(inputs) {
  const sourceRunId = inputs.source_run_id;
  const qaSourceRunId = inputs.qa_source_run_id;
  const soakSourceRunId = inputs.soak_source_run_id;
  const candidateIds = [
    ["source_run_id", sourceRunId],
    ["qa_source_run_id", qaSourceRunId],
    ["soak_source_run_id", soakSourceRunId],
  ].filter(([, value]) => value.length > 0);
  const invalidIds = candidateIds
    .filter(([, value]) => !isPositiveIntegerString(value))
    .map(([name]) => name);

  if (invalidIds.length > 0) {
    return {
      id: "sourceRunIds",
      status: "invalid",
      detail: "runIdsMustBePositiveIntegers",
      invalidInputs: invalidIds,
    };
  }

  if (sourceRunId.length > 0) {
    return {
      id: "sourceRunIds",
      status: "pass",
      mode: "shared",
    };
  }

  if (qaSourceRunId.length > 0 && soakSourceRunId.length > 0) {
    return {
      id: "sourceRunIds",
      status: "pass",
      mode: "dedicated",
    };
  }

  return {
    id: "sourceRunIds",
    status: "missing",
    detail: "sourceRunIdsMissing",
    requiredInputs: ["source_run_id", "qa_source_run_id", "soak_source_run_id"],
  };
}

function evaluateArtifactName(id, artifactName) {
  if (artifactName.length === 0) {
    return {
      id,
      status: "missing",
      detail: "artifactNameMissing",
    };
  }

  if (containsPathSeparatorOrControlCharacter(artifactName)) {
    return {
      id,
      status: "invalid",
      detail: "artifactNameContainsPathOrControlCharacter",
    };
  }

  return {
    id,
    status: "pass",
  };
}

function buildNextActions(checks) {
  return checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    switch (check.detail) {
      case "runIdsMustBePositiveIntegers":
        return [
          {
            id: "fixRunIds",
            action: "use GitHub Actions numeric run IDs for every provided source run input",
            inputs: check.invalidInputs,
          },
        ];
      case "sourceRunIdsMissing":
        return [
          {
            id: "provideSourceRunIds",
            action:
              "provide --source-run-id or provide both --qa-source-run-id and --soak-source-run-id",
          },
        ];
      case "artifactNameMissing":
        return [
          {
            id: "setArtifactName",
            action: "provide the expected GitHub Actions artifact name",
            checkId: check.id,
          },
        ];
      case "artifactNameContainsPathOrControlCharacter":
        return [
          {
            id: "sanitizeArtifactName",
            action: "use an artifact name, not a file path",
            checkId: check.id,
          },
        ];
      default:
        return [
          {
            id: "reviewReleaseWorkflowInputs",
            action: "review the failing Windows MVP release workflow input check",
            checkId: check.id,
          },
        ];
    }
  });
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

function normalizeText(value) {
  return typeof value === "string" ? value.trim() : "";
}

function containsPathSeparatorOrControlCharacter(value) {
  return [...value].some((character) => {
    const codePoint = character.codePointAt(0);
    return character === "/" || character === "\\" || (codePoint !== undefined && codePoint < 32);
  });
}

function isPositiveIntegerString(value) {
  return /^[1-9]\d*$/.test(value);
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseWorkflowInputsArgs(process.argv.slice(2));
  const initialReport = buildWindowsMvpReleaseWorkflowInputsReport(options);
  const outputReport = initialReport.ready
    ? buildWindowsMvpReleaseWorkflowInputsReport({
        ...options,
        dispatchInputsWritten: true,
      })
    : initialReport;

  if (outputReport.ready) {
    writeWindowsMvpReleaseWorkflowInputsFile(options.outFile, outputReport.inputs);
  }

  process.stdout.write(`${JSON.stringify(outputReport, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseWorkflowInputs(outputReport));
}
