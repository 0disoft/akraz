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
  ["run", "-p", "akraz-daemon", "--", "--akraz-smoke-loopback-transport"],
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
  throw new Error(`loopback transport smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("loopback transport smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
if (report.daemonVersion !== appPackage.version) {
  throw new Error(
    `loopback transport smoke returned daemon version ${report.daemonVersion}, expected ${appPackage.version}`,
  );
}

const [start, forward, release, stop] = report.commands ?? [];
assertKind(start, "startRemoteSession", "first command");
assertKind(forward, "forwardInput", "second command");
assertKind(release, "releaseAllInputs", "third command");
assertKind(stop, "stopRemoteSession", "fourth command");

if (start.peerId !== "loopback-peer" || start.crossing?.localEdge !== "right") {
  throw new Error("loopback transport smoke reported unexpected remote start command");
}
if (
  forward.event?.kind !== "pointerMoved" ||
  forward.event.deltaX !== 8 ||
  forward.event.deltaY !== 2
) {
  throw new Error("loopback transport smoke reported unexpected forward input command");
}
if (stop.sessionId !== "loopback-session") {
  throw new Error("loopback transport smoke reported unexpected stop session command");
}

console.log("Loopback transport smoke passed.");

function assertKind(command, expected, label) {
  if (command?.kind !== expected) {
    throw new Error(`loopback transport smoke expected ${label} ${expected}, got ${command?.kind}`);
  }
}
