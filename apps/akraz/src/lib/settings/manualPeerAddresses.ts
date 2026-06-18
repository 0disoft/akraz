import type { ManualPeerAddressSetting } from "../api/types";

export function manualPeerAddress(entries: ManualPeerAddressSetting[], peerId: string): string {
  const normalizedPeerId = peerId.trim();
  return entries.find((entry) => entry.peerId === normalizedPeerId)?.address ?? "";
}

export function updateManualPeerAddress(
  entries: ManualPeerAddressSetting[],
  peerId: string,
  address: string,
): ManualPeerAddressSetting[] {
  const normalizedPeerId = peerId.trim();
  if (normalizedPeerId.length === 0) {
    return entries;
  }

  const normalizedAddress = address.trim();
  const remaining = entries.filter((entry) => entry.peerId !== normalizedPeerId);
  return normalizedAddress.length === 0
    ? remaining
    : [...remaining, { peerId: normalizedPeerId, address: normalizedAddress }];
}
