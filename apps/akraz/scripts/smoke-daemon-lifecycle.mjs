import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRoot = dirname(scriptDir);
const workspaceRoot = join(appRoot, "..", "..");
const appPackage = JSON.parse(readFileSync(join(appRoot, "package.json"), "utf8"));
const extension = process.platform === "win32" ? ".exe" : "";
const smokeProfiles = {
  lifecycle: {
    flag: "--akraz-smoke-daemon-lifecycle",
    label: "daemon lifecycle smoke",
    expectSettings: false,
  },
  "settings-start": {
    flag: "--akraz-smoke-settings-start",
    label: "daemon settings start smoke",
    expectSettings: true,
  },
};
const smokeMode = process.argv[2] ?? "lifecycle";
const smokeProfile = smokeProfiles[smokeMode];
if (!smokeProfile) {
  throw new Error(`unknown daemon smoke mode: ${smokeMode}`);
}
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

const smoke = spawnSync(appExecutable, [smokeProfile.flag], {
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
  throw new Error(`${smokeProfile.label} failed with exit code ${smoke.status}`);
}

const reportLine = smoke.stdout.split(/\r?\n/).findLast((line) => line.trimStart().startsWith("{"));
if (!reportLine) {
  throw new Error(`${smokeProfile.label} did not emit a JSON report`);
}

const report = JSON.parse(reportLine);
assertPhase(report.initial?.phase, "not_running", "initial");
assertPhase(report.started?.phase, "running", "started");
assertPhase(report.stopped?.phase, "not_running", "stopped");
assertGracefulStop(report);
assertPermissions(report.permissions);
if (smokeProfile.expectSettings) {
  assertSettings(report.settings);
}

const daemonVersion = report.started?.status?.daemonVersion;
if (daemonVersion !== appPackage.version) {
  throw new Error(
    `${smokeProfile.label} returned daemon version ${daemonVersion}, expected ${appPackage.version}`,
  );
}

console.log(`${smokeProfile.label[0].toUpperCase()}${smokeProfile.label.slice(1)} passed.`);

function assertPhase(actual, expected, label) {
  if (actual !== expected) {
    throw new Error(`${smokeProfile.label} expected ${label} phase ${expected}, got ${actual}`);
  }
}

function assertGracefulStop(smokeReport) {
  if (smokeReport.stopMethod !== "graceful_shutdown") {
    throw new Error(
      `${smokeProfile.label} expected graceful shutdown stop method, got ${smokeReport.stopMethod}`,
    );
  }
  if (!smokeReport.shutdown?.requested) {
    throw new Error(`${smokeProfile.label} did not report a requested daemon shutdown`);
  }
  if (typeof smokeReport.shutdown.releasedInputs !== "boolean") {
    throw new Error(`${smokeProfile.label} reported invalid shutdown releasedInputs`);
  }
  if (typeof smokeReport.shutdown.disconnectedPeerSession !== "boolean") {
    throw new Error(`${smokeProfile.label} reported invalid shutdown disconnectedPeerSession`);
  }
  if (typeof smokeReport.shutdown.mode !== "string" || smokeReport.shutdown.mode.length === 0) {
    throw new Error(`${smokeProfile.label} reported invalid shutdown mode`);
  }
}

function assertSettings(settings) {
  if (!settings) {
    throw new Error(`${smokeProfile.label} did not report saved settings`);
  }
  if (settings.captureInput !== true) {
    throw new Error(`${smokeProfile.label} expected captureInput true`);
  }
  if (settings.peerListenAddress !== "127.0.0.1:0") {
    throw new Error(`${smokeProfile.label} expected peerListenAddress 127.0.0.1:0`);
  }
  const [binding] = settings.edgeBindings ?? [];
  if (!binding || settings.edgeBindings.length !== 1) {
    throw new Error(`${smokeProfile.label} expected one edge binding`);
  }
  if (
    binding.localEdge !== "right" ||
    binding.peerId !== "linux-laptop" ||
    binding.remoteEdge !== "left"
  ) {
    throw new Error(`${smokeProfile.label} reported unexpected edge binding`);
  }
  const [manualAddress] = settings.manualPeerAddresses ?? [];
  if (!manualAddress || settings.manualPeerAddresses.length !== 1) {
    throw new Error(`${smokeProfile.label} expected one manual peer address`);
  }
  if (manualAddress.peerId !== "linux-laptop" || manualAddress.address !== "127.0.0.1:4455") {
    throw new Error(`${smokeProfile.label} reported unexpected manual peer address`);
  }
}

function assertPermissions(permissions) {
  if (!permissions) {
    throw new Error(`${smokeProfile.label} did not report permissions`);
  }
  if (typeof permissions.adapterName !== "string" || permissions.adapterName.length === 0) {
    throw new Error(`${smokeProfile.label} reported an invalid permission adapter`);
  }
  const capabilities = permissions.capabilities;
  const capabilityKeys = [
    "canCapturePointer",
    "canCaptureKeyboard",
    "canInjectPointer",
    "canInjectKeyboard",
  ];
  for (const key of capabilityKeys) {
    if (typeof capabilities?.[key] !== "boolean") {
      throw new Error(`${smokeProfile.label} reported invalid ${key} capability`);
    }
  }
  if (!Array.isArray(permissions.issues)) {
    throw new Error(`${smokeProfile.label} reported invalid permission issues`);
  }

  const missingCodes = new Set();
  if (!capabilities.canCapturePointer) {
    missingCodes.add("capture_pointer_unavailable");
  }
  if (!capabilities.canCaptureKeyboard) {
    missingCodes.add("capture_keyboard_unavailable");
  }
  if (!capabilities.canInjectPointer) {
    missingCodes.add("inject_pointer_unavailable");
  }
  if (!capabilities.canInjectKeyboard) {
    missingCodes.add("inject_keyboard_unavailable");
  }

  const reportedCodes = new Set();
  for (const issue of permissions.issues) {
    if (typeof issue?.code !== "string" || issue.code.length === 0) {
      throw new Error(`${smokeProfile.label} reported a permission issue without a code`);
    }
    if (typeof issue.message !== "string" || issue.message.length === 0) {
      throw new Error(`${smokeProfile.label} reported a permission issue without a message`);
    }
    reportedCodes.add(issue.code);
  }

  if (reportedCodes.size !== missingCodes.size) {
    throw new Error(`${smokeProfile.label} reported permission issue count drift`);
  }
  for (const code of missingCodes) {
    if (!reportedCodes.has(code)) {
      throw new Error(`${smokeProfile.label} did not report missing permission code ${code}`);
    }
  }
}
