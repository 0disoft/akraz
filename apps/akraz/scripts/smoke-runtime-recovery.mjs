import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));

const smoke = spawnSync(
  "cargo",
  ["run", "-p", "akraz-daemon", "--", "--akraz-smoke-runtime-recovery"],
  {
    cwd: workspaceRoot,
    encoding: "utf8",
  },
);

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
  throw new Error(`runtime recovery smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("runtime recovery smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
if (report.daemonVersion !== appPackage.version) {
  throw new Error(
    `runtime recovery smoke returned daemon version ${report.daemonVersion}, expected ${appPackage.version}`,
  );
}

if (report.systemResume?.recovered !== true) {
  throw new Error("runtime recovery smoke did not report system resume recovery");
}
if (report.systemResume.finalMode !== "Local") {
  throw new Error(`runtime recovery smoke final mode was ${report.systemResume.finalMode}`);
}
assertStringArrayEquals(report.systemResume.coreActions, [
  "releaseLocalInputs",
  "releaseAllInputs",
  "stopRemoteSession",
]);
if (
  report.systemResume.localReleaseAllCount !== 1 ||
  report.systemResume.remoteReleaseAllCommands !== 1 ||
  report.systemResume.remoteStopSessionCommands !== 1
) {
  throw new Error("runtime recovery smoke reported incomplete system resume release evidence");
}

if (report.inputCaptureIdleWatchdog?.recovered !== true) {
  throw new Error("runtime recovery smoke did not report input capture idle watchdog recovery");
}
if (!report.inputCaptureIdleWatchdog.dispatchedActions?.includes("inputCaptureIdle")) {
  throw new Error("runtime recovery smoke did not dispatch input capture idle action");
}
if (!report.inputCaptureIdleWatchdog.logEvents?.includes("input.capture.idle")) {
  throw new Error("runtime recovery smoke did not record input capture idle log evidence");
}

console.log("Runtime recovery smoke passed.");

function assertStringArrayEquals(actual, expected) {
  if (!Array.isArray(actual) || JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(
      `runtime recovery smoke reported ${JSON.stringify(actual)}, expected ${JSON.stringify(
        expected,
      )}`,
    );
  }
}
