import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
  EXPECTED_LINUX_DEB_PRIORITY,
  EXPECTED_LINUX_DEB_SECTION,
  LINUX_DEB_PACKAGING_PREFLIGHT_SCHEMA_VERSION,
  REQUIRED_LINUX_DEB_DEPENDS,
  buildLinuxDebPackagingPreflightReport,
  exitCodeForLinuxDebPackagingPreflight,
  parseCargoWorkspacePackageVersion,
} from "../scripts/linux-deb-packaging-preflight.mjs";

function runAppPackageScript(scriptName, args = []) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

describe("Linux .deb packaging preflight", () => {
  test("accepts the repository Linux .deb draft metadata", () => {
    const report = buildLinuxDebPackagingPreflightReport();

    expect(report.schemaVersion).toBe(LINUX_DEB_PACKAGING_PREFLIGHT_SCHEMA_VERSION);
    expect(report.releaseTarget).toBe("Linux X11 .deb draft");
    expect(report.ready).toBe(true);
    expect(report.requiredDebDepends).toEqual(REQUIRED_LINUX_DEB_DEPENDS);
    expect(report.checks.find((check) => check.id === "debDepends")).toMatchObject({
      status: "pass",
      depends: REQUIRED_LINUX_DEB_DEPENDS,
    });
    expect(report.checks.find((check) => check.id === "debMetadata")).toMatchObject({
      status: "pass",
      section: EXPECTED_LINUX_DEB_SECTION,
      priority: EXPECTED_LINUX_DEB_PRIORITY,
    });
    expect(report.manualVerification.map((entry) => entry.id)).toEqual([
      "linuxDebBuild",
      "linuxX11RuntimeSmoke",
    ]);
    expect(report.privacy).toEqual({
      includesSecretValues: false,
      includesFullFilePaths: false,
      includesBuildLogs: false,
    });
    expect(exitCodeForLinuxDebPackagingPreflight(report)).toBe(0);
  });

  test("verifies Linux .deb metadata through the app package script", () => {
    const result = runAppPackageScript("release:linux-deb-preflight");
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.schemaVersion).toBe(LINUX_DEB_PACKAGING_PREFLIGHT_SCHEMA_VERSION);
    expect(report.ready).toBe(true);
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
    expect(result.stdout.endsWith("\n")).toBe(true);
  });

  test("rejects missing X11 runtime dependencies before a .deb build", () => {
    const tempDirectory = writeWorkspaceFixture();
    try {
      const tauriConfigPath = join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json");
      const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
      tauriConfig.bundle.linux.deb.depends = ["libx11-6"];
      writeJson(tauriConfigPath, tauriConfig);

      const report = buildLinuxDebPackagingPreflightReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "debDepends")).toMatchObject({
        status: "invalid",
        detail: "linuxX11RuntimeLibrariesMissingFromDebDepends",
        missingDepends: ["libxi6", "libxtst6", "libxrandr2"],
      });
      expect(report.nextActions).toContainEqual({
        id: "addLinuxX11DebDepends",
        action: "add the Linux X11 runtime libraries to Tauri deb depends",
        missingDepends: ["libxi6", "libxtst6", "libxrandr2"],
      });
      expect(exitCodeForLinuxDebPackagingPreflight(report)).toBe(1);
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("rejects maintainer scripts in the draft package lifecycle", () => {
    const tempDirectory = writeWorkspaceFixture();
    try {
      const tauriConfigPath = join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json");
      const tauriConfig = JSON.parse(readFileSync(tauriConfigPath, "utf8"));
      tauriConfig.bundle.linux.deb.postInstallScript = "scripts/linux/postinstall.sh";
      writeJson(tauriConfigPath, tauriConfig);

      const report = buildLinuxDebPackagingPreflightReport(tempDirectory);

      expect(report.ready).toBe(false);
      expect(report.checks.find((check) => check.id === "maintainerScripts")).toMatchObject({
        status: "invalid",
        detail: "debDraftMustNotRunMaintainerScriptsWithoutReview",
        configuredScripts: ["postInstallScript"],
      });
      expect(report.nextActions).toContainEqual({
        id: "reviewMaintainerScripts",
        action: "remove Debian maintainer scripts or add a dedicated reviewed install lifecycle",
        configuredScripts: ["postInstallScript"],
      });
    } finally {
      rmSync(tempDirectory, { force: true, recursive: true });
    }
  });

  test("parses Cargo workspace package version", () => {
    expect(
      parseCargoWorkspacePackageVersion(`
[workspace]
members = []

[workspace.package]
version = "0.22.4"
edition = "2024"
`),
    ).toBe("0.22.4");
  });
});

function writeWorkspaceFixture() {
  const tempDirectory = mkdtempSync(join(tmpdir(), "akraz-linux-deb-"));
  const packageRoot = process.cwd();

  mkdirSync(join(tempDirectory, "apps", "akraz", "src-tauri"), { recursive: true });
  writeFileSync(
    join(tempDirectory, "package.json"),
    readFileSync(join(packageRoot, "package.json"), "utf8"),
    "utf8",
  );
  writeFileSync(
    join(tempDirectory, "Cargo.toml"),
    readFileSync(join(packageRoot, "Cargo.toml"), "utf8"),
    "utf8",
  );
  writeFileSync(
    join(tempDirectory, "apps", "akraz", "package.json"),
    readFileSync(join(packageRoot, "apps", "akraz", "package.json"), "utf8"),
    "utf8",
  );
  writeFileSync(
    join(tempDirectory, "apps", "akraz", "src-tauri", "tauri.conf.json"),
    readFileSync(join(packageRoot, "apps", "akraz", "src-tauri", "tauri.conf.json"), "utf8"),
    "utf8",
  );

  return tempDirectory;
}

function writeJson(path: string, payload: unknown) {
  writeFileSync(path, `${JSON.stringify(payload, null, 2)}\n`, "utf8");
}
