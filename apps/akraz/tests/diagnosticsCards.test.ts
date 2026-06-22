import { describe, expect, test } from "bun:test";

import {
  diagnosticsCards,
  diagnosticsCardStatusClass,
  diagnosticsCardStatusLabel,
} from "../src/lib/diagnostics/diagnosticsCards";
import type { LayoutMismatchReport } from "../src/lib/layout/layoutMismatch";
import type { PermissionsProbe } from "../src/lib/api/types";

function permissionsFixture(overrides: Partial<PermissionsProbe> = {}): PermissionsProbe {
  return {
    adapterName: "windows",
    capabilities: {
      canCapturePointer: true,
      canCaptureKeyboard: true,
      canInjectPointer: true,
      canInjectKeyboard: true,
    },
    issues: [],
    ...overrides,
  };
}

const linuxX11CaptureMessage =
  "Linux X11 input capture is disabled for this build; " +
  "XInput2 capture and Xrandr layout probes are required before capture can be enabled.";

const linuxX11InjectionAdapterBlockedMessage =
  "Linux X11 input injection is disabled for this build; " +
  "XTEST is available, and pointer, button, scroll, and keyboard injection handlers are required next.";

const linuxX11XtestUnavailableMessage =
  "Linux X11 input injection cannot start because the XTEST extension is not available; " +
  "enable XTEST in the X server before enabling injection.";

describe("diagnostics capability cards", () => {
  test("keeps the user-facing card order stable", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture(),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 1,
      hasScreenTopology: true,
    });

    expect(cards.map((card) => card.id)).toEqual([
      "inputCapture",
      "inputInjection",
      "networkDiscovery",
      "pairingAuth",
      "screenLayout",
      "clipboard",
      "updates",
    ]);
  });

  test("reports missing capture and injection permissions as actions", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture({
        capabilities: {
          canCapturePointer: true,
          canCaptureKeyboard: false,
          canInjectPointer: false,
          canInjectKeyboard: true,
        },
        issues: [
          {
            code: "capture_keyboard_unavailable",
            message: "키보드 입력 권한이 필요해.",
          },
          {
            code: "inject_pointer_unavailable",
            message: "마우스 입력 전송 권한이 필요해.",
          },
        ],
      }),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 1,
      hasScreenTopology: true,
    });

    expect(cards.find((card) => card.id === "inputCapture")).toMatchObject({
      status: "NeedsAction",
      summary: "입력 권한 필요",
      detail: "키보드 입력 권한이 필요해.",
    });
    expect(cards.find((card) => card.id === "inputInjection")).toMatchObject({
      status: "NeedsAction",
      summary: "입력 보내기 제한",
      detail: "마우스 입력 전송 권한이 필요해.",
    });
  });

  test("prefers Linux X11 diagnostic issues over generic missing capability text", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture({
        adapterName: "linux-x11",
        capabilities: {
          canCapturePointer: false,
          canCaptureKeyboard: false,
          canInjectPointer: false,
          canInjectKeyboard: false,
        },
        issues: [
          {
            code: "linux_x11_capture_unimplemented",
            message: linuxX11CaptureMessage,
          },
          {
            code: "linux_x11_injection_unimplemented",
            message: linuxX11InjectionAdapterBlockedMessage,
          },
          {
            code: "capture_pointer_unavailable",
            message: "Pointer capture is not available.",
          },
          {
            code: "inject_pointer_unavailable",
            message: "Pointer injection is not available.",
          },
        ],
      }),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 1,
      hasScreenTopology: true,
    });

    expect(cards.find((card) => card.id === "inputCapture")).toMatchObject({
      status: "NeedsAction",
      detail: linuxX11CaptureMessage,
    });
    expect(cards.find((card) => card.id === "inputInjection")).toMatchObject({
      status: "NeedsAction",
      detail: linuxX11InjectionAdapterBlockedMessage,
    });
  });

  test("shows Linux X11 XTEST probe failures before generic injection text", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture({
        adapterName: "linux-x11",
        capabilities: {
          canCapturePointer: false,
          canCaptureKeyboard: false,
          canInjectPointer: false,
          canInjectKeyboard: false,
        },
        issues: [
          {
            code: "linux_x11_capture_unimplemented",
            message: linuxX11CaptureMessage,
          },
          {
            code: "linux_x11_injection_xtest_unavailable",
            message: linuxX11XtestUnavailableMessage,
          },
          {
            code: "inject_pointer_unavailable",
            message: "Pointer injection is not available.",
          },
        ],
      }),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 1,
      hasScreenTopology: true,
    });

    expect(cards.find((card) => card.id === "inputInjection")).toMatchObject({
      status: "NeedsAction",
      detail: linuxX11XtestUnavailableMessage,
    });
  });

  test("derives pairing and layout readiness from current owners", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture(),
      hasLocalIdentity: false,
      trustedPeerCount: 0,
      layoutBindingCount: 0,
      hasScreenTopology: false,
    });

    expect(cards.find((card) => card.id === "pairingAuth")).toMatchObject({
      status: "NeedsAction",
      summary: "내 기기 등록 필요",
    });
    expect(cards.find((card) => card.id === "screenLayout")).toMatchObject({
      status: "NeedsAction",
      summary: "배치 필요",
    });
  });

  test("uses layout mismatch details for the screen layout card", () => {
    const layoutMismatch: LayoutMismatchReport = {
      status: "needs-action",
      issues: [
        {
          code: "unknown-peer",
          status: "needs-action",
          message: "신뢰 목록에 없는 기기가 배치에 남아 있어.",
        },
        {
          code: "duplicate-local-edge",
          status: "needs-action",
          message: "같은 화면 끝에 여러 기기가 붙어 있어.",
        },
      ],
      validBindingCount: 0,
      bindingCount: 2,
      trustedPeerCount: 1,
      hasUsableTopology: true,
    };

    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture(),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 2,
      hasScreenTopology: true,
      layoutMismatch,
    });

    expect(cards.find((card) => card.id === "screenLayout")).toMatchObject({
      status: "NeedsAction",
      summary: "2개 조치 필요",
      detail: "신뢰 목록에 없는 기기가 배치에 남아 있어.",
    });
  });

  test("reports ready layout mismatch as confirmed", () => {
    const cards = diagnosticsCards({
      snapshot: null,
      permissions: permissionsFixture(),
      hasLocalIdentity: true,
      trustedPeerCount: 1,
      layoutBindingCount: 1,
      hasScreenTopology: true,
      layoutMismatch: {
        status: "ready",
        issues: [],
        validBindingCount: 1,
        bindingCount: 1,
        trustedPeerCount: 1,
        hasUsableTopology: true,
      },
    });

    expect(cards.find((card) => card.id === "screenLayout")).toMatchObject({
      status: "OK",
      summary: "1개 경계 확인됨",
      detail: "신뢰한 기기와 현재 화면 범위가 맞아.",
    });
  });

  test("labels and class names stay explicit", () => {
    expect(diagnosticsCardStatusLabel("OK")).toBe("정상");
    expect(diagnosticsCardStatusLabel("NeedsAction")).toBe("조치 필요");
    expect(diagnosticsCardStatusClass("NeedsAction")).toBe("needs-action");
    expect(diagnosticsCardStatusClass("Limited")).toBe("limited");
  });
});
