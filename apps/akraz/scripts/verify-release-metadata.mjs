import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const RELEASE_METADATA_SCHEMA_VERSION = "akraz.releaseMetadata/v1";

const AKRAZ_LOCK_PACKAGE_PREFIX = "akraz";
const EXPECTED_REPOSITORY_URL = "https://github.com/0disoft/akraz";
const SEMVER_PATTERN = /^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/;

export function evaluateReleaseMetadataVersions(metadata) {
  const expectedVersion = metadata.rootPackageVersion;
  const checks = [
    checkVersion("rootPackage", "package.json", metadata.rootPackageVersion, expectedVersion),
    checkVersion(
      "appPackage",
      "apps/akraz/package.json",
      metadata.appPackageVersion,
      expectedVersion,
    ),
    checkVersion(
      "tauriConfig",
      "apps/akraz/src-tauri/tauri.conf.json",
      metadata.tauriConfigVersion,
      expectedVersion,
    ),
    checkVersion("cargoWorkspace", "Cargo.toml", metadata.cargoWorkspaceVersion, expectedVersion),
    checkExactValue(
      "cargoWorkspaceRepository",
      "Cargo.toml",
      metadata.cargoWorkspaceRepository,
      EXPECTED_REPOSITORY_URL,
    ),
    checkCargoLockPackages(metadata.cargoLockPackages, expectedVersion),
  ];

  return {
    schemaVersion: RELEASE_METADATA_SCHEMA_VERSION,
    ready: Boolean(expectedVersion) && SEMVER_PATTERN.test(expectedVersion) && checks.every(isPass),
    expectedVersion,
    checks,
  };
}

export function readReleaseMetadata(workspaceRoot) {
  const rootPackage = readJson(join(workspaceRoot, "package.json"));
  const appPackage = readJson(join(workspaceRoot, "apps", "akraz", "package.json"));
  const tauriConfig = readJson(
    join(workspaceRoot, "apps", "akraz", "src-tauri", "tauri.conf.json"),
  );
  const cargoToml = readFileSync(join(workspaceRoot, "Cargo.toml"), "utf8");
  const cargoLock = readFileSync(join(workspaceRoot, "Cargo.lock"), "utf8");

  return {
    rootPackageVersion: rootPackage.version,
    appPackageVersion: appPackage.version,
    tauriConfigVersion: tauriConfig.version,
    cargoWorkspaceVersion: parseCargoWorkspacePackageVersion(cargoToml),
    cargoWorkspaceRepository: parseCargoWorkspacePackageRepository(cargoToml),
    cargoLockPackages: parseCargoLockPackages(cargoLock),
  };
}

export function parseCargoWorkspacePackageVersion(cargoToml) {
  return parseCargoWorkspacePackageField(cargoToml, "version");
}

export function parseCargoWorkspacePackageRepository(cargoToml) {
  return parseCargoWorkspacePackageField(cargoToml, "repository");
}

function parseCargoWorkspacePackageField(cargoToml, fieldName) {
  let inWorkspacePackage = false;
  const fieldPattern = new RegExp(`^${fieldName}\\s*=\\s*"(?<value>[^"]+)"$`);

  for (const line of cargoToml.split(/\r?\n/)) {
    if (line.trim() === "[workspace.package]") {
      inWorkspacePackage = true;
      continue;
    }

    if (inWorkspacePackage && line.trimStart().startsWith("[")) {
      return undefined;
    }

    if (inWorkspacePackage) {
      const match = line.match(fieldPattern);
      if (match?.groups?.value) {
        return match.groups.value;
      }
    }
  }

  return undefined;
}

function checkExactValue(id, source, actualValue, expectedValue) {
  if (!actualValue) {
    return {
      id,
      source,
      status: "missing",
      expectedValue,
    };
  }

  return {
    id,
    source,
    status: actualValue === expectedValue ? "pass" : "mismatch",
    expectedValue,
    actualValue,
  };
}

export function parseCargoLockPackages(cargoLock) {
  return cargoLock
    .split(/\r?\n(?=\[\[package\]\])/)
    .map(parseCargoLockPackageBlock)
    .filter((entry) => entry.name?.startsWith(AKRAZ_LOCK_PACKAGE_PREFIX));
}

function parseCargoLockPackageBlock(block) {
  const name = block.match(/^name\s*=\s*"(?<name>[^"]+)"$/m);
  const version = block.match(/^version\s*=\s*"(?<version>[^"]+)"$/m);
  return {
    name: name?.groups?.name,
    version: version?.groups?.version,
  };
}

function checkVersion(id, source, actualVersion, expectedVersion) {
  if (!actualVersion) {
    return {
      id,
      source,
      status: "missing",
      expectedVersion,
    };
  }

  if (!SEMVER_PATTERN.test(actualVersion)) {
    return {
      id,
      source,
      status: "invalid",
      expectedVersion,
      actualVersion,
      detail: "version is not SemVer",
    };
  }

  return {
    id,
    source,
    status: actualVersion === expectedVersion ? "pass" : "mismatch",
    expectedVersion,
    actualVersion,
  };
}

function checkCargoLockPackages(packages, expectedVersion) {
  if (!packages.length) {
    return {
      id: "cargoLock",
      source: "Cargo.lock",
      status: "missing",
      expectedVersion,
      packages: [],
      detail: "no Akraz packages found in Cargo.lock",
    };
  }

  const packageChecks = packages.map((entry) =>
    checkVersion(`cargoLock:${entry.name}`, "Cargo.lock", entry.version, expectedVersion),
  );

  return {
    id: "cargoLock",
    source: "Cargo.lock",
    status: packageChecks.every(isPass) ? "pass" : "mismatch",
    expectedVersion,
    packages: packageChecks,
  };
}

function isPass(check) {
  return check.status === "pass";
}

function readJson(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

if (import.meta.main) {
  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const appRoot = dirname(scriptDir);
  const workspaceRoot = join(appRoot, "..", "..");
  const report = evaluateReleaseMetadataVersions(readReleaseMetadata(workspaceRoot));
  process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
  process.exit(report.ready ? 0 : 1);
}
