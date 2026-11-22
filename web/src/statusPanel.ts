import type { ApiClient, StatusResponse } from "./api";
import { shallowReactive, watch } from "vue";
import type { StatusData } from "./statusStore";
import { formatUptime, isStatusSnapshot } from "./statusStore";
import type { createVersionPanel } from "./versionPanel";
import { authPresentation, type ManagementState } from "./managementState";
import { currentLocale, t } from "./i18n";

interface StatusPanelOptions {
  state: ManagementState;
  api: Pick<ApiClient, "status">;
  versionPanel: ReturnType<typeof createVersionPanel>;
  errorMessage(error: unknown): string;
  updateOverviewInstaller(): void;
}

type CheckedState = "idle" | "ok" | "error";
type VersionState = "initial" | "installed" | "pending" | "uninstalled" | "unavailable";

export function createStatusPanel({ api, state, versionPanel, errorMessage, updateOverviewInstaller }: StatusPanelOptions) {
  let statusReqSeq = 0;
  let lastEventRecoveryAt = -Infinity;
  let eventRecovery: Promise<void> | null = null;
  const raw = shallowReactive({
    reachable: null as boolean | null,
    recovery: "",
    data: null as StatusData | null,
    busyInstall: false,
    versionState: "initial" as VersionState,
    versionValue: "",
    checkedState: "idle" as CheckedState,
    checkedAt: null as Date | null,
    uptime: "",
  });
  const view = {
    get reachable() { return raw.reachable; },
    get recovery() { return raw.recovery; },
    get data() { return raw.data; },
    get busyInstall() { return raw.busyInstall; },
    get uptime() { return raw.uptime; },
    get version() {
      if (raw.versionState === "installed") return raw.versionValue;
      if (raw.versionState === "pending") return t("status.versionPending");
      if (raw.versionState === "uninstalled") return t("status.versionUninstalled");
      if (raw.versionState === "unavailable") return t("status.unavailable");
      return "—";
    },
    get checked() {
      if (raw.checkedState === "ok" && raw.checkedAt) {
        return t("status.lastSync", { time: raw.checkedAt.toLocaleTimeString(currentLocale.value, { hour12: false }) });
      }
      if (raw.checkedState === "error") return t("status.syncFailed");
      return t("status.waitingSync");
    },
  };

  let uptimeTimer: ReturnType<typeof setInterval> | null = null;
  function syncUptimeTimer(): void {
    const running = !!raw.data?.startAt;
    if (running && uptimeTimer === null) {
      uptimeTimer = window.setInterval(() => {
        if (!document.hidden) updateUptime();
      }, 1000);
    } else if (!running && uptimeTimer !== null) {
      window.clearInterval(uptimeTimer);
      uptimeTimer = null;
    }
  }
  function updateUptime(): void {
    raw.uptime = raw.data?.startAt
      ? formatUptime(Math.max(0, Math.floor(Date.now() / 1000) - raw.data.startAt), currentLocale.value)
      : "";
    syncUptimeTimer();
  }

  const stopLocaleWatch = watch(currentLocale, () => {
    if (raw.data?.startAt) updateUptime();
    if (raw.reachable === false) state.servicePort = t("status.unavailable");
  });

  async function fetchStatus(snapshot?: StatusResponse): Promise<void> {
    if (document.hidden) return;
    if (snapshot !== undefined && !isStatusSnapshot(snapshot)) {
      await recoverInvalidEvent();
      return;
    }
    // 每个有效快照也推进请求序号，淘汰已在途 GET 的成功和失败。
    const mySeq = ++statusReqSeq;
    try {
      let data: StatusResponse;
      if (snapshot) {
        data = snapshot;
      } else {
        const result = await api.status();
        if (!result.ok) throw new Error("HTTP " + result.status);
        if (result.data?.success !== true || typeof result.data.installed !== "boolean"
          || typeof result.data.running !== "boolean") throw new Error(t("status.invalidSnapshot"));
        data = result.data;
      }
      if (mySeq !== statusReqSeq || document.hidden) return;
      raw.reachable = state.serviceReachable = true;
      raw.recovery = data.install?.recovery_required === true ? data.install.recovery_error?.trim() || t("status.recoveryFallback") : "";
      raw.busyInstall = !!data.install?.running;
      if (data.installed) {
        raw.versionState = data.version ? "installed" : "pending";
        raw.versionValue = data.version || "";
      } else {
        raw.versionState = "uninstalled";
        raw.versionValue = "";
      }
      raw.data = { installed: data.installed, recoveryReason: raw.recovery, running: !!data.running,
        pid: String(data.pid ?? "-"), startAt: data.running && data.startAt ? data.startAt : null,
        cpu: typeof data.cpu === "number" ? data.cpu : null, memKB: typeof data.memKB === "number" ? data.memKB : null };
      raw.checkedState = "ok";
      raw.checkedAt = new Date();
      versionPanel.setCurrentRunningVersion(data.installed ? data.version || "" : "");
      state.auth = authPresentation(!data.installed ? "unknown" : data.authStatus || (data.authEnabled ? "protected" : "disabled"));
      state.servicePort = String(data.port || 18088);
      state.backupAvailable = !!data.installed && !!data.running && !raw.recovery;
    } catch (error) {
      if (mySeq !== statusReqSeq || document.hidden) return;
      console.error("fetchStatus err:", error);
      raw.reachable = state.serviceReachable = state.backupAvailable = false;
      raw.checkedState = "error";
      raw.checkedAt = null;
      raw.busyInstall = false;
      raw.versionState = "unavailable";
      raw.versionValue = "";
      raw.data = { installed: null, running: false, pid: "-", startAt: null, cpu: null, memKB: null, degraded: true, degradedDetail: errorMessage(error) };
      state.servicePort = t("status.unavailable");
      state.auth = authPresentation("offline");
    }
    updateUptime();
    updateOverviewInstaller();
  }

  // 坏帧/连续断线共用一次有界恢复；失败后仍由 30s 轮询保证进展。
  function recoverInvalidEvent(): Promise<void> {
    if (document.hidden) return Promise.resolve();
    if (eventRecovery) return eventRecovery;
    const now = Date.now();
    if (now - lastEventRecoveryAt < 30000) return Promise.resolve();
    lastEventRecoveryAt = now;
    eventRecovery = fetchStatus().finally(() => { eventRecovery = null; });
    return eventRecovery;
  }

  function consumeSnapshot(rawData: unknown): Promise<void> {
    if (document.hidden) return Promise.resolve();
    if (typeof rawData === "string") {
      try {
        const data: unknown = JSON.parse(rawData);
        if (isStatusSnapshot(data)) return fetchStatus(data);
      } catch {}
    }
    return recoverInvalidEvent();
  }

  return {
    refresh: fetchStatus,
    consumeSnapshot,
    consumeMetrics(rawMetrics: string) {
      const current = raw.data;
      if (!current?.running || document.hidden) return;
      try {
        const m = JSON.parse(rawMetrics);
        if (m.pid === 0) { void recoverInvalidEvent(); return; }
        if (String(m.pid) !== current.pid || m.startAt !== current.startAt ||
          !(m.cpu === null || (Number.isFinite(m.cpu) && m.cpu >= 0 && m.cpu <= 100)) ||
          !Number.isSafeInteger(m.memKB) || m.memKB < 0) return;
        raw.data = { ...current, cpu: m.cpu, memKB: m.memKB };
      } catch { /* A partial resource frame never changes process or action state. */ }
    },
    recoverEvents: recoverInvalidEvent,
    destroy: () => {
      stopLocaleWatch();
      if (uptimeTimer !== null) {
        window.clearInterval(uptimeTimer);
        uptimeTimer = null;
      }
    },
    view, reachable: () => raw.reachable,
    recoveryReason: () => raw.recovery,
  };
}
