import type { ApiClient } from "./api";
import { operationSucceeded } from "./apiResult";
import { computed, reactive } from "vue";
import type { ManagementState } from "./managementState";
import type { ShowModal } from "./modal";
import { t, type MessageKey } from "./i18n";

const MIRROR_SPECS: readonly { value: string; labelKey: MessageKey }[] = [
  { value: "https://mirrors.aliyun.com/pypi/simple/", labelKey: "mirror.aliyun" },
  { value: "https://pypi.tuna.tsinghua.edu.cn/simple", labelKey: "mirror.tsinghua" },
  { value: "https://pypi.mirrors.ustc.edu.cn/simple/", labelKey: "mirror.ustc" },
  { value: "https://mirrors.cloud.tencent.com/pypi/simple", labelKey: "mirror.tencent" },
  { value: "", labelKey: "mirror.pypi" },
];

function canonicalMirror(value: string): string {
  const normalized = value.replace(/\/+$/, "");
  if (normalized === "https://pypi.org/simple") return "";
  return MIRROR_SPECS.find(item => item.value && item.value.replace(/\/+$/, "") === normalized)?.value ?? value;
}

interface SettingsPanelOptions {
  state: ManagementState;
  api: Pick<ApiClient,
    "changePort" | "webProxyPort" | "changeWebProxyPort" | "migrateEntry" | "resetAuth" | "getMirror" | "setMirror">;
  showModal: ShowModal;
  showToast(message: string): void;
  errorMessage(error: unknown): string;
  supervisorReachable(): boolean | null;
  fetchStatus(): void;
  fetchLogs(): void;
  fetchVersionList(sourceChanged?: boolean): Promise<boolean>;
}

