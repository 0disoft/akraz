export const WINDOWS_MVP_QA_PLAN_SCHEMA_VERSION = "akraz.windowsMvpQaPlan/v1";

const windowsMvpQaCases = [
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
  return JSON.parse(JSON.stringify(testCase));
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
