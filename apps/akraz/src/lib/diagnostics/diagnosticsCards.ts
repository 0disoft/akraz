import type {
  DiagnosticsSnapshot,
  PermissionIssue,
  PermissionsProbe,
  PlatformCapabilities,
} from "../api/types";

export type DiagnosticsCardStatus = "OK" | "NeedsAction" | "Limited" | "Error";

export type DiagnosticsCardId =
  | "inputCapture"
  | "inputInjection"
  | "networkDiscovery"
  | "pairingAuth"
  | "screenLayout"
  | "clipboard"
  | "updates";

export interface DiagnosticsCard {
  id: DiagnosticsCardId;
  title: string;
  status: DiagnosticsCardStatus;
  summary: string;
  detail: string;
}

export interface DiagnosticsCardsInput {
  snapshot: DiagnosticsSnapshot | null;
  permissions: PermissionsProbe | null;
  hasLocalIdentity: boolean;
  trustedPeerCount: number;
  layoutBindingCount: number;
  hasScreenTopology: boolean;
}

type PermissionFacts = {
  capabilities: PlatformCapabilities;
  issues: PermissionIssue[];
};

const statusLabels: Record<DiagnosticsCardStatus, string> = {
  OK: "정상",
  NeedsAction: "조치 필요",
  Limited: "제한",
  Error: "실패",
};

export function diagnosticsCardStatusLabel(status: DiagnosticsCardStatus): string {
  return statusLabels[status];
}

export function diagnosticsCardStatusClass(status: DiagnosticsCardStatus): string {
  return status === "NeedsAction" ? "needs-action" : status.toLowerCase();
}

export function diagnosticsCards(input: DiagnosticsCardsInput): DiagnosticsCard[] {
  const permissions = input.snapshot?.permissions ?? input.permissions;
  const hasScreenTopology = Boolean(input.snapshot?.screenTopology) || input.hasScreenTopology;

  return [
    inputCaptureCard(permissions),
    inputInjectionCard(permissions),
    networkDiscoveryCard(),
    pairingAuthCard(input.hasLocalIdentity, input.trustedPeerCount),
    screenLayoutCard(input.layoutBindingCount, hasScreenTopology),
    clipboardCard(),
    updatesCard(),
  ];
}

function inputCaptureCard(permissions: PermissionFacts | null): DiagnosticsCard {
  if (!permissions) {
    return {
      id: "inputCapture",
      title: "입력 캡처",
      status: "Limited",
      summary: "확인 전",
      detail: "데몬을 실행한 뒤 마우스와 키보드 권한을 확인할 수 있어.",
    };
  }

  const missingPointer = !permissions.capabilities.canCapturePointer;
  const missingKeyboard = !permissions.capabilities.canCaptureKeyboard;
  if (missingPointer || missingKeyboard) {
    return {
      id: "inputCapture",
      title: "입력 캡처",
      status: "NeedsAction",
      summary: "입력 권한 필요",
      detail:
        matchingIssueMessage(permissions.issues, ["capture"]) ??
        "운영체제 설정에서 마우스와 키보드 입력 권한을 허용해줘.",
    };
  }

  return {
    id: "inputCapture",
    title: "입력 캡처",
    status: "OK",
    summary: "입력을 잡을 수 있어",
    detail: "마우스와 키보드 입력을 이 기기에서 감지할 수 있어.",
  };
}

function inputInjectionCard(permissions: PermissionFacts | null): DiagnosticsCard {
  if (!permissions) {
    return {
      id: "inputInjection",
      title: "입력 주입",
      status: "Limited",
      summary: "확인 전",
      detail: "데몬을 실행한 뒤 원격 입력 전송 상태를 확인할 수 있어.",
    };
  }

  const missingPointer = !permissions.capabilities.canInjectPointer;
  const missingKeyboard = !permissions.capabilities.canInjectKeyboard;
  if (missingPointer || missingKeyboard) {
    return {
      id: "inputInjection",
      title: "입력 주입",
      status: "NeedsAction",
      summary: "입력 보내기 제한",
      detail:
        matchingIssueMessage(permissions.issues, ["inject"]) ??
        "상대 기기에 마우스와 키보드 입력을 보내려면 운영체제 입력 권한이 필요해.",
    };
  }

  return {
    id: "inputInjection",
    title: "입력 주입",
    status: "OK",
    summary: "입력을 보낼 수 있어",
    detail: "상대 기기에 마우스와 키보드 입력을 전달할 준비가 됐어.",
  };
}

function networkDiscoveryCard(): DiagnosticsCard {
  return {
    id: "networkDiscovery",
    title: "네트워크 검색",
    status: "Limited",
    summary: "수동 연결",
    detail: "상대 기기 주소를 저장해 두면 같은 네트워크에서 바로 연결할 수 있어.",
  };
}

function pairingAuthCard(hasLocalIdentity: boolean, trustedPeerCount: number): DiagnosticsCard {
  if (!hasLocalIdentity) {
    return {
      id: "pairingAuth",
      title: "페어링 인증",
      status: "NeedsAction",
      summary: "내 기기 등록 필요",
      detail: "기기 등록에서 내 기기 코드를 만들고 상대 기기와 교환해.",
    };
  }

  if (trustedPeerCount === 0) {
    return {
      id: "pairingAuth",
      title: "페어링 인증",
      status: "NeedsAction",
      summary: "상대 기기 등록 필요",
      detail: "상대 기기의 등록 코드를 붙여넣으면 신뢰 목록에 추가돼.",
    };
  }

  return {
    id: "pairingAuth",
    title: "페어링 인증",
    status: "OK",
    summary: `${trustedPeerCount}대 등록됨`,
    detail: "등록된 기기와 안전하게 세션을 시작할 수 있어.",
  };
}

function screenLayoutCard(layoutBindingCount: number, hasScreenTopology: boolean): DiagnosticsCard {
  if (layoutBindingCount === 0) {
    return {
      id: "screenLayout",
      title: "화면 배치",
      status: "NeedsAction",
      summary: "배치 필요",
      detail: "화면 배치에서 넘어갈 경계와 상대 기기를 추가해.",
    };
  }

  if (!hasScreenTopology) {
    return {
      id: "screenLayout",
      title: "화면 배치",
      status: "Limited",
      summary: `${layoutBindingCount}개 경계`,
      detail: "데몬이 실행 중이면 현재 화면 범위까지 함께 확인할 수 있어.",
    };
  }

  return {
    id: "screenLayout",
    title: "화면 배치",
    status: "OK",
    summary: `${layoutBindingCount}개 경계 확인됨`,
    detail: "현재 화면 범위와 저장된 경계를 함께 확인했어.",
  };
}

function clipboardCard(): DiagnosticsCard {
  return {
    id: "clipboard",
    title: "클립보드",
    status: "Limited",
    summary: "입력 중심",
    detail: "클립보드 내용은 아직 공유 대상에 포함하지 않아.",
  };
}

function updatesCard(): DiagnosticsCard {
  return {
    id: "updates",
    title: "업데이트",
    status: "Limited",
    summary: "수동 확인",
    detail: "새 버전은 배포 페이지에서 직접 확인해.",
  };
}

function matchingIssueMessage(issues: PermissionIssue[], keywords: string[]): string | null {
  for (const issue of issues) {
    const searchable = `${issue.code} ${issue.message}`.toLowerCase();
    if (keywords.some((keyword) => searchable.includes(keyword))) {
      return issue.message;
    }
  }

  return issues[0]?.message ?? null;
}