export function createSettingsPanel({
  api, state, showModal, showToast, errorMessage, supervisorReachable, fetchStatus, fetchLogs, fetchVersionList,
}: SettingsPanelOptions) {
  let webProxyReadSeq = 0;
  let statusRefreshTimer: ReturnType<typeof setTimeout> | null = null;
  const mirror = reactive({ value: '', ready: false, open: false, saving: false });
  const mirrors = computed(() => MIRROR_SPECS.map(item => ({ value: item.value, label: t(item.labelKey) })));
  let mirrorRead = 0;

  function setWebProxyScope(value: boolean | null): void {
    state.proxyPublic = value;
    if (value !== null) state.proxyDraft = value;
  }

  async function runAction(action: () => Promise<void>): Promise<void> {
    if (state.pending) return;
    state.pending = true;
    try { await action(); }
    finally { state.pending = false; state.saving = false; }
  }

  async function changeServicePort(): Promise<void> {
    if (supervisorReachable() !== true) {
      showToast(t("settings.portNotSyncedToast"));
      fetchStatus();
      return;
    }
    const currentPort = state.servicePort;
    const raw = await showModal({
      title: t("settings.changePortTitle"),
      message: t("settings.changePortMsg"),
      type: "warn",
      confirmText: t("settings.changePortConfirm"),
      input: {
        label: t("settings.newPortLabel"),
        value: currentPort,
        inputMode: "numeric",
        placeholder: "1024-65535",
        validate: (value) => /^[0-9]+$/.test(value) && Number(value) >= 1024 && Number(value) <= 65535,
      },
    });
    if (raw === null || raw === false || raw === currentPort) return;
    const newPort = Number.parseInt(raw, 10);
    if (!Number.isInteger(newPort) || newPort < 1024 || newPort > 65535) {
      showToast(t("settings.invalidPort"));
      return;
    }
    const ok = await showModal({
      title: t("settings.changePortVerifyTitle"),
      message: t("settings.changePortVerifyMsg", { port: newPort }),
      type: "warn",
      confirmText: t("settings.changePortConfirm"),
    });
    if (!ok || supervisorReachable() !== true) return;
    state.saving = true;
    try {
      showToast(t("settings.savingPort", { port: newPort }));
      const data = operationSucceeded(await api.changePort(newPort));
      showToast(data.message || t("settings.portChanged", { port: newPort }));
      if (statusRefreshTimer !== null) clearTimeout(statusRefreshTimer);
      statusRefreshTimer = setTimeout(() => {
        statusRefreshTimer = null;
        fetchStatus();
        fetchLogs();
      }, 2500);
    } catch (error) {
      showToast(t("settings.changePortFailed", { error: errorMessage(error) }));
    }
  }

  async function fetchWebProxyPort(): Promise<void> {
    const seq = ++webProxyReadSeq;
    try {
      const result = await api.webProxyPort();
      if (seq !== webProxyReadSeq) return;
      const { data } = result;
      if (!result.ok || !data) {
        state.proxyPort = null;
        setWebProxyScope(null);
        return;
      }
      state.entryLogin = data.entryLogin === true;
      const validPort = Number.isInteger(data.port) && data.port > 0 && data.port <= 65535;
      state.proxyPort = validPort ? data.port : null;
      setWebProxyScope(typeof data.public === "boolean" ? data.public : null);
    } catch (error) {
      if (seq === webProxyReadSeq) {
        state.proxyPort = null;
        setWebProxyScope(null);
      }
      console.error("fetchWebProxyPort err:", error);
    }
  }

  async function changeWebProxyPort(): Promise<void> {
    if (state.proxyPublic === null || state.proxyPort === null) {
      showToast(t("settings.proxyUnknownToast"));
      await fetchWebProxyPort();
      return;
    }
    const currentPort = String(state.proxyPort);
    const raw = await showModal({
      title: t("settings.changeProxyTitle"),
      message: t("settings.changeProxyMsg"),
      type: "warn",
      confirmText: t("settings.changeProxyConfirm"),
      input: {
        label: t("settings.proxyPortLabel"),
        value: currentPort,
        inputMode: "numeric",
        placeholder: t("settings.proxyPortPlaceholder"),
        validate: (value) => /^[0-9]+$/.test(value) && Number(value) <= 65535,
      },
    });
    if (raw === null || raw === false) return;
    const newPort = Number(raw);
    if (!Number.isInteger(newPort) || newPort < 0 || newPort > 65535 || (newPort > 0 && newPort < 1024)) {
      showToast(t("settings.invalidProxyPort"));
      return;
    }
    const ok = await showModal({
      title: t("settings.switchProxyTitle"),
      message: newPort === 0 ? t("settings.switchProxyAutoMsg") : t("settings.switchProxyFixedMsg", { port: newPort }),
      type: "warn",
      confirmText: t("settings.switchProxyConfirm"),
    });
    if (!ok) return;
    const publicAccess = state.proxyPublic;
    if (publicAccess === null) {
      showToast(t("settings.proxyScopeUnknown"));
      return;
    }
    state.saving = true;
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      const data = operationSucceeded(await api.changeWebProxyPort(newPort, publicAccess));
      if (!Number.isInteger(data.port) || data.port <= 0 || data.port > 65535) {
        throw new Error(t("settings.proxyInvalidBackend"));
      }
      state.proxyPort = data.port;
      setWebProxyScope(publicAccess);
      showToast(t("settings.proxyChanged", { port: data.port }));
    } catch (error) {
      showToast(t("settings.proxyChangeFailed", { error: errorMessage(error) }));
      await fetchWebProxyPort();
    }
  }

  async function setWebProxyPublic(publicAccess: boolean): Promise<void> {
    const previous = state.proxyPublic;
    const port = state.proxyPort;
    if (previous === null || port === null) {
      setWebProxyScope(null);
      await fetchWebProxyPort();
      return;
    }
    state.proxyDraft = publicAccess;
    if (publicAccess) {
      const ok = await showModal({
        title: t("settings.allowPublicTitle"),
        message: t("settings.allowPublicMsg"),
        type: "warn",
        confirmText: t("settings.allowPublicConfirm"),
      });
      if (!ok) {
        state.proxyDraft = state.proxyPublic ?? false;
        return;
      }
    }
    if (state.proxyPublic !== previous || state.proxyPort !== port) return;
    state.saving = true;
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      operationSucceeded(await api.changeWebProxyPort(port, publicAccess));
      setWebProxyScope(publicAccess);
      showToast(publicAccess ? t("settings.publicEnabled") : t("settings.publicDisabled"));
    } catch (error) {
      showToast(t("settings.scopeChangeFailed", { error: errorMessage(error) }));
      await fetchWebProxyPort();
    }
  }

  async function migrateEntry(): Promise<void> {
    if (state.entryLogin !== false || !await showModal({
      title: t("settings.migrateTitle"),
      message: t("settings.migrateMsg"),
      confirmText: t("settings.migrateConfirm"),
      type: "warn",
    })) return;
    state.saving = true;
    try {
      const token = localStorage.getItem('qwenpaw_auth_token');
      if (!token) throw new Error(t("settings.migrateNoToken"));
      operationSucceeded(await api.migrateEntry(token));
      state.entryLogin = true;
      if (localStorage.getItem('qwenpaw_auth_token') === token) localStorage.removeItem('qwenpaw_auth_token');
      showToast(t("settings.migrateDone"));
    } catch (error) { showToast(t("settings.migrateFailed", { error: errorMessage(error) })); }
    await fetchWebProxyPort();
  }

  async function resetAuth(password: string): Promise<void> {
    state.saving = true;
    try {
      operationSucceeded(await api.resetAuth(password));
      showToast(t("settings.resetDone"));
    } catch (error) {
      showToast(t("settings.resetFailed", { error: errorMessage(error) }));
    }
  }

  async function pickMirror(value: string): Promise<void> {
    if (mirror.saving || !MIRROR_SPECS.some(item => item.value === value)) return;
    mirror.saving = true;
    mirror.open = false;
    ++mirrorRead;
    try {
      operationSucceeded(await api.setMirror(value));
      mirror.value = value;
      mirror.ready = true;
      await fetchVersionList(true);
      showToast(t("mirror.saved"));
    } catch (error) {
      showToast(t("mirror.saveFailed", { error: errorMessage(error) }));
      await loadMirror();
    } finally { mirror.saving = false; }
  }

  async function loadMirror(): Promise<void> {
    const request = ++mirrorRead;
    try {
      const {data, ok} = await api.getMirror();
      if (request !== mirrorRead) return;
      mirror.ready = ok && data.success === true;
      if (mirror.ready) mirror.value = canonicalMirror(data.mirror || '');
    } catch { if (request === mirrorRead) mirror.ready = false; }
  }

  return {
    changeServicePort: () => runAction(changeServicePort),
    fetchWebProxyPort: () => state.saving ? Promise.resolve() : fetchWebProxyPort(),
    changeWebProxyPort: () => runAction(changeWebProxyPort),
    setWebProxyPublic: (value: boolean) => runAction(() => setWebProxyPublic(value)),
    migrateEntry: () => runAction(migrateEntry),
    resetAuth: (password: string) => runAction(() => resetAuth(password)),
    loadMirror,
    mirror,
    get mirrors() { return mirrors.value; },
    toggleMirrorDropdown: () => { mirror.open = !mirror.open; if (mirror.open && !mirror.ready) void loadMirror(); },
    pickMirror,
    currentMirror: () => mirror.value,
    destroy: () => {
      if (statusRefreshTimer !== null) clearTimeout(statusRefreshTimer);
      statusRefreshTimer = null;
    },
  };
}
