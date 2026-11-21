import "./style.css";
import { createManagementState } from "./managementState";
import { createApp, reactive } from "vue";
import App from "./App.vue";
import { panelKey } from "./panelContext";
declare const __PANEL_TEST__: boolean;
import { createApiClient, type StatusResponse } from "./api";
import { createAuthorizedDirsPanel } from "./authorizedDirsPanel";
import { createInstallCoordinator } from "./installCoordinator";
import { createInstallProgressPanel } from "./installProgressPanel";
import { createLogPanel } from "./logPanel";
import { createPollingLifecycle } from "./pollingLifecycle";
import { createNavigation, type ConsoleView } from "./navigation";
import { operationSucceeded } from "./apiResult";
import { createPrompt } from "./modal";
import { createSettingsPanel } from "./settingsPanel";
import { createStatusPanel } from "./statusPanel";
import { createVersionPanel } from "./versionPanel";

const api = createApiClient({ pathname: window.location.pathname, cookie: () => document.cookie });

const prompt = createPrompt();
const showModal = prompt.show;

// 灵动岛风格置顶 Toast
// 定时器必须单例:每次 showToast 都新建 setTimeout 会让先前那条的计时器
// 继续跑,在后一条刚显示时把它移除 —— 连续提示(如"正在保存…"→"已保存")
// 会出现闪一下就消失。改为始终重置同一个计时器。
const toast = reactive({ message: "", visible: false });
let toastTimer: ReturnType<typeof setTimeout> | null = null;
function showToast(msg: string) {
  toast.message = msg;
  toast.visible = true;
  if (toastTimer !== null) clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toast.visible = false;
    toastTimer = null;
  }, 2500);
}

function updateOverviewInstaller(): void {
  if (navigation.section === 'overview' && needsInstaller()) loadDeployment();
}
function needsInstaller(): boolean {
  const v = statusPanel.view;
  return v.data?.installed === false && !v.recovery && !v.busyInstall;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}


const management = createManagementState();
const settingsPanel = createSettingsPanel({
  api, state: management, showModal, showToast, errorMessage,
  supervisorReachable,
  fetchStatus, fetchLogs: () => fetchLogs(), fetchVersionList,
});
const fetchWebProxyPort = settingsPanel.fetchWebProxyPort;
const loadMirror = settingsPanel.loadMirror;
const toggleMirrorDropdown = settingsPanel.toggleMirrorDropdown;
const currentMirror = settingsPanel.currentMirror;

async function uninstallKernel() {
  if (!versionPanel.currentRunningVersion()) {
    showToast(statusPanel.view.data?.installed ? '尚未确认当前内核版本，请刷新后再操作' : '当前未安装 QwenPaw 内核');
    return;
  }
  const ok = await showModal({
    title: "仅卸载 QwenPaw 内核",
    message: `将停止服务并删除 QwenPaw v${versionPanel.currentRunningVersion()} 的运行环境与版本钉。\n\n• 用户配置、会话记忆与管理员认证均完整保留，之后可随时重新安装。\n• 注意：受飞牛 fnOS 桌面会话缓存限制，桌面图标隐藏可能需刷新浏览器页面生效。确认继续？`,
    type: "danger",
    confirmText: "仅卸载内核，保留数据"
  });
  if (!ok || recoveryReason() || supervisorReachable() !== true) return;
  try {
    showToast("正在卸载内核并保留用户数据...");
    const data = operationSucceeded(await api.uninstallKernel());
    showToast(data.message || "内核已卸载");
    await fetchStatus();
    fetchLogs();
  } catch (e) {
    console.error("uninstallKernel err:", e);
    showToast("卸载请求失败: " + errorMessage(e));
  }
}

const serviceControl = reactive({ pending: '' });
async function controlService(act: "start" | "stop" | "restart"): Promise<void> {
  if (serviceControl.pending) return;
  const actionName = act === "restart" ? "重启" : (act === "stop" ? "停止" : "启动");
  const ok = await showModal({
    title: `${actionName}服务确认`,
    message: `确定要立即${actionName} QwenPaw 服务吗？${act === 'restart' ? '重启期间连接会短暂中断。' : ''}`,
    type: act === 'stop' ? 'danger' : 'primary',
    confirmText: `确认${actionName}`
  });
  if (!ok || serviceControl.pending || recoveryReason() || supervisorReachable() !== true) return;
  serviceControl.pending = `正在${actionName}服务…`;
  try {
    showToast(serviceControl.pending);
    const data = operationSucceeded(await api.controlService(act));
    showToast(data.message || `${actionName}指令已成功执行`);
    await fetchStatus();
    void fetchLogs();
  } catch (e) {
    showToast("操作失败: " + errorMessage(e));
  } finally {
    serviceControl.pending = '';
  }
}

