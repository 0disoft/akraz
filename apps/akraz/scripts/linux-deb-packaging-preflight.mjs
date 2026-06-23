import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const LINUX_DEB_PACKAGING_PREFLIGHT_SCHEMA_VERSION = "akraz.linuxDebPackagingPreflight/v1";

export const REQUIRED_LINUX_DEB_DEPENDS = ["libx11-6", "libxi6", "libxtst6", "libxrandr2"];

export const EXPECTED_LINUX_DEB_SECTION = "utils";
export const EXPECTED_LINUX_DEB_PRIORITY = "optional";
export const EXPECTED_TAURI_SIDECAR_EXTERNAL_BIN = "binaries/akraz-daemon";
export const EXPECTED_TAURI_BEFORE_BUILD_COMMAND =
  "bun run prepare:sidecar:release && bun run build";

export function buildLinuxDebPackagingPreflightReport(workspaceRoot = currentWorkspaceRoot()) {
  const rootPackage = readJson(join(workspaceRoot, "package.json"));
  const appPackage = readJson(join(workspaceRoot, "apps", "akraz", "package.json"));
  const tauriConfig = readJson(
    join(workspaceRoot, "apps", "akraz", "src-tauri", "tauri.conf.json"),
  );
  const cargoToml = readFileSync(join(workspaceRoot, "Cargo.toml"), "utf8");
  const checks = [
    evaluateVersionSync(rootPackage, appPackage, tauriConfig, cargoToml),
    evaluateDebTarget(tauriConfig),
    evaluateDebDepends(tauriConfig),
    evaluateDebMetadata(tauriConfig),
    evaluateSidecarPackaging(tauriConfig),
    evaluateBuildHook(tauriConfig),
    evaluateMaintainerScripts(tauriConfig),
  ];

  return {
    schemaVersion: LINUX_DEB_PACKAGING_PREFLIGHT_SCHEMA_VERSION,
    releaseTarget: "Linux X11 .deb draft",
    ready: checks.every((check) => check.status === "pass"),
    tauriConfigPath: "apps/akraz/src-tauri/tauri.conf.json",
    requiredDebDepends: REQUIRED_LINUX_DEB_DEPENDS,
    checks,
    nextActions: buildNextActions(checks),
    manualVerification: [
      {
        id: "linuxDebBuild",
        status: "manual",
        action: "build the .deb on a Linux host with the Tauri Linux prerequisites installed",
      },
      {
        id: "linuxX11RuntimeSmoke",
        status: "manual",
        action:
          "install the .deb on a Linux X11 session and confirm Akraz starts with the bundled daemon sidecar",
      },
    ],
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesBuildLogs: false,
    },
  };
}

export function exitCodeForLinuxDebPackagingPreflight(report) {
  return report.ready ? 0 : 1;
}

function evaluateVersionSync(rootPackage, appPackage, tauriConfig, cargoToml) {
  const rootVersion = rootPackage.version;
  const appVersion = appPackage.version;
  const tauriVersion = tauriConfig.version;
  const cargoVersion = parseCargoWorkspacePackageVersion(cargoToml);
  const versions = [
    ["rootPackage", rootVersion],
    ["appPackage", appVersion],
    ["tauriConfig", tauriVersion],
    ["cargoWorkspace", cargoVersion],
  ];
  const invalidVersions = versions
    .filter(([, version]) => typeof version !== "string" || version !== rootVersion)
    .map(([id, version]) => ({ id, version: version ?? null }));

  if (typeof rootVersion === "string" && invalidVersions.length === 0) {
    return {
      id: "versionSync",
      status: "pass",
      expectedVersion: rootVersion,
    };
  }

  return {
    id: "versionSync",
    status: "invalid",
    detail: "releaseVersionsMustMatchBeforeDebPackaging",
    expectedVersion: typeof rootVersion === "string" ? rootVersion : null,
    invalidVersions,
  };
}

function evaluateDebTarget(tauriConfig) {
  const targets = tauriConfig?.bundle?.targets;
  const includesDeb =
    targets === "all" ||
    targets === "deb" ||
    (Array.isArray(targets) && targets.some((target) => target === "deb"));

  if (includesDeb) {
    return {
      id: "debTarget",
      status: "pass",
      targets,
    };
  }

  return {
    id: "debTarget",
    status: "invalid",
    detail: "tauriBundleTargetsMustIncludeDeb",
    targets: targets ?? null,
  };
}

function evaluateDebDepends(tauriConfig) {
  const depends = debConfig(tauriConfig)?.depends;
  const actualDepends = Array.isArray(depends) ? depends : [];
  const missingDepends = REQUIRED_LINUX_DEB_DEPENDS.filter(
    (dependency) => !actualDepends.includes(dependency),
  );

  if (missingDepends.length === 0) {
    return {
      id: "debDepends",
      status: "pass",
      depends: actualDepends,
    };
  }

  return {
    id: "debDepends",
    status: actualDepends.length === 0 ? "missing" : "invalid",
    detail: "linuxX11RuntimeLibrariesMissingFromDebDepends",
    missingDepends,
    requiredDepends: REQUIRED_LINUX_DEB_DEPENDS,
    actualDepends,
  };
}

function evaluateDebMetadata(tauriConfig) {
  const config = debConfig(tauriConfig);
  const invalidFields = [
    {
      id: "section",
      expectedValue: EXPECTED_LINUX_DEB_SECTION,
      actualValue: config?.section ?? null,
    },
    {
      id: "priority",
      expectedValue: EXPECTED_LINUX_DEB_PRIORITY,
      actualValue: config?.priority ?? null,
    },
  ].filter((field) => field.actualValue !== field.expectedValue);

  if (invalidFields.length === 0) {
    return {
      id: "debMetadata",
      status: "pass",
      section: EXPECTED_LINUX_DEB_SECTION,
      priority: EXPECTED_LINUX_DEB_PRIORITY,
    };
  }

  return {
    id: "debMetadata",
    status: "invalid",
    detail: "debianControlMetadataDrifted",
    invalidFields,
  };
}

