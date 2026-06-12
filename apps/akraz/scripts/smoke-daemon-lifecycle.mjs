import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const extension = process.platform === "win32" ? ".exe" : "";
const smokeFlag = "--akraz-smoke-daemon-lifecycle";
const configuredApp = process.env.AKRAZ_SMOKE_APP;
const appExecutable = configuredApp || join(workspaceRoot, "target", "debug", `akraz${extension}`);

if (configuredApp) {
  if (!existsSync(configuredApp)) {
    throw new Error(`AKRAZ_SMOKE_APP does not exist: ${configuredApp}`);
  }
} else {
  execFileSync("bun", ["run", "prepare:sidecar"], {
    cwd: appRoot,
    stdio: "inherit",
  });
  execFileSync("cargo", ["build", "-p", "akraz-app"], {
    cwd: workspaceRoot,
    stdio: "inherit",
  });
}

const smoke = spawnSync(appExecutable, [smokeFlag], {
  cwd: workspaceRoot,
  encoding: "utf8",
});

if (smoke.stdout) {
  process.stdout.write(smoke.stdout);
}
if (smoke.stderr) {
  process.stderr.write(smoke.stderr);
}
if (smoke.error) {
  throw smoke.error;
}
if (smoke.status !== 0) {
  throw new Error(`daemon lifecycle smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("daemon lifecycle smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
assertPhase(report.initial?.phase, "not_running", "initial");
assertPhase(report.started?.phase, "running", "started");
assertPhase(report.stopped?.phase, "not_running", "stopped");

const daemonVersion = report.started?.status?.daemonVersion;
if (daemonVersion !== appPackage.version) {
  throw new Error(
    `daemon lifecycle smoke returned daemon version ${daemonVersion}, expected ${appPackage.version}`,
  );
}

console.log("Daemon lifecycle smoke passed.");

function assertPhase(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`daemon lifecycle smoke expected ${label} phase ${expected}, got ${actual}`);
  }
}
