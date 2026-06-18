import { describe, expect, test } from "bun:test";

import {
  manualPeerAddress,
  updateManualPeerAddress,
} from "../src/lib/settings/manualPeerAddresses";
import type { ManualPeerAddressSetting } from "../src/lib/api/types";

describe("manual peer addresses", () => {
  test("reads saved addresses by normalized peer id", () => {
    const entries: ManualPeerAddressSetting[] = [
      { peerId: "linux-laptop", address: "127.0.0.1:4455" },
    ];

    expect(manualPeerAddress(entries, " linux-laptop ")).toBe("127.0.0.1:4455");
    expect(manualPeerAddress(entries, "unknown")).toBe("");
  });

  test("upserts one trimmed address per peer", () => {
    const initial: ManualPeerAddressSetting[] = [
      { peerId: "linux-laptop", address: "127.0.0.1:4455" },
      { peerId: "macbook", address: "127.0.0.1:4456" },
    ];

    expect(updateManualPeerAddress(initial, " linux-laptop ", " 127.0.0.1:4460 ")).toEqual([
      { peerId: "macbook", address: "127.0.0.1:4456" },
      { peerId: "linux-laptop", address: "127.0.0.1:4460" },
    ]);
  });

  test("removes a peer address when the address is cleared", () => {
    const initial: ManualPeerAddressSetting[] = [
      { peerId: "linux-laptop", address: "127.0.0.1:4455" },
      { peerId: "macbook", address: "127.0.0.1:4456" },
    ];

    expect(updateManualPeerAddress(initial, "linux-laptop", " ")).toEqual([
      { peerId: "macbook", address: "127.0.0.1:4456" },
    ]);
  });

  test("ignores empty peer ids", () => {
    const initial: ManualPeerAddressSetting[] = [
      { peerId: "linux-laptop", address: "127.0.0.1:4455" },
    ];

    expect(updateManualPeerAddress(initial, " ", "127.0.0.1:4456")).toBe(initial);
  });
});
