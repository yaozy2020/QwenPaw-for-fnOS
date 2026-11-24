import { reactive } from "vue";
import { t, type MessageKey } from "./i18n.ts";

type AuthStateKey = "initial" | "uninitialized" | "protected" | "disabled" | "unknown" | "offline";

const authTones: Record<AuthStateKey, string> = {
  initial: "text-muted",
  uninitialized: "warning",
  protected: "success",
  disabled: "danger-text",
  unknown: "text-muted",
  offline: "text-muted",
};

const authKeys: Record<AuthStateKey, { label: MessageKey; admin: MessageKey; desc: MessageKey }> = {
  initial: { label: "auth.initial.label", admin: "auth.initial.admin", desc: "auth.initial.desc" },
  uninitialized: { label: "auth.uninitialized.label", admin: "auth.uninitialized.admin", desc: "auth.uninitialized.desc" },
  protected: { label: "auth.protected.label", admin: "auth.protected.admin", desc: "auth.protected.desc" },
  disabled: { label: "auth.disabled.label", admin: "auth.disabled.admin", desc: "auth.disabled.desc" },
  unknown: { label: "auth.unknown.label", admin: "auth.unknown.admin", desc: "auth.unknown.desc" },
  offline: { label: "auth.offline.label", admin: "auth.offline.admin", desc: "auth.offline.desc" },
};

export interface AuthPresentation {
  readonly tone: string;
  readonly label: string;
  readonly admin: string;
  readonly description: string;
}

function makeAuthPresentation(key: AuthStateKey): AuthPresentation {
  return {
    tone: authTones[key],
    get label() { return t(authKeys[key].label); },
    get admin() { return t(authKeys[key].admin); },
    get description() { return t(authKeys[key].desc); },
  };
}

const authStates: Record<AuthStateKey, AuthPresentation> = {
  initial: makeAuthPresentation("initial"),
  uninitialized: makeAuthPresentation("uninitialized"),
  protected: makeAuthPresentation("protected"),
  disabled: makeAuthPresentation("disabled"),
  unknown: makeAuthPresentation("unknown"),
  offline: makeAuthPresentation("offline"),
};

export function authPresentation(status: string): AuthPresentation {
  return status === "protected" || status === "disabled" || status === "uninitialized" || status === "offline"
    ? authStates[status] : authStates.unknown;
}

export function createManagementState() {
  return reactive({
    servicePort: "18088", serviceReachable: false, backupAvailable: false, auth: authStates.initial,
    proxyPort: null as number | null, proxyPublic: null as boolean | null, proxyDraft: false,
    entryLogin: null as boolean | null,
    pending: false, saving: false,
  });
}
export type ManagementState = ReturnType<typeof createManagementState>;
