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
const REQUIRED_SMOKE_WORKFLOW_SCRIPTS = [
  "smoke:loopback-transport",
  "smoke:tcp-transport",
  "smoke:peer-session",
  "smoke:peer-session-executor",
  "smoke:session-connect-lifecycle",
  "smoke:diagnostics-snapshot",
  "smoke:daemon-lifecycle",
  "smoke:settings-start",
  "smoke:windows-mvp-soak",
];
const SMOKE_SOAK_REPORT_PATH = "apps/akraz/reports/windows-mvp-soak-smoke.json";
const WORKSPACE_DELEGATED_APP_SCRIPTS = [
  "build",
  "qa:windows-mvp-plan",
  "qa:windows-mvp-report",
  "qa:windows-mvp-report-template",
  "qa:windows-mvp-report-update",
  "qa:windows-mvp-workflow-payload",
  "release:signing-preflight",
  "release:updater-config-preflight",
  "release:windows-mvp-bundle",
  "release:windows-mvp-evidence-sources",
  "release:windows-mvp-gate",
  "release:windows-mvp-resolved-evidence",
  "release:windows-mvp-workflow-inputs",
  "smoke:daemon-lifecycle",
  "smoke:diagnostics-snapshot",
  "smoke:loopback-transport",
  "smoke:peer-session",
  "smoke:peer-session-executor",
  "smoke:session-connect-lifecycle",
  "smoke:settings-start",
  "smoke:signing-preflight",
  "smoke:tcp-transport",
  "smoke:updater-config-preflight",
  "smoke:windows-mvp-release-bundle",
  "smoke:windows-mvp-release-gate",
  "smoke:windows-mvp-soak",
  "verify:release-metadata",
  "verify:workflow-contracts",
];
const EXPECTED_APP_PACKAGE_SCRIPTS = {
  build: "vite build",
  check: "svelte-check --tsconfig ./tsconfig.json",
  "prepare:sidecar": "bun scripts/prepare-sidecar.mjs",
  "prepare:sidecar:release": "bun scripts/prepare-sidecar.mjs --release",
  "qa:windows-mvp-plan": "bun scripts/windows-mvp-qa-plan.mjs",
  "qa:windows-mvp-report": "bun scripts/windows-mvp-qa-report.mjs",
  "qa:windows-mvp-report-template": "bun scripts/windows-mvp-qa-report.mjs --template",
  "qa:windows-mvp-report-update": "bun scripts/windows-mvp-qa-report.mjs --update-result",
  "qa:windows-mvp-workflow-payload": "bun scripts/windows-mvp-qa-workflow-payload.mjs",
  "release:signing-preflight": "bun scripts/smoke-signing-preflight.mjs",
  "release:updater-config-preflight": "bun scripts/smoke-updater-config-preflight.mjs",
  "release:windows-mvp-bundle": "bun scripts/windows-mvp-release-bundle.mjs",
  "release:windows-mvp-evidence-sources": "bun scripts/windows-mvp-release-evidence-sources.mjs",
  "release:windows-mvp-gate": "bun scripts/windows-mvp-release-gate.mjs",
  "release:windows-mvp-resolved-evidence": "bun scripts/windows-mvp-release-resolved-evidence.mjs",
  "release:windows-mvp-workflow-inputs": "bun scripts/windows-mvp-release-workflow-inputs.mjs",
  "smoke:daemon-lifecycle": "bun scripts/smoke-daemon-lifecycle.mjs",
  "smoke:diagnostics-snapshot": "bun scripts/smoke-diagnostics-snapshot.mjs",
  "smoke:loopback-transport": "bun scripts/smoke-loopback-transport.mjs",
  "smoke:peer-session": "bun scripts/smoke-peer-session.mjs",
  "smoke:peer-session-executor": "bun scripts/smoke-peer-session-executor.mjs",
  "smoke:session-connect-lifecycle": "bun scripts/smoke-session-connect-lifecycle.mjs",
  "smoke:settings-start": "bun scripts/smoke-daemon-lifecycle.mjs settings-start",
  "smoke:signing-preflight": "bun scripts/smoke-signing-preflight.mjs --expect-missing",
  "smoke:tcp-transport": "bun scripts/smoke-tcp-transport.mjs",
  "smoke:updater-config-preflight": "bun scripts/smoke-updater-config-preflight.mjs --expect-missing",
  "smoke:windows-mvp-release-bundle": "bun scripts/smoke-windows-mvp-release-bundle.mjs",
  "smoke:windows-mvp-release-gate": "bun scripts/smoke-windows-mvp-release-gate.mjs",
  "smoke:windows-mvp-soak": "bun scripts/smoke-windows-mvp-soak.mjs",
  tauri: "tauri",
  "verify:release-metadata": "bun scripts/verify-release-metadata.mjs",
  "verify:workflow-contracts": "bun scripts/verify-workflow-contracts.mjs",
};

