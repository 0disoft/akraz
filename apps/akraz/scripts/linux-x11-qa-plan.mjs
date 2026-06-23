export const LINUX_X11_QA_PLAN_SCHEMA_VERSION = "akraz.linuxX11QaPlan/v1";

const linuxX11QaCases = [
  {
    id: "LX11-001",
    milestone: "M9",
    category: "cross-os-remote-control",
    title: "Windows source to Linux X11 target remote input",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Linux desktop on X11",
      sourceSession: "Windows desktop",
      targetSession: "X11",
      hardware: "Two physical machines on a trusted local network",
    },
    prerequisites: [
      "The Linux target is running an X11 session, not Wayland.",
      "The Linux target has XTEST injection available.",
      "Both devices are paired and trusted.",
      "A source edge binding is saved before the test.",
      "A non-sensitive Linux target window is focused before entering remote mode.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Capture source screen topology diagnostics.",
      "Capture Linux target input-adapter diagnostics.",
      "Cross the configured Windows source edge into the Linux X11 target.",
      "Move the pointer, click, drag, and scroll on the Linux target.",
      "Press and release basic non-text keys on the Linux target.",
      "Leave remote mode and verify local control on the Windows source.",
    ],
    expected: [
      "The pointer enters the Linux target from the configured edge.",
      "Pointer movement, buttons, scroll, and non-text key events are applied on the Linux target.",
      "The Windows source does not receive remote-mode input while remote mode is active.",
      "Leaving remote mode restores local Windows source control.",
    ],
    evidence: [
      "Windows source screen topology diagnostics artifact id",
      "Linux X11 target input-adapter diagnostics artifact id",
      "Manual Linux pointer, button, scroll, and non-text keyboard pass note",
      "Manual Windows source local-input-leak pass note",
    ],
  },
  {
    id: "LX11-002",
    milestone: "M9",
    category: "cross-os-remote-control",
    title: "Linux X11 source to Windows target remote input",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Linux desktop on X11",
      targetOs: "Windows 11",
      sourceSession: "X11",
      targetSession: "Windows desktop",
      hardware: "Two physical machines on a trusted local network",
    },
    prerequisites: [
      "The Linux source is running an X11 session, not Wayland.",
      "The Linux source has XInput2 capture diagnostics available.",
      "The Windows target can receive pointer, button, scroll, and keyboard injection.",
      "Both devices are paired and trusted.",
      "A Linux source edge binding is saved before the test.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Capture Linux source screen topology and XInput2 diagnostics.",
      "Cross the configured Linux source edge into the Windows target.",
      "Move the pointer, click, drag, and scroll on the Windows target.",
      "Press and release basic non-text keys on the Windows target.",
      "Leave remote mode and verify local control on the Linux source.",
    ],
    expected: [
      "The pointer enters the Windows target from the configured edge.",
      "Pointer movement, buttons, scroll, and non-text key events are applied on the Windows target.",
      "The Linux source does not receive remote-mode input while remote mode is active.",
      "Leaving remote mode restores local Linux source control.",
    ],
    evidence: [
      "Linux X11 source screen topology diagnostics artifact id",
      "Linux X11 source XInput2 capture diagnostics artifact id",
      "Manual Windows pointer, button, scroll, and non-text keyboard pass note",
      "Manual Linux source local-input-leak pass note",
    ],
  },
  {
    id: "LX11-003",
    milestone: "M9",
    category: "capability-diagnostics",
    title: "Missing XTEST capability reports a clear unsupported state",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11 or Linux desktop on X11",
      targetOs: "Linux desktop without usable XTEST",
      sourceSession: "Windows desktop or X11",
      targetSession: "X11 with XTEST unavailable",
      hardware: "Linux endpoint where XTEST can be disabled or proven unavailable",
    },
    prerequisites: [
      "The Linux endpoint can run Akraz while XTEST is unavailable or intentionally blocked.",
      "A trusted peer is available for a remote input attempt.",
      "Diagnostics can be generated after the failed capability probe.",
    ],
    steps: [
      "Start Akraz on the Linux endpoint with XTEST unavailable.",
      "Run input-adapter diagnostics.",
      "Attempt to enter a remote-control session that would require Linux X11 injection.",
      "Generate a diagnostics support bundle.",
    ],
    expected: [
      "Akraz refuses Linux X11 injection instead of pretending the adapter is ready.",
      "The diagnostics identify the unsupported XTEST capability without printing local paths or secrets.",
      "The session remains recoverable and local input stays available.",
    ],
    evidence: [
      "Linux X11 unsupported capability diagnostics artifact id",
      "Manual unsupported-state user-visible error note",
      "Manual local-control-after-failed-probe pass note",
    ],
  },
  {
    id: "LX11-004",
    milestone: "M9",
    category: "geometry",
    title: "Linux X11 xrandr geometry and edge crossing",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Linux desktop on X11",
      targetOs: "Windows 11 or Linux desktop on X11",
      sourceSession: "X11",
      targetSession: "Windows desktop or X11",
      hardware: "Linux source machine with multiple displays or a changed display layout",
    },
    prerequisites: [
      "The Linux source reports display geometry through the X11 screen topology path.",
      "A source edge binding is saved on a monitor boundary.",
      "A trusted target device is available.",
    ],
    steps: [
      "Capture Linux source screen topology diagnostics.",
      "Save an edge binding on a Linux X11 monitor edge.",
      "Cross the configured edge slowly.",
      "Cross the same edge with a fast pointer movement.",
      "Change the Linux display layout and repeat the topology probe.",
    ],
    expected: [
      "The Linux X11 topology matches the active display layout.",
      "Edge crossing enters the target from the configured Linux source edge.",
      "Fast and slow crossings preserve proportional edge offset.",
      "A display layout change produces updated diagnostics before the next session.",
    ],
    evidence: [
      "Linux X11 topology diagnostics before layout change artifact id",
      "Manual slow and fast edge-crossing pass note",
      "Linux X11 topology diagnostics after layout change artifact id",
    ],
  },
  {
    id: "LX11-005",
    milestone: "M9",
    category: "release-all-recovery",
    title: "Cross-OS disconnect releases Linux X11 input state",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11 or Linux desktop on X11",
      targetOs: "Linux desktop on X11",
      sourceSession: "Windows desktop or X11",
      targetSession: "X11",
      hardware: "Two physical machines on a trusted local network",
    },
    prerequisites: [
      "The Linux target can receive pointer and keyboard injection.",
      "Both devices are paired and trusted.",
      "A remote session can be interrupted by disconnecting the local network.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode into the Linux X11 target.",
      "Hold a mouse button or non-text modifier while moving the remote pointer.",
      "Disconnect the network while the input is still held.",
      "Reconnect and generate diagnostics on both endpoints.",
      "Use local input on both endpoints after recovery.",
    ],
    expected: [
      "The Linux X11 target receives release-all during disconnect recovery.",
      "No key or mouse button remains pressed on either endpoint.",
      "Local input works on both endpoints after recovery.",
      "A reconnect starts a clean session without replaying stale input state.",
    ],
    evidence: [
      "Linux X11 target release-all diagnostics artifact id",
      "Manual no-stuck-button-or-modifier pass note",
      "Manual clean-reconnect pass note",
    ],
  },
  {
    id: "LX11-006",
    milestone: "M9",
    category: "linux-packaging",
    title: "Linux deb install starts the daemon sidecar on X11",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Linux desktop on X11",
      targetOs: "Linux desktop on X11",
      sourceSession: "X11",
      targetSession: "X11",
      hardware: "Fresh Linux desktop or VM with X11 session and package dependencies installed",
    },
    prerequisites: [
      "A Linux deb artifact is available from the release build.",
      "The test machine has no previous Akraz install state.",
      "The X11 session exposes XInput2, XTEST, and xrandr runtime libraries.",
    ],
    steps: [
      "Install the Linux deb artifact.",
      "Launch Akraz from the installed desktop entry or app launcher.",
      "Verify the daemon sidecar starts and reports input-adapter diagnostics.",
      "Run a local diagnostics snapshot.",
      "Uninstall Akraz and verify the app no longer starts the daemon.",
    ],
    expected: [
      "The Linux deb installs required X11 runtime dependencies.",
      "The installed app starts the daemon sidecar without requiring a source checkout.",
      "Input-adapter diagnostics identify X11 support state.",
      "Uninstall completes without leaving an active Akraz daemon process.",
    ],
    evidence: [
      "Linux deb install smoke artifact id",
      "Installed app daemon sidecar diagnostics artifact id",
      "Uninstall and daemon-not-running pass note",
    ],
  },
];

