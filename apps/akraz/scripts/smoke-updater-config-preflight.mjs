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

export const UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION = "akraz.updaterConfig.preflight/v1";

const TAURI_CONFIG_SOURCE = "apps/akraz/src-tauri/tauri.conf.json";
const WINDOWS_INSTALL_MODES = new Set(["passive", "basicUi", "quiet"]);

export function evaluateUpdaterConfigPreflight(config) {
  const checks = [
    evaluateCreateUpdaterArtifacts(config),
    evaluateUpdaterPubkey(config),
    evaluateUpdaterEndpoints(config),
    evaluateDangerousTransport(config),
    evaluateWindowsInstallMode(config),
  ];

  return {
    schemaVersion: UPDATER_CONFIG_PREFLIGHT_SCHEMA_VERSION,
    ready: checks.every((check) => check.status === "pass"),
    checks,
    privacy: {
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesEndpointValues: false,
    },
  };
}

export function readTauriConfig(workspaceRoot) {
  return JSON.parse(
    readFileSync(join(workspaceRoot, "apps", "akraz", "src-tauri", "tauri.conf.json"), "utf8"),
  );
}

export function parseUpdaterConfigPreflightArgs(args) {
  const options = {
    expectMissing: false,
    outFile: undefined,
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    switch (arg) {
      case "--expect-missing":
        options.expectMissing = true;
        break;
      case "--out-file":
        options.outFile = readValue(args, ++index, arg);
        break;
      default:
        throw new Error(`unknown updater config preflight argument: ${arg}`);
    }
  }

  return options;
}

export function exitCodeForUpdaterConfigPreflight(report, options = {}) {
  if (report.ready) {
    return options.expectMissing ? 1 : 0;
  }

  return options.expectMissing && hasOnlyMissingChecks(report) ? 0 : 1;
}

export function writeUpdaterConfigPreflightOutputFile(outFile, payload) {
  if (!outFile) {
    return undefined;
  }

  const resolvedOutFile = resolve(outFile);
  const outDirectory = dirname(resolvedOutFile);
  const tempFile = resolve(
    outDirectory,
    `.${basename(resolvedOutFile)}.${process.pid}.${Date.now()}.tmp`,
  );
  const serializedPayload = `${JSON.stringify(payload, null, 2)}\n`;

  mkdirSync(outDirectory, { recursive: true });

  let fileDescriptor;
  try {
    fileDescriptor = openSync(tempFile, "w", 0o600);
    writeFileSync(fileDescriptor, serializedPayload, "utf8");
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

function hasOnlyMissingChecks(report) {
  return (
    report.checks.some((check) => check.status === "missing") &&
    report.checks.every((check) => check.status === "pass" || check.status === "missing")
  );
}

function evaluateCreateUpdaterArtifacts(config) {
  const value = config?.bundle?.createUpdaterArtifacts;

  if (value === undefined) {
    return buildCheck("bundleCreateUpdaterArtifacts", "bundle", "missing", {
      detail: "createUpdaterArtifactsMissing",
    });
  }

  if (value !== true) {
    return buildCheck("bundleCreateUpdaterArtifacts", "bundle", "invalid", {
      detail: "createUpdaterArtifactsMustBeTrue",
      valueType: typeof value,
    });
  }

  return buildCheck("bundleCreateUpdaterArtifacts", "bundle", "pass");
}

function evaluateUpdaterPubkey(config) {
  const value = config?.plugins?.updater?.pubkey;

  if (!hasText(value)) {
    return buildCheck("updaterPubkey", "plugins.updater", "missing", {
      detail: "pubkeyMissing",
    });
  }

  if (looksLikePrivateKey(value)) {
    return buildCheck("updaterPubkey", "plugins.updater", "invalid", {
      detail: "pubkeyLooksPrivate",
    });
  }

  if (looksLikePath(value)) {
    return buildCheck("updaterPubkey", "plugins.updater", "invalid", {
      detail: "pubkeyMustBeContentNotPath",
    });
  }

  if (looksLikePlaceholder(value)) {
    return buildCheck("updaterPubkey", "plugins.updater", "invalid", {
      detail: "pubkeyLooksPlaceholder",
    });
  }

  return buildCheck("updaterPubkey", "plugins.updater", "pass");
}

function evaluateUpdaterEndpoints(config) {
  const endpoints = config?.plugins?.updater?.endpoints;

  if (!Array.isArray(endpoints) || endpoints.length === 0) {
    return buildCheck("updaterEndpoints", "plugins.updater", "missing", {
      detail: "endpointsMissing",
      endpointCount: 0,
    });
  }

  const invalidEndpointIndexes = endpoints.flatMap((endpoint, index) =>
    isValidProductionEndpoint(endpoint) ? [] : [index],
  );

  if (invalidEndpointIndexes.length > 0) {
    return buildCheck("updaterEndpoints", "plugins.updater", "invalid", {
      detail: "endpointsMustBeHttpsUrls",
      endpointCount: endpoints.length,
      invalidEndpointIndexes,
    });
  }

  return buildCheck("updaterEndpoints", "plugins.updater", "pass", {
    endpointCount: endpoints.length,
  });
}

function evaluateDangerousTransport(config) {
  const enabled = config?.plugins?.updater?.dangerousInsecureTransportProtocol === true;

  if (enabled) {
    return buildCheck("updaterDangerousTransport", "plugins.updater", "invalid", {
      detail: "dangerousInsecureTransportProtocolEnabled",
    });
  }

  return buildCheck("updaterDangerousTransport", "plugins.updater", "pass");
}

function evaluateWindowsInstallMode(config) {
  const value = config?.plugins?.updater?.windows?.installMode;

  if (value === undefined) {
    return buildCheck("updaterWindowsInstallMode", "plugins.updater.windows", "pass", {
      detail: "defaultInstallMode",
    });
  }

  if (!WINDOWS_INSTALL_MODES.has(value)) {
    return buildCheck("updaterWindowsInstallMode", "plugins.updater.windows", "invalid", {
      detail: "unsupportedInstallMode",
    });
  }

  return buildCheck("updaterWindowsInstallMode", "plugins.updater.windows", "pass");
}

function buildCheck(id, category, status, extra = {}) {
  return {
    id,
    category,
    source: TAURI_CONFIG_SOURCE,
    status,
    ...extra,
  };
}

function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function looksLikePrivateKey(value) {
  return /PRIVATE KEY/i.test(value);
}

function looksLikePath(value) {
  return /[\\/]/.test(value) || /\.(pem|key|pub)$/i.test(value.trim());
}

function looksLikePlaceholder(value) {
  return /CONTENT FROM PUBLICKEY\.PEM|TODO|CHANGE_ME|PLACEHOLDER/i.test(value);
}

function isValidProductionEndpoint(value) {
  if (!hasText(value)) {
    return false;
  }

  try {
    return new URL(value).protocol === "https:";
  } catch {
    return false;
  }
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0 || value.startsWith("--")) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseUpdaterConfigPreflightArgs(process.argv.slice(2));
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  const workspaceRoot = join(appRoot, "..", "..");
  const report = evaluateUpdaterConfigPreflight(readTauriConfig(workspaceRoot));
  writeUpdaterConfigPreflightOutputFile(options.outFile, report);
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(exitCodeForUpdaterConfigPreflight(report, options));
}
