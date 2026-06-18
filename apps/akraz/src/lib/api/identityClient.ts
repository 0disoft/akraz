import { invoke } from "@tauri-apps/api/core";

import type {
  IdentityForgetTrustedParams,
  IdentityForgetTrustedResult,
  IdentityShowResult,
  IdentityTrustParams,
  IdentityTrustedPeersResult,
  IdentityTrustResult,
} from "./types";

export const identityClient = {
  show(): Promise<IdentityShowResult> {
    return invoke<IdentityShowResult>("identity_show");
  },

  listTrusted(): Promise<IdentityTrustedPeersResult> {
    return invoke<IdentityTrustedPeersResult>("identity_list_trusted");
  },

  trust(params: IdentityTrustParams): Promise<IdentityTrustResult> {
    return invoke<IdentityTrustResult>("identity_trust", { params });
  },

  forgetTrusted(params: IdentityForgetTrustedParams): Promise<IdentityForgetTrustedResult> {
    return invoke<IdentityForgetTrustedResult>("identity_forget_trusted", { params });
  },
};