export function buildLinuxX11QaPlan(options = {}) {
  const selectedCaseIds = new Set(options.caseIds ?? []);
  const cases =
    selectedCaseIds.size === 0
      ? linuxX11QaCases
      : linuxX11QaCases.filter((testCase) => selectedCaseIds.has(testCase.id));
  const missingCaseIds = [...selectedCaseIds].filter(
    (caseId) => !linuxX11QaCases.some((testCase) => testCase.id === caseId),
  );

  if (missingCaseIds.length > 0) {
    throw new Error(
      `unknown Linux X11 QA case id(s): ${missingCaseIds.join(", ")}; available case ids: ${linuxX11QaCases
        .map((testCase) => testCase.id)
        .join(", ")}`,
    );
  }

  return {
    schemaVersion: LINUX_X11_QA_PLAN_SCHEMA_VERSION,
    milestone: "M9",
    target: "Linux X11 alpha evidence gate",
    caseCount: cases.length,
    releaseBlockingCaseCount: cases.filter((testCase) => testCase.priority === "release-blocking")
      .length,
    cases: cases.map(cloneCase),
    privacy: {
      includesTypedContent: false,
      includesSecretValues: false,
      includesFullFilePaths: false,
    },
  };
}

export function listLinuxX11QaCaseIds() {
  return linuxX11QaCases.map((testCase) => testCase.id);
}

export function parseLinuxX11QaPlanArgs(args) {
  const caseIds = [];

  let index = 0;
  while (index < args.length) {
    const arg = args[index];
    switch (arg) {
      case "--case-id":
        caseIds.push(readValue(args, ++index, arg));
        break;
      case "--list":
        return { list: true, caseIds: [] };
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
    index += 1;
  }

  return { list: false, caseIds };
}

function cloneCase(testCase) {
  const clonedCase = JSON.parse(JSON.stringify(testCase));
  clonedCase.evidenceRequirements = clonedCase.evidence.map((label, index) => ({
    id: `${clonedCase.id}-E${index + 1}`,
    label,
  }));
  return clonedCase;
}

function readValue(args, index, flag) {
  const value = args[index];
  if (!value || value.trim().length === 0) {
    throw new Error(`${flag} requires a non-empty value`);
  }
  return value;
}

if (import.meta.main) {
  const options = parseLinuxX11QaPlanArgs(process.argv.slice(2));
  const payload = options.list
    ? { cases: listLinuxX11QaCaseIds() }
    : buildLinuxX11QaPlan({ caseIds: options.caseIds });
  process.stdout.write(`${JSON.stringify(payload, null, 2)}\n`);
}
