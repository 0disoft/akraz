export const WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION = "akraz.windowsMvpQaPlan/v1";

const windowsMvpQaCases = [
  {
    id: "WIN-001",
    milestone: "M6",
    category: "baseline-remote-control",
    title: "Baseline Windows remote control loop",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "The target device can receive pointer, button, scroll, and keyboard injection.",
      "A non-sensitive target window is focused before entering remote mode.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Capture screen topology diagnostics on the source device.",
      "Cross the configured source edge into the target device.",
      "Move the pointer, click, drag, and scroll on the target device.",
      "Press and release basic non-text keys on the target device.",
      "Leave remote mode and use local input on the source device.",
    ],
    expected: [
      "The pointer enters the target device from the configured remote edge.",
      "Pointer movement, click, drag, scroll, and basic keyboard input are applied on the target device.",
      "Local source apps do not receive remote-mode pointer, button, scroll, or keyboard input.",
      "Leaving remote mode restores local control on the source device.",
    ],
    evidence: [
      "Screen topology diagnostics and saved edge binding snapshot",
      "Manual target pointer, click, drag, scroll, and keyboard pass note without typed content",
      "Manual source local-input-leak pass note",
      "Windows MVP soak report with remote input and release QA evidence passing",
    ],
  },
  {
    id: "WIN-002",
    milestone: "M8",
    category: "disconnect-recovery",
    title: "Drag disconnect release recovery",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "The target device can receive pointer and button injection.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode from the configured edge.",
      "Start a left-button drag on the target device.",
      "Disconnect the network while the button is still held.",
      "Reconnect and generate a diagnostics support bundle.",
    ],
    expected: [
      "The target device receives release-all after the disconnect.",
      "The drag does not remain active on the target device.",
      "Local control is restored on the source device.",
      "A reconnect starts a new clean session.",
    ],
    evidence: [
      "Diagnostics support bundle after reconnect",
      "Manual target button-state pass note",
      "Windows MVP soak report with stuckInputSuspicions equal to 0",
    ],
  },
  {
    id: "WIN-003",
    milestone: "M8",
    category: "disconnect-recovery",
    title: "Modifier disconnect release recovery",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "The target device can receive keyboard injection.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode from the configured edge.",
      "Hold Shift on the source device.",
      "Disconnect the network while Shift is still held.",
      "Release Shift locally and reconnect.",
      "Use release-all from the UI after reconnect.",
    ],
    expected: [
      "The target device receives a Shift release during recovery.",
      "No modifier remains pressed on either device.",
      "The source device returns to local mode.",
      "The next remote session starts without replaying stale modifier state.",
    ],
    evidence: [
      "Diagnostics support bundle after modifier recovery",
      "Input release result",
      "Windows MVP soak report with stuckInputSuspicions equal to 0",
    ],
  },
  {
    id: "WIN-006",
    milestone: "M8",
    category: "sleep-resume",
    title: "Sleep and resume recovery",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "Diagnostics support bundle generation works before the test.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode from the configured edge.",
      "Hold Shift while moving the pointer on the remote device.",
      "Put the source device to sleep.",
      "Resume the source device and reconnect.",
      "Generate a diagnostics support bundle.",
    ],
    expected: [
      "Local control is restored after resume.",
      "The remote device receives release-all before or during recovery.",
      "No key or mouse button remains pressed on either device.",
      "A new session is used after reconnect.",
    ],
    evidence: [
      "Diagnostics support bundle after resume",
      "Windows MVP soak report with stuckInputSuspicions equal to 0",
    ],
  },
  {
    id: "WIN-007",
    milestone: "M8",
    category: "mixed-dpi",
    title: "Mixed DPI edge crossing",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Source machine with at least two monitors using different scale factors",
    },
    prerequisites: [
      "Screen topology diagnostics reports at least two monitors.",
      "The reported monitor scale factors are not all the same.",
      "A trusted target device is available.",
    ],
    steps: [
      "Probe the current screen topology.",
      "Save an edge binding on a monitor boundary.",
      "Use the layout test preview for the configured edge.",
      "Cross the configured edge slowly.",
      "Cross the same edge with a fast pointer movement.",
      "Repeat from a second vertical offset on the edge.",
    ],
    expected: [
      "The pointer enters the target device from the configured remote edge.",
      "Edge offset is proportional to the source edge position.",
      "No local app receives click, scroll, or key input while remote mode is active.",
      "Diagnostics marks mixed DPI as limited, not as ready without review.",
    ],
    evidence: [
      "Screen topology diagnostics",
      "Layout mismatch report showing mixed-dpi",
      "Manual crossing pass note for slow and fast movement",
    ],
  },
  {
    id: "WIN-008",
    milestone: "M8",
    category: "hook-watchdog",
    title: "Remote-mode hook watchdog under input load",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "The source device can capture pointer and keyboard input.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode from the configured edge.",
      "Move the pointer quickly for at least 60 seconds.",
      "Send repeated click, scroll, and modifier key events while remote mode is active.",
      "Leave remote mode and generate a diagnostics support bundle.",
    ],
    expected: [
      "Remote input remains responsive during the burst.",
      "Local source apps do not receive the remote-mode input burst.",
      "The hook watchdog does not report a stuck or dead capture path.",
      "Release-all leaves no key or button stuck after the burst.",
    ],
    evidence: [
      "Diagnostics support bundle after input burst",
      "Manual local-input-leak pass note",
      "Windows MVP soak report with scenarioFailures equal to 0",
    ],
  },
  {
    id: "WIN-010",
    milestone: "M8",
    category: "emergency-recovery",
    title: "Panic hotkey immediate local recovery",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Two physical machines or two stable Windows endpoints",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A right or left edge binding is saved.",
      "The panic hotkey is configured to the default emergency shortcut.",
    ],
    steps: [
      "Start Akraz on both devices.",
      "Enter remote mode from the configured edge.",
      "Hold a modifier key while moving the remote pointer.",
      "Trigger the panic hotkey on the source device.",
      "Try local input on the source device immediately after recovery.",
      "Generate a diagnostics support bundle.",
    ],
    expected: [
      "The source device returns to local mode immediately.",
      "The target device receives release-all.",
      "The active remote session is stopped.",
      "No key or mouse button remains pressed on either device.",
    ],
    evidence: [
      "Diagnostics support bundle after panic hotkey",
      "Input release result",
      "Manual immediate-local-control pass note",
    ],
  },
  {
    id: "WIN-011",
    milestone: "M8",
    category: "layout-recovery",
    title: "Screen layout change during remote session",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      hardware: "Source machine with a removable monitor or docked display setup",
    },
    prerequisites: [
      "Both devices are paired and trusted.",
      "A saved edge binding is active on the source display.",
      "Screen topology diagnostics works before the test.",
    ],
    steps: [
      "Probe the current screen topology.",
      "Enter remote mode from the configured edge.",
      "Change the source screen layout by connecting, disconnecting, or reconfiguring a display.",
      "Attempt local input on the source device after the layout change.",
      "Generate a diagnostics support bundle.",
    ],
    expected: [
      "Akraz recovers local control after detecting the layout change.",
      "The target device receives release-all.",
      "The active remote session is stopped.",
      "Diagnostics shows the updated screen topology after recovery.",
    ],
    evidence: [
      "Screen topology diagnostics before and after the layout change",
      "Diagnostics support bundle after layout recovery",
      "Manual local-control pass note",
    ],
  },
  {
    id: "I18N-001",
    milestone: "M8",
    category: "ime",
    title: "Windows Korean 2-beolsik target input",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      targetInputMethod: "Korean 2-beolsik IME",
    },
    prerequisites: [
      "Target device has Korean 2-beolsik enabled.",
      "Target text editor is focused before entering remote mode.",
      "Keyboard layout diagnostics are captured before the test.",
    ],
    steps: [
      "Enable Korean IME on the target device.",
      "Enter remote mode from the source device.",
      "Type a short Korean composition sequence.",
      "Commit the composition with Space or Enter.",
      "Use release-all from the UI after typing.",
    ],
    expected: [
      "Composition is owned by the target IME.",
      "Akraz forwards physical key down and key up events without logging typed text.",
      "Release-all leaves no modifier or physical key stuck.",
      "Diagnostics can identify the target keyboard layout without exposing typed content.",
    ],
    evidence: [
      "Keyboard layout diagnostics",
      "Manual text entry pass note without storing typed content",
      "Input release result",
    ],
  },
  {
    id: "I18N-004",
    milestone: "M8",
    category: "ime",
    title: "Windows Japanese IME target conversion",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      targetInputMethod: "Japanese IME",
    },
    prerequisites: [
      "Target device has Japanese IME enabled.",
      "Target text editor is focused before entering remote mode.",
      "Keyboard layout diagnostics are captured before the test.",
    ],
    steps: [
      "Enable Japanese IME on the target device.",
      "Enter remote mode from the source device.",
      "Type romaji into the target editor.",
      "Open conversion candidates with Space.",
      "Commit a candidate and leave remote mode.",
    ],
    expected: [
      "Romaji conversion is owned by the target IME.",
      "Candidate selection is not controlled or inspected by Akraz.",
      "Local source apps do not receive the input sequence.",
      "Release-all leaves no modifier or physical key stuck.",
    ],
    evidence: [
      "Keyboard layout diagnostics",
      "Manual conversion pass note without storing typed content",
      "Input release result",
    ],
  },
  {
    id: "REL-001",
    milestone: "M8",
    category: "installer-updater",
    title: "Signed installer and updater dry run",
    priority: "release-blocking",
    automation: "manual",
    environment: {
      sourceOs: "Windows 11",
      targetOs: "Windows 11",
      artifact: "Signed Windows installer and Tauri updater artifact",
    },
    prerequisites: [
      "Updater public key is configured without private key material.",
      "Windows signing certificate is available in the release environment.",
      "Release metadata versions are synchronized.",
    ],
    steps: [
      "Run signing preflight in the release environment.",
      "Run updater config preflight in the release environment.",
      "Build the Windows installer.",
      "Install, uninstall, and reinstall Akraz.",
      "Run an updater dry run against a staged artifact.",
    ],
    expected: [
      "Signing preflight passes without printing secret values.",
      "Updater config preflight passes with HTTPS endpoints only.",
      "Installer install and uninstall complete without orphaning the daemon sidecar.",
      "Updater dry run verifies the artifact signature before applying.",
    ],
    evidence: [
      "Signing preflight JSON",
      "Updater config preflight JSON",
      "Installer smoke result",
      "Updater dry-run result",
    ],
  },
];

