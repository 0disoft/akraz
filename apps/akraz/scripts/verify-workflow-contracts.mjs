import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  DEFAULT_QA_REPORT_ARTIFACT,
  DEFAULT_SOAK_REPORT_ARTIFACT,
  WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
} from "./windows-mvp-release-workflow-inputs.mjs";

export const WORKFLOW_CONTRACTS_SCHEMA_VERSION = "akraz.workflowContracts/v1";

const WORKFLOW_DIRECTORY = ".github/workflows";
const REQUIRED_CHECKOUT_VERSION = "v6";
const RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH =
  "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-evidence-sources.json";
const RELEASE_WORKFLOW_INPUTS_MANIFEST_PATH =
  "$RELEASE_EVIDENCE_DIR/manifest/windows-mvp-release-workflow-inputs.json";
const RELEASE_BUNDLE_EVIDENCE_SOURCES_SNIPPET = [
  "bun run release:windows-mvp-bundle -- \\",
  `            --evidence-sources-file "${RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH}" \\`,
].join("\n");
const RESOLVED_EVIDENCE_COMMAND_SNIPPET = [
  "bun run release:windows-mvp-resolved-evidence -- \\",
  `            --evidence-sources-file "${RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH}" \\`,
  '            --qa-report-file "$qa_report_file" \\',
  '            --soak-report-file "$soak_report_file"',
].join("\n");

export function buildWorkflowContractsReport(workspaceRoot = currentWorkspaceRoot()) {
  const rootPackage = readJsonFile(join(workspaceRoot, "package.json"));
  const workflows = readWorkflowFiles(workspaceRoot);
  const expectedBunVersion = packageManagerBunVersion(rootPackage.packageManager);
  const workflowScripts = uniqueWorkflowScripts(workflows);
  const checks = [
    evaluateBunPackageManager(rootPackage.packageManager, expectedBunVersion),
    ...evaluateCheckoutVersions(workflows),
    ...evaluateBunVersions(workflows, expectedBunVersion),
    ...evaluateWorkflowScripts(workflowScripts, rootPackage.scripts ?? {}),
    evaluateReleaseWorkflowFile(workflows),
    evaluateReleaseArtifactContract(workflows),
    evaluateReleaseEvidenceSourcesWiring(workflows),
  ];

  return {
    schemaVersion: WORKFLOW_CONTRACTS_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    workflowDirectory: WORKFLOW_DIRECTORY,
    workflowFiles: workflows.map((workflow) => workflow.name).toSorted(),
    expectedBunVersion,
    workflowScripts,
    checks,
    nextActions: buildNextActions(checks),
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesWorkflowPayloads: false,
    },
  };
}

export function exitCodeForWorkflowContracts(report) {
  return report.ready ? 0 : 1;
}

function evaluateBunPackageManager(packageManager, expectedBunVersion) {
  if (expectedBunVersion) {
    return {
      id: "packageManagerBunVersion",
      status: "pass",
      expectedBunVersion,
    };
  }

  return {
    id: "packageManagerBunVersion",
    status: "invalid",
    detail: "packageManagerMustDeclareBunVersion",
    packageManager: typeof packageManager === "string" ? packageManager : null,
  };
}

