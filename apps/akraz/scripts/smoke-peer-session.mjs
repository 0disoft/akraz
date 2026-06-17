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
  ["run", "-p", "akraz-daemon", "--", "--akraz-smoke-peer-session"],
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
  throw new Error(`peer session smoke failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error("peer session smoke did not emit a JSON report");
}

const report = JSON.parse(reportLine);
if (report.daemonVersion !== appPackage.version) {
  throw new Error(
    `peer session smoke returned daemon version ${report.daemonVersion}, expected ${appPackage.version}`,
  );
}

if (
  report.hello?.protocolMajor !== 1 ||
  report.hello.protocolMinor !== 4 ||
  report.hello.deviceId !== "local-smoke-device" ||
  report.hello.peerId !== "loopback-peer"
) {
  throw new Error("peer session smoke reported unexpected hello frame");
}

const [start, forward, release, stop] = report.commands ?? [];
assertKind(start, "startRemoteSession", "first command");
assertKind(forward, "forwardInput", "second command");
assertKind(release, "releaseAllInputs", "third command");
assertKind(stop, "stopRemoteSession", "fourth command");

if (start.peerId !== "loopback-peer" || start.crossing?.localEdge !== "right") {
  throw new Error("peer session smoke reported unexpected remote start command");
}
if (
  forward.event?.kind !== "pointerMoved" ||
  forward.event.deltaX !== 8 ||
  forward.event.deltaY !== 2
) {
  throw new Error("peer session smoke reported unexpected forward input command");
}
if (stop.sessionId !== "loopback-session") {
  throw new Error("peer session smoke reported unexpected stop session command");
}

console.log("Peer session smoke passed.");

function assertKind(command, expected, label) {
  if (command?.kind !== expected) {
    throw new Error(`peer session smoke expected ${label} ${expected}, got ${command?.kind}`);
  }
}