const versionPanel = createVersionPanel({
  api, showToast,
});
function fetchVersionList(sourceChanged = false): Promise<boolean> { return versionPanel.fetchVersionList(sourceChanged); }
async function checkOnlineVersions(): Promise<void> {
  if (versionPanel.view.loading) return;
  deploymentLoaded = true;
  if (!settingsPanel.mirror.ready) void loadMirror();
  await versionPanel.checkOnlineVersions();
}
const pickVersion = versionPanel.pickVersion;
const currentInstallAction = versionPanel.currentInstallAction;

const statusPanel = createStatusPanel({ api, state: management, versionPanel, errorMessage, updateOverviewInstaller });
function fetchStatus(snapshot?: StatusResponse): Promise<void> { return statusPanel.refresh(snapshot); }
function supervisorReachable(): boolean | null { return statusPanel.reachable(); }
function recoveryReason(): string { return statusPanel.recoveryReason(); }

const installProgress = createInstallProgressPanel({
  api, showToast, errorMessage, fetchStatus, fetchLogs: () => fetchLogs(),
  currentInstallAction,
  selectedVersion: versionPanel.selectedVersion,
  pendingInstallAction: versionPanel.pendingInstallAction,
});
const installCoordinator = createInstallCoordinator({
  api, progress: installProgress, versionPanel, currentMirror, recoveryReason,
  supervisorReachable, showModal, showToast, errorMessage,
});
function checkOrUpgrade() {
  if (statusPanel.view.data?.installed && !versionPanel.currentRunningVersion()) {
    showToast('尚未确认当前内核版本，请刷新后再操作');
    return;
  }
  return installCoordinator.checkOrUpgrade();
}
const attachRunningInstall = installProgress.attachRunning;
const closeUpgradeModal = installProgress.close;
const minimizeUpgradeModal = installProgress.minimize;
const showTaskDetails = installProgress.showTaskDetails;
const updateTaskCenterButton = installProgress.updateTaskCenterButton;


const navigation = createNavigation(enterView);
const logPanel = createLogPanel({ api, showToast, confirm: showModal, errorMessage, visible: () => navigation.section === 'logs' });
const fetchLogs = logPanel.refresh;
const clearTerminalLogs = logPanel.clear;
function toggleAutoRefresh(enabled: boolean): void {
  logPanel.setAutoRefresh(enabled);
  syncStatusEvents();
}



const webEntry = reactive({ open: false, url: "about:blank" });
let webEntryHistoryPushed = false;
let webTrigger: HTMLElement | null = null;
function closeWebEntryOverlay(): void {
  if (!webEntry.open) return;
  webEntry.open = false;
  webEntry.url = "about:blank";
  document.documentElement.classList.remove("web-entry-open");
  webEntryHistoryPushed = false;
  const trigger = webTrigger;
  webTrigger = null;
  // History traversal restores browser focus after popstate, not inside it.
  requestAnimationFrame(() => trigger?.focus({ preventScroll: true }));
}
function dismissWeb(): void {
  if (webEntryHistoryPushed && history.state?.qwenpawWebEntry) history.back();
  else closeWebEntryOverlay();
}
function openWebPage(url: string, trigger: HTMLElement | null): void {
  if (!/FNAppType\//.test(navigator.userAgent || "")) {
    window.open(url, "_blank", "noopener");
    return;
  }
  webTrigger = trigger;
  if (!webEntry.open) {
    try {
      history.pushState({ ...history.state, qwenpawWebEntry: true }, "", location.href);
      webEntryHistoryPushed = true;
    } catch { webEntryHistoryPushed = false; }
  }
  webEntry.url = url;
  webEntry.open = true;
  document.documentElement.classList.add("web-entry-open");
}
function openConsole(event: MouseEvent): void {
  const link = event.currentTarget;
  if (!(link instanceof HTMLAnchorElement) || link.getAttribute("href") !== "/app/qwenpaw/web/") return;
  event.preventDefault();
  openWebPage("/app/qwenpaw/web/", link);
}
function handleOverlayKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape" && webEntry.open) dismissWeb();
}
window.addEventListener("popstate", closeWebEntryOverlay);
document.addEventListener("keydown", handleOverlayKeydown);
let statusEvents: EventSource | null = null;
let eventQuery = '';

