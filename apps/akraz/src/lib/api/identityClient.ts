import { invoke } from "@tauri-apps/api/core";

import type { IdentityShowResult, IdentityTrustParams, IdentityTrustResult } from "./types";

export const identityClient = {
  show(): Promise<IdentityShowResult> {
    return invoke<IdentityShowResult>("identity_show");
  },

  trust(params: IdentityTrustParams): Promise<IdentityTrustResult> {
    return invoke<IdentityTrustResult>("identity_trust", { params });
  },
};