function evaluateSidecarPackaging(tauriConfig) {
  const externalBin = Array.isArray(tauriConfig?.bundle?.externalBin)
    ? tauriConfig.bundle.externalBin
    : [];

  if (externalBin.includes(EXPECTED_TAURI_SIDECAR_EXTERNAL_BIN)) {
    return {
      id: "sidecarPackaging",
      status: "pass",
      externalBin: EXPECTED_TAURI_SIDECAR_EXTERNAL_BIN,
    };
  }

  return {
    id: "sidecarPackaging",
    status: externalBin.length === 0 ? "missing" : "invalid",
    detail: "debPackageMustIncludeDaemonSidecar",
    expectedExternalBin: EXPECTED_TAURI_SIDECAR_EXTERNAL_BIN,
    actualExternalBin: externalBin,
  };
}

function evaluateBuildHook(tauriConfig) {
  const beforeBuildCommand = tauriConfig?.build?.beforeBuildCommand;

  if (beforeBuildCommand === EXPECTED_TAURI_BEFORE_BUILD_COMMAND) {
    return {
      id: "buildHook",
      status: "pass",
      beforeBuildCommand,
    };
  }

  return {
    id: "buildHook",
    status: typeof beforeBuildCommand === "string" ? "invalid" : "missing",
    detail: "tauriBuildMustPrepareReleaseSidecarBeforeDebPackaging",
    expectedBeforeBuildCommand: EXPECTED_TAURI_BEFORE_BUILD_COMMAND,
    actualBeforeBuildCommand: beforeBuildCommand ?? null,
  };
}

function evaluateMaintainerScripts(tauriConfig) {
  const config = debConfig(tauriConfig);
  const maintainerScriptFields = [
    "preInstallScript",
    "postInstallScript",
    "preRemoveScript",
    "postRemoveScript",
  ];
  const configuredScripts = maintainerScriptFields.filter(
    (field) => typeof config?.[field] === "string" && config[field].trim().length > 0,
  );

  if (configuredScripts.length === 0) {
    return {
      id: "maintainerScripts",
      status: "pass",
      configuredScripts: [],
    };
  }

  return {
    id: "maintainerScripts",
    status: "invalid",
    detail: "debDraftMustNotRunMaintainerScriptsWithoutReview",
    configuredScripts,
  };
}

function buildNextActions(checks) {
  return checks.flatMap((check) => {
    if (check.status === "pass") {
      return [];
    }

    switch (check.detail) {
      case "releaseVersionsMustMatchBeforeDebPackaging":
        return [
          {
            id: "syncReleaseVersions",
            action: "sync package, Tauri, and Cargo workspace versions before building the .deb",
          },
        ];
      case "tauriBundleTargetsMustIncludeDeb":
        return [
          {
            id: "includeDebTarget",
            action: "configure Tauri bundle targets to include deb or all",
          },
        ];
      case "linuxX11RuntimeLibrariesMissingFromDebDepends":
        return [
          {
            id: "addLinuxX11DebDepends",
            action: "add the Linux X11 runtime libraries to Tauri deb depends",
            missingDepends: check.missingDepends,
          },
        ];
      case "debianControlMetadataDrifted":
        return [
          {
            id: "syncDebControlMetadata",
            action: "restore Debian section and priority metadata",
            invalidFields: check.invalidFields,
          },
        ];
      case "debPackageMustIncludeDaemonSidecar":
        return [
          {
            id: "includeDaemonSidecar",
            action: "restore the Tauri externalBin entry for the Akraz daemon sidecar",
          },
        ];
      case "tauriBuildMustPrepareReleaseSidecarBeforeDebPackaging":
        return [
          {
            id: "restoreReleaseSidecarBuildHook",
            action: "restore the Tauri beforeBuildCommand that prepares the release sidecar",
          },
        ];
      case "debDraftMustNotRunMaintainerScriptsWithoutReview":
        return [
          {
            id: "reviewMaintainerScripts",
            action:
              "remove Debian maintainer scripts or add a dedicated reviewed install lifecycle",
            configuredScripts: check.configuredScripts,
          },
        ];
      default:
        return [
          {
            id: "reviewLinuxDebPreflight",
            action: "review the failing Linux .deb packaging preflight check",
            checkId: check.id,
          },
        ];
    }
  });
}

export function parseCargoWorkspacePackageVersion(cargoToml) {
  let inWorkspacePackage = false;

  for (const line of cargoToml.split(/\r?\n/)) {
    if (line.trim() === "[workspace.package]") {
      inWorkspacePackage = true;
      continue;
    }

    if (inWorkspacePackage && line.trimStart().startsWith("[")) {
      return undefined;
    }

    if (inWorkspacePackage) {
      const match = line.match(/^version\s*=\s*"(?<version>[^"]+)"$/);
      if (match?.groups?.version) {
        return match.groups.version;
      }
    }
  }

  return undefined;
}

function debConfig(tauriConfig) {
  return tauriConfig?.bundle?.linux?.deb;
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function currentWorkspaceRoot() {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  return join(appRoot, "..", "..");
}

if (import.meta.main) {
  const report = buildLinuxDebPackagingPreflightReport();
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForLinuxDebPackagingPreflight(report));
}
