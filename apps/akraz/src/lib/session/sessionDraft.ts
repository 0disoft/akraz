export interface SessionDraft {
  peerId: string;
  localDeviceId: string;
  address: string;
}

export function selectTrustedPeerSessionDraft(
  draft: SessionDraft,
  peerId: string,
  savedAddress: string,
  localDeviceId: string | null,
): SessionDraft {
  if (peerId.length === 0) {
    return draft;
  }

  return {
    peerId,
    localDeviceId: localDeviceId ?? draft.localDeviceId,
    address: savedAddress,
  };
}
