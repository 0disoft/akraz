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

import {
  DEFAULT_QA_REPORT_ARTIFACT,
  DEFAULT_SOAK_REPORT_ARTIFACT,
  WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  buildWindowsMvpReleaseWorkflowInputsReport,
} from "./windows-mvp-release-workflow-inputs.mjs";

export const WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION =
  "akraz.windowsMvpReleaseEvidenceSources/v1";

export const WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES = {
  qaReport: "windows-mvp-qa-report.json",
  soakReport: "windows-mvp-soak-report.json",
};

export function buildWindowsMvpReleaseEvidenceSourcesReport(options = {}) {
  const workflowInputsReport = buildWindowsMvpReleaseWorkflowInputsReport(options);
  const sources = buildEvidenceSources(workflowInputsReport);
  const checks = [evaluateWorkflowInputs(workflowInputsReport), evaluateEvidenceSources(sources)];
  const ready = checks.every((check) => check.status === "pass");

  return {
    schemaVersion: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCES_SCHEMA_VERSION,
    ready,
    workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
    manifestWritten: Boolean(options.manifestWritten),
    dispatchInputsWritten: Boolean(options.dispatchInputsWritten),
    sources,
    dispatchInputs: workflowInputsReport.inputs,
    checks,
    nextActions: buildNextActions(checks, workflowInputsReport.nextActions),
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesArtifactPayloads: false,
    },
  };
}

export function parseWindowsMvpReleaseEvidenceSourcesArgs(args) {
  const options = {
    sourceRunId: undefined,
    qaSourceRunId: undefined,
    soakSourceRunId: undefined,
    qaReportArtifact: DEFAULT_QA_REPORT_ARTIFACT,
    soakReportArtifact: DEFAULT_SOAK_REPORT_ARTIFACT,
    outFile: undefined,
    dispatchInputsFile: undefined,
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
      case "--dispatch-inputs-file":
        options.dispatchInputsFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP release evidence sources argument: ${arg}`);
    }
  }

  if (options.outFile === undefined && options.dispatchInputsFile === undefined) {
    throw new Error("at least one of --out-file or --dispatch-inputs-file is required");
  }

  return options;
}

export function writeWindowsMvpReleaseEvidenceSourcesFile(outFile, report) {
  if (!outFile) {
    throw new Error("--out-file is required");
  }

  return writeJsonFileAtomic(outFile, report);
}

export function writeWindowsMvpReleaseEvidenceSourcesDispatchInputsFile(outFile, inputs) {
  if (!outFile) {
    throw new Error("--dispatch-inputs-file is required");
  }

  return writeJsonFileAtomic(outFile, inputs);
}

export function exitCodeForWindowsMvpReleaseEvidenceSources(report) {
  return report.ready ? 0 : 1;
}

function buildEvidenceSources(workflowInputsReport) {
  const inputs = workflowInputsReport.inputs;

  return [
    {
      id: "qaReport",
      sourceRunId: workflowInputsReport.resolvedRunIds.qa,
      artifactName: inputs.qa_report_artifact,
      expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
      bundle: {
        artifactId: "qaReport",
        releaseGateCheckId: "qaReport",
        fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.qaReport,
      },
    },
    {
      id: "soakReport",
      sourceRunId: workflowInputsReport.resolvedRunIds.soak,
      artifactName: inputs.soak_report_artifact,
      expectedFileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
      bundle: {
        artifactId: "soakReport",
        releaseGateCheckId: "soakReport",
        fileName: WINDOWS_MVP_RELEASE_EVIDENCE_SOURCE_FILES.soakReport,
      },
    },
  ];
}

function evaluateWorkflowInputs(workflowInputsReport) {
  if (workflowInputsReport.ready) {
    return {
      id: "workflowInputs",
      status: "pass",
    };
  }

  return {
    id: "workflowInputs",
    status: "invalid",
    detail: "releaseWorkflowInputsNotReady",
    failingChecks: workflowInputsReport.checks
      .filter((check) => check.status !== "pass")
      .map((check) => check.id),
  };
}

function evaluateEvidenceSources(sources) {
  const missingSourceIds = sources
    .filter((source) => source.sourceRunId === null || source.artifactName.length === 0)
    .map((source) => source.id);

  if (missingSourceIds.length === 0) {
    return {
      id: "evidenceSources",
      status: "pass",
    };
  }

  return {
    id: "evidenceSources",
    status: "missing",
    detail: "evidenceSourcesMissing",
    missingSourceIds,
  };
}

function buildNextActions(checks, workflowInputNextActions) {
  const sourceActions = checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    switch (check.detail) {
      case "releaseWorkflowInputsNotReady":
        return [
          {
            id: "fixReleaseWorkflowInputs",
            action: "provide valid Windows MVP release workflow source run inputs",
            failingChecks: check.failingChecks,
          },
        ];
      case "evidenceSourcesMissing":
        return [
          {
            id: "provideEvidenceSources",
            action: "provide source run IDs and artifact names for every release evidence source",
            missingSourceIds: check.missingSourceIds,
          },
        ];
      default:
        return [
          {
            id: "reviewReleaseEvidenceSources",
            action: "review the failing Windows MVP release evidence source check",
            checkId: check.id,
          },
        ];
    }
  });

  return [...sourceActions, ...workflowInputNextActions];
}

function writeJsonFileAtomic(outFile, payload) {
  return writeTextFileAtomic(outFile, `${JSON.stringify(payload, null, 2)}\n`);
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

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseWindowsMvpReleaseEvidenceSourcesArgs(process.argv.slice(2));
  const initialReport = buildWindowsMvpReleaseEvidenceSourcesReport(options);
  const outputReport = initialReport.ready
    ? buildWindowsMvpReleaseEvidenceSourcesReport({
        ...options,
        dispatchInputsWritten: options.dispatchInputsFile !== undefined,
        manifestWritten: options.outFile !== undefined,
      })
    : initialReport;

  if (outputReport.ready) {
    if (options.outFile !== undefined) {
      writeWindowsMvpReleaseEvidenceSourcesFile(options.outFile, outputReport);
    }
    if (options.dispatchInputsFile !== undefined) {
      writeWindowsMvpReleaseEvidenceSourcesDispatchInputsFile(
        options.dispatchInputsFile,
        outputReport.dispatchInputs,
      );
    }
  }

  process.stdout.write(`${JSON.stringify(outputReport, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpReleaseEvidenceSources(outputReport));
}
