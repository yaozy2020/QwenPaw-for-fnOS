import type { ApiClient, StatusResponse } from "./api";
import { shallowReactive } from "vue";
import type { StatusData } from "./statusStore";
import { formatUptime, isStatusSnapshot } from "./statusStore";
import type { createVersionPanel } from "./versionPanel";

import { authPresentation, type ManagementState } from "./managementState";

interface StatusPanelOptions {
  state: ManagementState;
  api: Pick<ApiClient, "status">;
  versionPanel: ReturnType<typeof createVersionPanel>;
  errorMessage(error: unknown): string;
  updateOverviewInstaller(): void;
}

export function createStatusPanel({ api, state, versionPanel, errorMessage, updateOverviewInstaller }: StatusPanelOptions) {
  let statusReqSeq = 0;
  let lastEventRecoveryAt = -Infinity;
  let eventRecovery: Promise<void> | null = null;
  const view = shallowReactive({
    reachable: null as boolean | null, recovery: '', data: null as StatusData | null,
    version: '—', busyInstall: false, checked: '等待首次同步', uptime: '',
  });
  function updateUptime(): void {
    view.uptime = view.data?.startAt ? formatUptime(Math.max(0, Math.floor(Date.now() / 1000) - view.data.startAt)) : '';
  }

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
          || typeof result.data.running !== "boolean") throw new Error("无效的服务状态");
        data = result.data;
      }
      if (mySeq !== statusReqSeq || document.hidden) return;
      view.reachable = state.serviceReachable = true;
      view.recovery = data.install?.recovery_required === true ? data.install.recovery_error?.trim() || '上一次事务尚未安全收尾' : '';
      view.busyInstall = !!data.install?.running;
      view.version = data.installed ? data.version || '待确认' : '未安装';
      view.data = { installed: data.installed, recoveryReason: view.recovery, running: !!data.running,
        pid: String(data.pid ?? '-'), startAt: data.running && data.startAt ? data.startAt : null,
        cpu: typeof data.cpu === 'number' ? data.cpu : null, memKB: typeof data.memKB === 'number' ? data.memKB : null };
      view.checked = `最近同步 ${new Date().toLocaleTimeString('zh-CN', {hour12:false})}`;
      versionPanel.setCurrentRunningVersion(data.installed ? data.version || '' : '');
      state.auth = authPresentation(!data.installed ? 'unknown' : data.authStatus || (data.authEnabled ? 'protected' : 'disabled'));
      state.servicePort = String(data.port || 18088);
      state.backupAvailable = !!data.installed && !!data.running && !view.recovery;
    } catch (error) {
      if (mySeq !== statusReqSeq || document.hidden) return;
      console.error('fetchStatus err:', error);
      view.reachable = state.serviceReachable = state.backupAvailable = false;
      view.checked = '同步失败 · 自动重试中';
      view.busyInstall = false;
      view.version = '无法获取';
      view.data = { installed: null, running: false, pid: '-', startAt: null, cpu: null, memKB: null, degraded: true, degradedDetail: errorMessage(error) };
      state.servicePort = '无法获取';
      state.auth = authPresentation('offline');
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

  function consumeSnapshot(raw: unknown): Promise<void> {
    if (document.hidden) return Promise.resolve();
    if (typeof raw === "string") {
      try {
        const data: unknown = JSON.parse(raw);
        if (isStatusSnapshot(data)) return fetchStatus(data);
      } catch {}
    }
    return recoverInvalidEvent();
  }

  window.setInterval(() => {
    if (!document.hidden) updateUptime();
  }, 1000);

  return {
    refresh: fetchStatus,
    consumeSnapshot,
    consumeMetrics(raw: string) {
      const current = view.data;
      if (!current?.running || document.hidden) return;
      try {
        const m = JSON.parse(raw);
        if (m.pid === 0) { void recoverInvalidEvent(); return; }
        if (String(m.pid) !== current.pid || m.startAt !== current.startAt ||
          !(m.cpu === null || (Number.isFinite(m.cpu) && m.cpu >= 0 && m.cpu <= 100)) ||
          !Number.isSafeInteger(m.memKB) || m.memKB < 0) return;
        view.data = { ...current, cpu: m.cpu, memKB: m.memKB };
      } catch { /* A partial resource frame never changes process or action state. */ }
    },
    recoverEvents: recoverInvalidEvent,
    view, reachable: () => view.reachable,
    recoveryReason: () => view.recovery,
  };
}