export function buildWorkflowContractsReport(workspaceRoot = currentWorkspaceRoot()) {
  const rootPackage = readJsonFile(join(workspaceRoot, "package.json"));
  const appPackage = readJsonFile(join(workspaceRoot, "apps", "akraz", "package.json"));
  const workflows = readWorkflowFiles(workspaceRoot);
  const expectedBunVersion = packageManagerBunVersion(rootPackage.packageManager);
  const workflowScripts = uniqueWorkflowScripts(workflows);
  const checks = [
    evaluateBunPackageManager(rootPackage.packageManager, expectedBunVersion),
    ...evaluateWorkspaceAppScriptDelegation(rootPackage.scripts ?? {}, appPackage.scripts ?? {}),
    ...evaluateAppPackageScripts(appPackage.scripts ?? {}),
    ...evaluateCheckoutVersions(workflows),
    ...evaluateBunVersions(workflows, expectedBunVersion),
    ...evaluateWorkflowScripts(workflowScripts, rootPackage.scripts ?? {}),
    evaluateReleaseWorkflowFile(workflows),
    evaluateReleaseArtifactContract(workflows),
    evaluateReleaseEvidenceSourcesWiring(workflows),
    evaluateSmokeWorkflowCoverage(workflows),
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

function evaluateWorkspaceAppScriptDelegation(rootScripts, appScripts) {
  return WORKSPACE_DELEGATED_APP_SCRIPTS.map((scriptName) => {
    const expectedCommand = `bun run --cwd apps/akraz ${scriptName}`;
    const actualCommand = rootScripts[scriptName];
    const appCommand = appScripts[scriptName];

    if (actualCommand === expectedCommand && typeof appCommand === "string") {
      return {
        id: `workspaceAppScript:${scriptName}`,
        status: "pass",
        scriptName,
        expectedCommand,
      };
    }

    return {
      id: `workspaceAppScript:${scriptName}`,
      status: typeof actualCommand === "string" ? "invalid" : "missing",
      detail:
        typeof appCommand === "string" ? "workspaceScriptDelegationDrifted" : "appScriptMissing",
      scriptName,
      expectedCommand,
      actualCommand: typeof actualCommand === "string" ? actualCommand : null,
      appCommand: typeof appCommand === "string" ? appCommand : null,
    };
  });
}

function evaluateAppPackageScripts(appScripts) {
  return Object.entries(EXPECTED_APP_PACKAGE_SCRIPTS).map(([scriptName, expectedCommand]) => {
    const actualCommand = appScripts[scriptName];

    if (actualCommand === expectedCommand) {
      return {
        id: `appPackageScript:${scriptName}`,
        status: "pass",
        scriptName,
        expectedCommand,
      };
    }

    return {
      id: `appPackageScript:${scriptName}`,
      status: typeof actualCommand === "string" ? "invalid" : "missing",
      detail: "appPackageScriptDrifted",
      scriptName,
      expectedCommand,
      actualCommand: typeof actualCommand === "string" ? actualCommand : null,
    };
  });
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

function evaluateSmokeWorkflowCoverage(workflows) {
  const checkWorkflow = workflows.find((workflow) => workflow.name === "check.yml");
  if (!checkWorkflow) {
    return {
      id: "smokeWorkflowCoverage",
      status: "missing",
      detail: "checkWorkflowMissing",
      workflowFile: "check.yml",
      requiredScripts: REQUIRED_SMOKE_WORKFLOW_SCRIPTS,
    };
  }

  const workflowScripts = uniqueWorkflowScripts([checkWorkflow]);
  const missingScripts = REQUIRED_SMOKE_WORKFLOW_SCRIPTS.filter(
    (scriptName) => !workflowScripts.includes(scriptName),
  );
  const requiredSnippets = [
    {
      id: "smokeNeedsRust",
      snippet: "      - rust",
    },
    {
      id: "smokeNeedsFrontend",
      snippet: "      - frontend",
    },
    {
      id: "smokeRunsOnWindows",
      snippet: "runs-on: windows-latest",
    },
    {
      id: "smokeSoakDuration",
      snippet: "--duration-ms 1",
    },
    {
      id: "smokeSoakMaxCycles",
      snippet: "--max-cycles 1",
    },
    {
      id: "smokeSoakReportFile",
      snippet: "--report-file reports/windows-mvp-soak-smoke.json",
    },
    {
      id: "smokeSoakUploadArtifact",
      snippet: "name: windows-mvp-soak-smoke-report",
    },
    {
      id: "smokeSoakUploadPath",
      snippet: `path: ${SMOKE_SOAK_REPORT_PATH}`,
    },
  ];
  const missingSnippets = requiredSnippets
    .filter((requirement) => !checkWorkflow.source.includes(requirement.snippet))
    .map((requirement) => requirement.id);

  if (missingScripts.length === 0 && missingSnippets.length === 0) {
    return {
      id: "smokeWorkflowCoverage",
      status: "pass",
      workflowFile: "check.yml",
      requiredScripts: REQUIRED_SMOKE_WORKFLOW_SCRIPTS,
      soakReportPath: SMOKE_SOAK_REPORT_PATH,
    };
  }

  return {
    id: "smokeWorkflowCoverage",
    status: "invalid",
    detail: "smokeWorkflowCoverageDrifted",
    workflowFile: "check.yml",
    missingScripts,
    missingSnippets,
    requiredScripts: REQUIRED_SMOKE_WORKFLOW_SCRIPTS,
    soakReportPath: SMOKE_SOAK_REPORT_PATH,
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
      case "workspaceScriptDelegationDrifted":
      case "appScriptMissing":
        return [
          {
            id: "syncWorkspaceAppScript",
            action: "sync the root package script with the app package script",
            scriptName: check.scriptName,
            expectedCommand: check.expectedCommand,
          },
        ];
      case "appPackageScriptDrifted":
        return [
          {
            id: "syncAppPackageScript",
            action: "restore the expected app package script command",
            scriptName: check.scriptName,
            expectedCommand: check.expectedCommand,
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
      case "smokeWorkflowCoverageDrifted":
        return [
          {
            id: "syncSmokeWorkflowCoverage",
            action: "restore the Windows smoke workflow scripts and soak report artifact wiring",
            workflowFile: check.workflowFile,
            missingScripts: check.missingScripts,
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