function evaluateCheckoutVersions(workflows) {
  return workflows.flatMap((workflow) => {
    const matches = [...workflow.source.matchAll(/uses:\s+actions\/checkout@([^\s#]+)/g)];
    if (matches.length === 0) {
      return [
        {
          id: `checkoutVersion:${workflow.name}`,
          status: "missing",
          detail: "checkoutActionMissing",
          workflowFile: workflow.name,
        },
      ];
    }

    return matches.map((match) => {
      const actualVersion = match[1];
      return {
        id: `checkoutVersion:${workflow.name}:${lineNumberForOffset(workflow.source, match.index)}`,
        status: actualVersion === REQUIRED_CHECKOUT_VERSION ? "pass" : "invalid",
        workflowFile: workflow.name,
        expectedVersion: REQUIRED_CHECKOUT_VERSION,
        actualVersion,
      };
    });
  });
}

function evaluateBunVersions(workflows, expectedBunVersion) {
  return workflows.flatMap((workflow) => {
    if (!workflow.source.includes("oven-sh/setup-bun@")) {
      return [];
    }

    const matches = [...workflow.source.matchAll(/bun-version:\s*([^\s#]+)/g)];
    if (matches.length === 0) {
      return [
        {
          id: `bunVersion:${workflow.name}`,
          status: "missing",
          detail: "setupBunVersionMissing",
          workflowFile: workflow.name,
          expectedBunVersion,
        },
      ];
    }

    return matches.map((match) => {
      const actualVersion = unquote(match[1]);
      return {
        id: `bunVersion:${workflow.name}:${lineNumberForOffset(workflow.source, match.index)}`,
        status: actualVersion === expectedBunVersion ? "pass" : "invalid",
        workflowFile: workflow.name,
        expectedBunVersion,
        actualVersion,
      };
    });
  });
}

function evaluateWorkflowScripts(workflowScripts, rootScripts) {
  if (workflowScripts.length === 0) {
    return [
      {
        id: "workflowScripts",
        status: "missing",
        detail: "workflowBunScriptsMissing",
      },
    ];
  }

  return workflowScripts.map((scriptName) => ({
    id: `workflowScript:${scriptName}`,
    status: typeof rootScripts[scriptName] === "string" ? "pass" : "invalid",
    scriptName,
    detail: typeof rootScripts[scriptName] === "string" ? undefined : "packageScriptMissing",
  }));
}

function evaluateReleaseWorkflowFile(workflows) {
  const workflow = workflows.find(
    (candidate) => candidate.name === WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  );
  if (workflow) {
    return {
      id: "releaseWorkflowFile",
      status: "pass",
      workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
    };
  }

  return {
    id: "releaseWorkflowFile",
    status: "missing",
    detail: "releaseWorkflowFileMissing",
    workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  };
}

function evaluateReleaseArtifactContract(workflows) {
  const releaseWorkflow = workflows.find(
    (workflow) => workflow.name === WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  );
  const qaWorkflow = workflows.find((workflow) => workflow.name === "windows-mvp-qa.yml");
  const soakWorkflow = workflows.find((workflow) => workflow.name === "windows-mvp-soak.yml");
  const releaseQaDefault = releaseWorkflow
    ? inputDefault(releaseWorkflow.source, "qa_report_artifact")
    : null;
  const releaseSoakDefault = releaseWorkflow
    ? inputDefault(releaseWorkflow.source, "soak_report_artifact")
    : null;
  const qaUploadName = qaWorkflow ? uploadArtifactName(qaWorkflow.source) : null;
  const soakUploadName = soakWorkflow ? uploadArtifactName(soakWorkflow.source) : null;
  const mismatches = [];

  if (
    releaseQaDefault !== DEFAULT_QA_REPORT_ARTIFACT ||
    qaUploadName !== DEFAULT_QA_REPORT_ARTIFACT
  ) {
    mismatches.push("qaReportArtifact");
  }
  if (
    releaseSoakDefault !== DEFAULT_SOAK_REPORT_ARTIFACT ||
    soakUploadName !== DEFAULT_SOAK_REPORT_ARTIFACT
  ) {
    mismatches.push("soakReportArtifact");
  }

  if (mismatches.length === 0) {
    return {
      id: "releaseArtifactNames",
      status: "pass",
      qaReportArtifact: DEFAULT_QA_REPORT_ARTIFACT,
      soakReportArtifact: DEFAULT_SOAK_REPORT_ARTIFACT,
    };
  }

  return {
    id: "releaseArtifactNames",
    status: "invalid",
    detail: "releaseArtifactNamesDrifted",
    mismatches,
    expected: {
      qaReportArtifact: DEFAULT_QA_REPORT_ARTIFACT,
      soakReportArtifact: DEFAULT_SOAK_REPORT_ARTIFACT,
    },
    actual: {
      releaseQaDefault,
      releaseSoakDefault,
      qaUploadName,
      soakUploadName,
    },
  };
}

function evaluateReleaseEvidenceSourcesWiring(workflows) {
  const releaseWorkflow = workflows.find(
    (workflow) => workflow.name === WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
  );
  if (!releaseWorkflow) {
    return {
      id: "releaseEvidenceSourcesWiring",
      status: "missing",
      detail: "releaseWorkflowFileMissing",
      workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
    };
  }

  const requiredSnippets = [
    {
      id: "evidenceSourcesScript",
      snippet: "release:windows-mvp-evidence-sources",
    },
    {
      id: "evidenceSourcesOutFile",
      snippet: "--out-file",
    },
    {
      id: "evidenceSourcesManifestPath",
      snippet: RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH,
    },
    {
      id: "workflowInputsDispatchFile",
      snippet: "--dispatch-inputs-file",
    },
    {
      id: "workflowInputsManifestPath",
      snippet: RELEASE_WORKFLOW_INPUTS_MANIFEST_PATH,
    },
    {
      id: "releaseBundleScript",
      snippet: "release:windows-mvp-bundle",
    },
    {
      id: "resolvedEvidenceCommand",
      snippet: RESOLVED_EVIDENCE_COMMAND_SNIPPET,
    },
    {
      id: "releaseBundleEvidenceSourcesCommand",
      snippet: RELEASE_BUNDLE_EVIDENCE_SOURCES_SNIPPET,
    },
    {
      id: "releaseBundleIntegritySmokeScript",
      snippet: "smoke:windows-mvp-release-bundle",
    },
    {
      id: "releaseBundleIntegrityBundleDirArgument",
      snippet: "--bundle-dir",
    },
  ];
  const missingSnippets = requiredSnippets
    .filter((requirement) => !releaseWorkflow.source.includes(requirement.snippet))
    .map((requirement) => requirement.id);

  if (missingSnippets.length === 0) {
    return {
      id: "releaseEvidenceSourcesWiring",
      status: "pass",
      workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
      evidenceSourcesManifestPath: RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH,
      workflowInputsManifestPath: RELEASE_WORKFLOW_INPUTS_MANIFEST_PATH,
    };
  }

  return {
    id: "releaseEvidenceSourcesWiring",
    status: "invalid",
    detail: "releaseEvidenceSourcesWiringDrifted",
    workflowFile: WINDOWS_MVP_RELEASE_WORKFLOW_FILE,
    missingSnippets,
    expected: {
      evidenceSourcesManifestPath: RELEASE_EVIDENCE_SOURCES_MANIFEST_PATH,
      workflowInputsManifestPath: RELEASE_WORKFLOW_INPUTS_MANIFEST_PATH,
    },
  };
}

function buildNextActions(checks) {
  return checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    switch (check.detail) {
      case "checkoutActionMissing":
        return [
          {
            id: "addCheckoutAction",
            action: "add actions/checkout to the workflow before repository commands run",
            workflowFile: check.workflowFile,
          },
        ];
      case "setupBunVersionMissing":
      case "packageManagerMustDeclareBunVersion":
        return [
          {
            id: "syncBunVersion",
            action: "declare one Bun version in packageManager and every setup-bun step",
            workflowFile: check.workflowFile,
          },
        ];
      case "packageScriptMissing":
        return [
          {
            id: "addPackageScript",
            action: "add the workflow-called package script or update the workflow command",
            scriptName: check.scriptName,
          },
        ];
      case "releaseWorkflowFileMissing":
        return [
          {
            id: "restoreReleaseWorkflow",
            action:
              "restore the Windows MVP release workflow or update the release workflow constant",
            workflowFile: check.workflowFile,
          },
        ];
      case "releaseArtifactNamesDrifted":
        return [
          {
            id: "syncReleaseArtifactNames",
            action:
              "sync release workflow artifact defaults with QA and soak upload artifact names",
            mismatches: check.mismatches,
          },
        ];
      case "releaseEvidenceSourcesWiringDrifted":
        return [
          {
            id: "syncReleaseEvidenceSourcesWiring",
            action: "wire the release evidence source manifest generation into the bundle command",
            workflowFile: check.workflowFile,
            missingSnippets: check.missingSnippets,
          },
        ];
      default:
        if (check.id?.startsWith("checkoutVersion:")) {
          return [
            {
              id: "upgradeCheckoutAction",
              action: `use actions/checkout@${REQUIRED_CHECKOUT_VERSION}`,
              workflowFile: check.workflowFile,
            },
          ];
        }
        if (check.id?.startsWith("bunVersion:")) {
          return [
            {
              id: "syncBunVersion",
              action: "sync setup-bun with the packageManager Bun version",
              workflowFile: check.workflowFile,
            },
          ];
        }
        return [
          {
            id: "reviewWorkflowContract",
            action: "review the failing GitHub Actions workflow contract check",
            checkId: check.id,
          },
        ];
    }
  });
}

function uniqueWorkflowScripts(workflows) {
  return [
    ...new Set(workflows.flatMap((workflow) => extractBunRunScripts(workflow.source))),
  ].toSorted();
}

function extractBunRunScripts(source) {
  const directScripts = [...source.matchAll(/\bbun\s+run\s+([A-Za-z0-9:_-]+)/g)].map(
    (match) => match[1],
  );
  const arrayScripts = [];
  const lines = source.split(/\r?\n/);
  for (let index = 0; index < lines.length - 1; index += 1) {
    if (unquoteListItem(lines[index]) !== "run") {
      continue;
    }

    const nextValue = unquoteListItem(lines[index + 1]);
    if (nextValue && /^[A-Za-z0-9:_-]+$/.test(nextValue)) {
      arrayScripts.push(nextValue);
    }
  }

  return [...directScripts, ...arrayScripts];
}

function inputDefault(source, inputName) {
  const pattern = new RegExp(`${inputName}:[\\s\\S]*?\\n\\s+default:\\s*([^\\n#]+)`);
  const match = source.match(pattern);
  return match ? unquote(match[1].trim()) : null;
}

function uploadArtifactName(source) {
  const match = source.match(
    /uses:\s+actions\/upload-artifact@[^\n]+[\s\S]*?\n\s+name:\s*([^\n#]+)/,
  );
  return match ? unquote(match[1].trim()) : null;
}

function readWorkflowFiles(workspaceRoot) {
  const workflowDirectory = join(workspaceRoot, WORKFLOW_DIRECTORY);
  return readdirSync(workflowDirectory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && /\.ya?ml$/.test(entry.name))
    .map((entry) => ({
      name: entry.name,
      source: readFileSync(join(workflowDirectory, entry.name), "utf8"),
    }));
}

function readJsonFile(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function packageManagerBunVersion(packageManager) {
  if (typeof packageManager !== "string") {
    return null;
  }

  const match = packageManager.match(/^bun@(.+)$/);
  return match ? match[1] : null;
}

function unquoteListItem(value) {
  const trimmed = value.trim().replace(/,$/, "");
  return unquote(trimmed);
}

function unquote(value) {
  const trimmed = String(value).trim();
  const quote = trimmed[0];
  if ((quote === `"` || quote === "'") && trimmed.endsWith(quote)) {
    return trimmed.slice(1, -1);
  }
  return trimmed;
}

function lineNumberForOffset(source, offset) {
  return source.slice(0, offset).split(/\r?\n/).length;
}

function currentWorkspaceRoot() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  return join(appRoot, "..", "..");
}

if (import.meta.main) {
  const report = buildWorkflowContractsReport();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForWorkflowContracts(report));
}
