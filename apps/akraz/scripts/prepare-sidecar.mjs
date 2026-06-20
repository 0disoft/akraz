import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export const AKRAZ_DAEMON_PACKAGE_NAME = "akraz-daemon";
export const AKRAZ_DAEMON_BINARY_NAME = "akraz-daemon";
export const TAURI_SIDECAR_EXTERNAL_BIN = "binaries/akraz-daemon";
export const WINDOWS_CI_TARGET_TRIPLE = "x86_64-pc-windows-msvc";
export const WINDOWS_CI_SIDECAR_FILE_NAME = sidecarFileName(WINDOWS_CI_TARGET_TRIPLE, "win32");

export function parsePrepareSidecarArgs(args) {
  return {
    release: args.includes("--release"),
  };
}

export function buildCargoArgs(options = {}) {
  const cargoArgs = ["build", "-p", AKRAZ_DAEMON_PACKAGE_NAME];
  if (options.release) {
    cargoArgs.push("--release");
  }
  return cargoArgs;
}

export function sidecarFileName(targetTriple, platform = process.platform) {
  return `${AKRAZ_DAEMON_BINARY_NAME}-${targetTriple}${binaryExtension(platform)}`;
}

export function buildSidecarPaths({
  appRoot = currentAppRoot(),
  platform = process.platform,
  release = false,
  targetTriple,
  workspaceRoot = currentWorkspaceRoot(appRoot),
}) {
  if (!targetTriple) {
    throw new Error("targetTriple is required");
  }

  const profile = release ? "release" : "debug";
  const source = join(
    workspaceRoot,
    "target",
    profile,
    `${AKRAZ_DAEMON_BINARY_NAME}${binaryExtension(platform)}`,
  );
  const sidecarDir = join(appRoot, "src-tauri", "binaries");
  const destination = join(sidecarDir, sidecarFileName(targetTriple, platform));

  return {
    destination,
    sidecarDir,
    source,
  };
}

export function resolveHostTargetTriple(workspaceRoot = currentWorkspaceRoot()) {
  const targetTriple = execFileSync("rustc", ["--print", "host-tuple"], {
    cwd: workspaceRoot,
    encoding: "utf8",
  }).trim();

  if (!targetTriple) {
    throw new Error("failed to determine Rust host target triple");
  }

  return targetTriple;
}

export function prepareSidecar(options = {}) {
  const appRoot = options.appRoot ?? currentAppRoot();
  const workspaceRoot = options.workspaceRoot ?? currentWorkspaceRoot(appRoot);
  const targetTriple = options.targetTriple ?? resolveHostTargetTriple(workspaceRoot);
  const paths = buildSidecarPaths({
    appRoot,
    platform: options.platform ?? process.platform,
    release: options.release,
    targetTriple,
    workspaceRoot,
  });

  execFileSync("cargo", buildCargoArgs(options), {
    cwd: workspaceRoot,
    stdio: "inherit",
  });

  mkdirSync(paths.sidecarDir, { recursive: true });
  copyFileSync(paths.source, paths.destination);
  return paths.destination;
}

function binaryExtension(platform) {
  return platform === "win32" ? ".exe" : "";
}

function currentAppRoot() {
  return dirname(dirname(fileURLToPath(import.meta.url)));
}

function currentWorkspaceRoot(appRoot = currentAppRoot()) {
  return join(appRoot, "..", "..");
}

if (import.meta.main) {
  const destination = prepareSidecar(parsePrepareSidecarArgs(process.argv.slice(2)));
  console.log(`Prepared ${destination}`);
}
