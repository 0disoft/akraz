import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { join } from "node:path";

import {
  evaluateReleaseMetadataVersions,
  parseCargoLockPackages,
  parseCargoWorkspacePackageVersion,
} from "../scripts/verify-release-metadata.mjs";

function runAppPackageScript(scriptName, args = []) {
  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    windowsHide: true,
  });
}

const releaseMetadataFixture = {
  rootPackageVersion: "0.4.69",
  appPackageVersion: "0.4.69",
  tauriConfigVersion: "0.4.69",
  cargoWorkspaceVersion: "0.4.69",
  cargoLockPackages: [
    { name: "akraz-app", version: "0.4.69" },
    { name: "akraz-core", version: "0.4.69" },
    { name: "akraz-daemon", version: "0.4.69" },
    { name: "akrazctl", version: "0.4.69" },
  ],
};

describe("release metadata verification", () => {
  test("accepts synchronized release versions", () => {
    const report = evaluateReleaseMetadataVersions(releaseMetadataFixture);

    expect(report.ready).toBe(true);
    expect(report.expectedVersion).toBe("0.4.69");
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
  });

  test("reports mismatched package and lockfile versions", () => {
    const report = evaluateReleaseMetadataVersions({
      ...releaseMetadataFixture,
      tauriConfigVersion: "0.4.68",
      cargoLockPackages: [
        { name: "akraz-app", version: "0.4.69" },
        { name: "akraz-core", version: "0.4.68" },
      ],
    });
    const tauriCheck = report.checks.find((check) => check.id === "tauriConfig");
    const cargoLockCheck = report.checks.find((check) => check.id === "cargoLock");

    expect(report.ready).toBe(false);
    expect(tauriCheck?.status).toBe("mismatch");
    expect(cargoLockCheck?.status).toBe("mismatch");
    expect(cargoLockCheck?.packages.some((entry) => entry.status === "mismatch")).toBe(true);
  });

  test("requires SemVer metadata and Cargo.lock Akraz packages", () => {
    const report = evaluateReleaseMetadataVersions({
      rootPackageVersion: "next",
      appPackageVersion: "next",
      tauriConfigVersion: "next",
      cargoWorkspaceVersion: undefined,
      cargoLockPackages: [],
    });

    expect(report.ready).toBe(false);
    expect(report.checks.map((check) => check.status)).toEqual([
      "invalid",
      "invalid",
      "invalid",
      "missing",
      "missing",
    ]);
  });

  test("parses Cargo workspace package version", () => {
    expect(
      parseCargoWorkspacePackageVersion(`
[workspace]
members = []

[workspace.package]
version = "0.4.69"
edition = "2024"
`),
    ).toBe("0.4.69");
  });

  test("parses Akraz packages from Cargo.lock", () => {
    expect(
      parseCargoLockPackages(`
[[package]]
name = "akraz-app"
version = "0.4.69"

[[package]]
name = "serde"
version = "1.0.0"

[[package]]
name = "akraz-core"
version = "0.4.69"

[[package]]
name = "akrazctl"
version = "0.4.69"
`),
    ).toEqual([
      { name: "akraz-app", version: "0.4.69" },
      { name: "akraz-core", version: "0.4.69" },
      { name: "akrazctl", version: "0.4.69" },
    ]);
  });

  test("verifies synchronized release metadata through the app package script", () => {
    const result = runAppPackageScript("verify:release-metadata");
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.ready).toBe(true);
    expect(report.schemaVersion).toBe("akraz.releaseMetadata/v1");
    expect(report.expectedVersion).toBe("0.5.2");
    expect(report.checks.every((check) => check.status === "pass")).toBe(true);
  });
});
