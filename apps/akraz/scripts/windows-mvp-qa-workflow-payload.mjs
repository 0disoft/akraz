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
  evaluateWindowsMvpQaReport,
  exitCodeForWindowsMvpQaReport,
  readWindowsMvpQaReport,
  writeWindowsMvpQaReportOutputFile,
} from "./windows-mvp-qa-report.mjs";

export const WINDOWS_MVP_QA_WORKFLOW_PAYLOAD_SCHEMA_VERSION =
  "akraz.windowsMvpQaWorkflowPayload/v1";
export const WINDOWS_MVP_QA_WORKFLOW_INPUT_NAME = "qa_report_base64";

export function buildWindowsMvpQaWorkflowPayload(report) {
  return Buffer.from(JSON.stringify(report), "utf8").toString("base64");
}

export function buildWindowsMvpQaWorkflowPayloadReport(report, options = {}) {
  const evaluation = evaluateWindowsMvpQaReport(report);
  const ready = exitCodeForWindowsMvpQaReport(evaluation) === 0;
  const payload = ready ? buildWindowsMvpQaWorkflowPayload(report) : "";

  return {
    schemaVersion: WINDOWS_MVP_QA_WORKFLOW_PAYLOAD_SCHEMA_VERSION,
    ready,
    inputName: WINDOWS_MVP_QA_WORKFLOW_INPUT_NAME,
    payloadEncoding: "base64",
    payloadWritten: Boolean(options.payloadWritten),
    dispatchInputsWritten: Boolean(options.dispatchInputsWritten),
    payloadLength: payload.length,
    evaluation,
    nextActions: ready ? [] : evaluation.nextActions,
    privacy: {
      includesReportPayload: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function parseWindowsMvpQaWorkflowPayloadArgs(args) {
  const options = {
    reportFile: undefined,
    outFile: undefined,
    dispatchInputsFile: undefined,
    evaluationOutFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--report-file":
        options.reportFile = readValue(args, ++index, arg);
        break;
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      case "--dispatch-inputs-file":
        options.dispatchInputsFile = readValue(args, ++index, arg);
        break;
      case "--evaluation-out-file":
        options.evaluationOutFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown Windows MVP QA workflow payload argument: ${arg}`);
    }
  }

  if (options.reportFile === undefined) {
    throw new Error("--report-file is required");
  }

  if (options.outFile === undefined && options.dispatchInputsFile === undefined) {
    throw new Error("at least one of --out-file or --dispatch-inputs-file is required");
  }

  return options;
}

export function writeWindowsMvpQaWorkflowPayloadOutputFile(outFile, payload) {
  if (!outFile) {
    throw new Error("--out-file is required");
  }

  return writeTextFileAtomic(outFile, `${payload}\n`);
}

export function writeWindowsMvpQaWorkflowDispatchInputsFile(outFile, payload) {
  if (!outFile) {
    throw new Error("--dispatch-inputs-file is required");
  }

  return writeTextFileAtomic(
    outFile,
    `${JSON.stringify({ [WINDOWS_MVP_QA_WORKFLOW_INPUT_NAME]: payload }, null, 2)}\n`,
  );
}

export function exitCodeForWindowsMvpQaWorkflowPayload(report) {
  return report.ready ? 0 : 1;
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
  const options = parseWindowsMvpQaWorkflowPayloadArgs(process.argv.slice(2));
  const qaReport = readWindowsMvpQaReport(options.reportFile);
  const initialReport = buildWindowsMvpQaWorkflowPayloadReport(qaReport);

  if (options.evaluationOutFile !== undefined) {
    writeWindowsMvpQaReportOutputFile(options.evaluationOutFile, initialReport.evaluation);
  }

  const outputReport = initialReport.ready
    ? buildWindowsMvpQaWorkflowPayloadReport(qaReport, {
        dispatchInputsWritten: options.dispatchInputsFile !== undefined,
        payloadWritten: options.outFile !== undefined,
      })
    : initialReport;

  if (outputReport.ready) {
    const workflowPayload = buildWindowsMvpQaWorkflowPayload(qaReport);
    if (options.outFile !== undefined) {
      writeWindowsMvpQaWorkflowPayloadOutputFile(options.outFile, workflowPayload);
    }
    if (options.dispatchInputsFile !== undefined) {
      writeWindowsMvpQaWorkflowDispatchInputsFile(options.dispatchInputsFile, workflowPayload);
    }
  }

  process.stdout.write(`${JSON.stringify(outputReport, null, 2)}\n`);
  process.exit(exitCodeForWindowsMvpQaWorkflowPayload(outputReport));
}