export function buildWindowsMvpQaPlan(options = {}) {
  const selectedCaseIds = new Set(options.caseIds ?? []);
  const cases =
    selectedCaseIds.size === 0
      ? windowsMvpQaCases
      : windowsMvpQaCases.filter((testCase) => selectedCaseIds.has(testCase.id));
  const missingCaseIds = [...selectedCaseIds].filter(
    (caseId) => !windowsMvpQaCases.some((testCase) => testCase.id === caseId),
  );

  if (missingCaseIds.length > 0) {
    throw new Error(
      `unknown Windows MVP QA case id(s): ${missingCaseIds.join(", ")}; available case ids: ${windowsMvpQaCases
        .map((testCase) => testCase.id)
        .join(", ")}`,
    );
  }

  return {
    schemaVersion: WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION,
    milestone: "M8",
    target: "Windows MVP alpha",
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

export function listWindowsMvpQaCaseIds() {
  return windowsMvpQaCases.map((testCase) => testCase.id);
}

export function parseWindowsMvpQaPlanArgs(args) {
  const caseIds = [];

  for (let index = 0; index < args.length; index += 1) {
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
  const options = parseWindowsMvpQaPlanArgs(process.argv.slice(2));
  const payload = options.list
    ? { cases: listWindowsMvpQaCaseIds() }
    : buildWindowsMvpQaPlan({ caseIds: options.caseIds });
  process.stdout.write(`${JSON.stringify(payload, null, 2)}\n`);
}