function syncStatusEvents(): void {
  if (document.hidden) {
    statusEvents?.close();
    statusEvents = null;
    return;
  }
  const logs = logPanel.watching();
  const query = logs ? '?watch=logs' : navigation.section === 'overview' ? '?watch=metrics' : '';
  if (typeof EventSource !== 'function' || (statusEvents && eventQuery === query)) return;
  statusEvents?.close();
  statusEvents = null;
  eventQuery = query;
  let stream: EventSource;
  try { stream = new EventSource(`${api.base}/events${query}`); }
  catch { return; }
  statusEvents = stream;
  if (query === '?watch=metrics') stream.addEventListener('metrics', event => {
    if (statusEvents === stream) statusPanel.consumeMetrics((event as MessageEvent).data);
  });
  if (logs) stream.addEventListener('log-change', () => {
    if (statusEvents === stream) logPanel.notifyChange();
  });
  stream.addEventListener("snapshot", event => {
    if (statusEvents !== stream) return;
    void statusPanel.consumeSnapshot((event as MessageEvent).data);
  });
  stream.addEventListener("error", () => {
    if (statusEvents !== stream) return;
    void statusPanel.recoverEvents();
  });
}

const authorizedDirsPanel = createAuthorizedDirsPanel({ showToast, confirm: showModal });
const statusPolling = createPollingLifecycle({ refresh: fetchStatus, intervalMs: 30000 });

const visited = reactive({ configuration: false, authorization: false });
const panel = {
  toast, webEntry,
  navigation, visited, prompt, management, settings: settingsPanel, dirs: authorizedDirsPanel, status: statusPanel,
  versions: versionPanel, logs: logPanel, progress: installProgress,
  showTaskDetails, serviceControl, controlService, checkOrUpgrade,
  checkOnlineVersions, needsInstaller, toggleMirrorDropdown, attachRunningInstall,
  closeUpgradeModal, minimizeUpgradeModal, toggleAutoRefresh, clearTerminalLogs, uninstallKernel,
  openConsole, dismissWeb, openBackup: () => openWebPage("/app/qwenpaw/web/backups", document.getElementById("openBackupBtn")),
  exportLogs: logPanel.exportLogs, copyLogs: logPanel.copyLogs,
};
export type PanelContext = typeof panel;
const app = createApp(App).provide(panelKey, panel);
app.onUnmount(() => {
  navigation.stop();
  statusPolling.destroy();
  statusPanel.destroy();
  logPanel.destroy();
  statusEvents?.close();
  statusEvents = null;
  if (toastTimer !== null) {
    clearTimeout(toastTimer);
    toastTimer = null;
  }
  window.removeEventListener("popstate", closeWebEntryOverlay);
  document.removeEventListener("keydown", handleOverlayKeydown);
  document.removeEventListener("visibilitychange", resumeVisiblePage);
  window.removeEventListener("pageshow", resumeVisiblePage);
});
app.mount('#consoleRoot');
logPanel.connect();
updateTaskCenterButton();
void authorizedDirsPanel.restorePending();
let deploymentLoaded = false;
function loadDeployment(): void {
  if (deploymentLoaded) return;
  deploymentLoaded = true;
  void loadMirror();
  void fetchVersionList();
}
function enterView(view: ConsoleView | null): void {
  logPanel.viewChanged();
  syncStatusEvents();
  if (view === null || view === 'version' || (view === 'overview' && needsInstaller())) loadDeployment();
  if (view === null || view === 'configuration') {
    visited.configuration = true;
    void fetchWebProxyPort();
  }
  if (view === null || view === 'authorization') {
    visited.authorization = true;
    void authorizedDirsPanel.refresh();
  }
}
navigation.start();
fetchStatus();
statusPolling.start();

function resumeVisiblePage(): void {
  if (document.hidden) {
    syncStatusEvents();
    return;
  }
  syncStatusEvents();
  installProgress.resume();
}
document.addEventListener("visibilitychange", resumeVisiblePage);
window.addEventListener("pageshow", resumeVisiblePage);

installProgress.restore();

if (__PANEL_TEST__) Object.assign(window, { fetchStatus, fetchLogs, pickVersion, clearTerminalLogs,
  checkOrUpgrade, checkOnlineVersions, attachRunningInstall });
