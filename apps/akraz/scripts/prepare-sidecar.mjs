import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const release = process.argv.includes("--release");
const profile = release ? "release" : "debug";
const extension = process.platform === "win32" ? ".exe" : "";

const targetTriple = execFileSync("rustc", ["--print", "host-tuple"], {
  cwd: workspaceRoot,
  encoding: "utf8",
}).trim();

if (!targetTriple) {
  throw new Error("failed to determine Rust host target triple");
}

const cargoArgs = ["build", "-p", "akraz-daemon"];
if (release) {
  cargoArgs.push("--release");
}

execFileSync("cargo", cargoArgs, {
  cwd: workspaceRoot,
  stdio: "inherit",
});

const source = join(workspaceRoot, "target", profile, `akraz-daemon${extension}`);
const sidecarDir = join(appRoot, "src-tauri", "binaries");
const destination = join(sidecarDir, `akraz-daemon-${targetTriple}${extension}`);

mkdirSync(sidecarDir, { recursive: true });
copyFileSync(source, destination);
console.log(`Prepared ${destination}`);
