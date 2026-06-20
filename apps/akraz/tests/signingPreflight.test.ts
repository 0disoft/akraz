import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

import { describe, expect, test } from "bun:test";

import {
  evaluateSigningPreflight,
  exitCodeForSigningPreflight,
  parseSigningPreflightArgs,
  writeSigningPreflightOutputFile,
} from "../scripts/smoke-signing-preflight.mjs";

function runAppPackageScript(scriptName, args, envOverrides = {}) {
  const env = { ...process.env, ...envOverrides };

  return spawnSync(process.execPath, ["run", scriptName, "--", ...args], {
    cwd: join(import.meta.dir, ".."),
    encoding: "utf8",
    env,
    windowsHide: true,
  });
}

describe("signing preflight", () => {
  test("reports missing release signing inputs without secret values", () => {
    const report = evaluateSigningPreflight({});
    const formatted = JSON.stringify(report);

    expect(report.ready).toBe(false);
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(report.privacy.includesFullFilePaths).toBe(false);
    expect(report.checks.map((check) => check.status)).toEqual([
      "missing",
      "missing",
      "missing",
      "missing",
    ]);
    expect(formatted).toContain("TAURI_SIGNING_PRIVATE_KEY");
    expect(formatted).not.toContain("super-secret");
  });

  test("accepts complete updater and Windows installer signing inputs", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-signing-preflight-"));
    const certificatePath = join(tempDir, "codesign.pfx");

    try {
      writeFileSync(certificatePath, "placeholder certificate fixture");

      const report = evaluateSigningPreflight({
        TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
        TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
        AKRAZ_WINDOWS_SIGNING_CERT_PATH: certificatePath,
        AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
      });
      const formatted = JSON.stringify(report);

      expect(report.ready).toBe(true);
      expect(report.checks.every((check) => check.status === "pass")).toBe(true);
      expect(formatted).not.toContain("super-secret");
      expect(formatted).not.toContain(certificatePath);
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });

  test("rejects a missing certificate path without printing the path", () => {
    const certificatePath = join(tmpdir(), "missing-akraz-codesign.pfx");
    const report = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: certificatePath,
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });
    const certificateCheck = report.checks.find(
      (check) => check.id === "windowsSigningCertificate",
    );

    expect(report.ready).toBe(false);
    expect(certificateCheck?.status).toBe("invalid");
    expect(JSON.stringify(report)).not.toContain(certificatePath);
  });

  test("uses expect-missing mode for no-secret smoke environments", () => {
    const missingReport = evaluateSigningPreflight({});
    const invalidReport = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: join(tmpdir(), "missing-akraz-codesign.pfx"),
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });
    const readyReport = evaluateSigningPreflight({
      TAURI_SIGNING_PRIVATE_KEY: "super-secret-updater-key",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "super-secret-updater-password",
      AKRAZ_WINDOWS_SIGNING_CERT_BASE64: "super-secret-cert-data",
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "super-secret-cert-password",
    });

    expect(parseSigningPreflightArgs(["--expect-missing"]).expectMissing).toBe(true);
    expect(exitCodeForSigningPreflight(missingReport, { expectMissing: true })).toBe(0);
    expect(exitCodeForSigningPreflight(invalidReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForSigningPreflight(readyReport, { expectMissing: true })).toBe(1);
    expect(exitCodeForSigningPreflight(readyReport)).toBe(0);
  });

  test("runs expect-missing smoke through the app package script", () => {
    const result = runAppPackageScript("smoke:signing-preflight", [], {
      AKRAZ_WINDOWS_SIGNING_CERT_BASE64: "",
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: "",
      TAURI_SIGNING_PRIVATE_KEY: "",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
    });
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(0);
    expect(report.ready).toBe(false);
    expect(report.checks.map((check) => check.status)).toEqual([
      "missing",
      "missing",
      "missing",
      "missing",
    ]);
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(result.stdout).not.toContain("super-secret");
  });

  test("fails closed through the release package script without signing inputs", () => {
    const result = runAppPackageScript("release:signing-preflight", [], {
      AKRAZ_WINDOWS_SIGNING_CERT_BASE64: "",
      AKRAZ_WINDOWS_SIGNING_CERT_PASSWORD: "",
      AKRAZ_WINDOWS_SIGNING_CERT_PATH: "",
      TAURI_SIGNING_PRIVATE_KEY: "",
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "",
    });
    const report = JSON.parse(result.stdout);

    expect(result.status).toBe(1);
    expect(report.ready).toBe(false);
    expect(report.schemaVersion).toBe("akraz.signing.preflight/v1");
    expect(report.checks.map((check) => check.status)).toEqual([
      "missing",
      "missing",
      "missing",
      "missing",
    ]);
    expect(report.privacy.includesSecretValues).toBe(false);
    expect(report.privacy.includesFullFilePaths).toBe(false);
    expect(result.stdout).not.toContain("super-secret");
  });

  test("parses output file arguments and writes atomic JSON evidence", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "akraz-signing-preflight-out-"));
    const outputFile = join(tempDir, "nested", "signing.json");

    try {
      expect(parseSigningPreflightArgs(["--expect-missing", "--out-file", outputFile])).toEqual({
        expectMissing: true,
        outFile: outputFile,
      });

      const report = evaluateSigningPreflight({});
      const written = writeSigningPreflightOutputFile(outputFile, report);

      expect(written).toBe(outputFile);
      expect(readFileSync(outputFile, "utf8").endsWith("\n")).toBe(true);
      expect(JSON.parse(readFileSync(outputFile, "utf8"))).toEqual(report);
      expect(() => parseSigningPreflightArgs(["--out-file"])).toThrow(
        "--out-file requires a non-empty value",
      );
      expect(() => parseSigningPreflightArgs(["--unknown"])).toThrow(
        "unknown signing preflight argument: --unknown",
      );
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });
});
