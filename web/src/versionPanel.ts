import type { ApiClient, VersionItem } from "./api";
import { renderHistoryItems, renderVersionCards } from "./components";

interface VersionPanelOptions {
  api: Pick<ApiClient, "versionList">;
  showToast(message: string): void;
  supervisorReachable(): boolean | null;
  updateOverviewInstaller(): void;
}

export function createVersionPanel({ api, showToast, supervisorReachable, updateOverviewInstaller }: VersionPanelOptions) {
  let rawVersionList: VersionItem[] = [];
  let userPickedVersion = false;
  let selectedVersion = "";
  let latestStableVer = "";
  let latestBetaVer = "";
  let currentRunningVersion = "";
  let pendingInstallAction = "";

  function compareVersion(v1: string, v2: string): number {
    function parse(value: string): number[] {
      if (!value) return [0, 0, 0, 0, 0];
      const clean = value.replace(/^v/, "").trim();
      const match = clean.match(/^(\d+)\.(\d+)(?:\.(\d+))?(.*)$/);
      if (!match) return [0, 0, 0, 0, 0];
      const major = Number.parseInt(match[1], 10);
      const minor = Number.parseInt(match[2], 10);
      const patch = Number.parseInt(match[3] || "0", 10);
      const extra = match[4] || "";
      let stage = 4;
      let num = 0;
      if (extra.includes(".post")) {
        stage = 5;
        num = Number.parseInt(extra.replace(/\D/g, "") || "0", 10);
      } else if (extra) {
        const suffix = extra.replace(/^[.-]/, "").toLowerCase();
        if (suffix.startsWith("dev")) stage = 0;
        else if (suffix.startsWith("a") || suffix.startsWith("alpha")) stage = 1;
        else if (suffix.startsWith("b") || suffix.startsWith("beta")) stage = 2;
        else if (suffix.startsWith("rc") || suffix.startsWith("c") || suffix.startsWith("pre")) stage = 3;
        num = Number.parseInt(suffix.replace(/\D/g, "") || "0", 10);
      }
      return [major, minor, patch, stage, num];
    }
    const left = parse(v1);
    const right = parse(v2);
    for (let index = 0; index < 5; index++) {
      if (left[index] > right[index]) return 1;
      if (left[index] < right[index]) return -1;
    }
    return 0;
  }

  function isDowngradeOf(current: string, selected: string): boolean {
    const item = rawVersionList.find(version => version.version === selected);
    if (item?.action) return item.action === "downgrade";
    const currentIndex = rawVersionList.findIndex(version => version.version === current);
    const selectedIndex = rawVersionList.findIndex(version => version.version === selected);
    if (currentIndex >= 0 && selectedIndex >= 0) return selectedIndex > currentIndex;
    return compareVersion(current, selected) > 0;
  }

  function isSameVersion(current: string, selected: string): boolean {
    if (!current || !selected) return false;
    const item = rawVersionList.find(version => version.version === selected);
    if (item?.action === "reinstall") return true;
    return current.replace(/^v/i, "").trim() === selected.replace(/^v/i, "").trim();
  }

  function currentInstallAction(): string {
    if (!currentRunningVersion) return "安装";
    const item = rawVersionList.find(version => version.version === selectedVersion);
    if (item?.action) {
      if (item.action === "reinstall") return "重新安装";
      if (item.action === "downgrade") return "降级";
      if (item.action === "upgrade") return "升级";
      if (item.action === "install") return "安装";
    }
    if (isSameVersion(currentRunningVersion, selectedVersion)) return "重新安装";
    return isDowngradeOf(currentRunningVersion, selectedVersion) ? "降级" : "升级";
  }

  function updateUpgradeButtonText(): void {
    const button = document.getElementById("btnUpgradeAction") as HTMLButtonElement | null;
    const warnBar = document.getElementById("downgradeWarnBar");
    if (!button) return;
    button.disabled = !selectedVersion || supervisorReachable() !== true;
    button.title = supervisorReachable() === false ? "控制台无法连接监督者，暂不可操作" : "";
    if (!selectedVersion) {
      button.innerText = "正在加载可用版本...";
      return;
    }
    if (!currentRunningVersion) {
      button.innerText = `安装内核 v${selectedVersion}`;
      button.className = "btn btn-primary btn-sm";
      if (warnBar) warnBar.hidden = true;
      return;
    }
    if (isSameVersion(currentRunningVersion, selectedVersion)) {
      button.innerText = `重新安装 v${selectedVersion}`;
      button.className = "btn btn-primary btn-sm";
      if (warnBar) warnBar.hidden = true;
      return;
    }
    if (isDowngradeOf(currentRunningVersion, selectedVersion)) {
      button.innerText = `降级至 v${selectedVersion}`;
      button.className = "btn btn-danger btn-sm";
      if (warnBar) warnBar.hidden = false;
    } else {
      button.innerText = `执行更新至 v${selectedVersion}`;
      button.className = "btn btn-primary btn-sm";
      if (warnBar) warnBar.hidden = true;
    }
  }

  function refreshVersionCards(): void {
    const container = document.getElementById("versionCardsContainer");
    const historyDrop = document.getElementById("historyDropdown");
    if (!container) return;
    renderVersionCards(latestBetaVer, latestStableVer, selectedVersion, pickVersion, container);
    container.querySelector(".log-empty")?.remove();
    if (historyDrop) {
      const historyList = rawVersionList.filter(version => version.version !== latestBetaVer && version.version !== latestStableVer);
      renderHistoryItems(historyList.slice(0, 10), pickVersion, historyDrop);
    }
    updateUpgradeButtonText();
    updateOverviewInstaller();
  }

  function pickVersion(version: string): void {
    if (!version) return;
    userPickedVersion = true;
    selectedVersion = version;
    refreshVersionCards();
    const drop = document.getElementById("historyDropdown");
    if (drop) drop.classList.remove("show");
    document.querySelector(".history-versions-toggle")?.setAttribute("aria-expanded", "false");
  }

  function toggleHistoryDropdown(): void {
    const drop = document.getElementById("historyDropdown");
    if (!drop) return;
    const expanded = drop.classList.toggle("show");
    document.querySelector(".history-versions-toggle")?.setAttribute("aria-expanded", String(expanded));
  }

  function showVersionSourceError(detail = ""): false {
    const alert = document.getElementById("versionSourceStatus");
    const message = "当前下载源不可用，无法获取版本列表。请更换下载源，或检查 NAS 网络、DNS 与代理设置后重试。";
    if (alert) {
      alert.innerText = detail ? `${message}（${detail}）` : message;
      alert.hidden = false;
    }
    showToast(message);
    return false;
  }

  async function fetchVersionList(): Promise<boolean> {
    try {
      const result = await api.versionList();
      const { data } = result;
      if (result.ok && data.success && Array.isArray(data.versions)) {
        rawVersionList = data.versions;
        latestStableVer = data.latest_stable || "";
        latestBetaVer = rawVersionList.find(version => version.is_beta)?.version || "";
        if (!userPickedVersion) selectedVersion = latestStableVer;
        const alert = document.getElementById("versionSourceStatus");
        if (alert) alert.hidden = true;
        refreshVersionCards();
        return true;
      }
      return showVersionSourceError(data.message || `HTTP ${result.status}`);
    } catch (error) {
      console.warn("fetchVersionList err:", error);
      return showVersionSourceError((error as Error).message);
    }
  }

  async function checkOnlineVersions(): Promise<void> {
    showToast("正在在线检查 PyPI 最新版本...");
    if (await fetchVersionList()) {
      showToast(`检查完成！最新稳定: v${latestStableVer}，最新测试: v${latestBetaVer}`);
    }
  }

  return {
    fetchVersionList,
    checkOnlineVersions,
    refreshVersionCards,
    pickVersion,
    toggleHistoryDropdown,
    updateUpgradeButtonText,
    currentInstallAction,
    isSameVersion,
    isDowngradeOf,
    selectedVersion: () => selectedVersion,
    latestStableVersion: () => latestStableVer,
    latestBetaVersion: () => latestBetaVer,
    currentRunningVersion: () => currentRunningVersion,
    setCurrentRunningVersion: (version: string) => { currentRunningVersion = version; },
    pendingInstallAction: () => pendingInstallAction,
    setPendingInstallAction: (action: string) => { pendingInstallAction = action; },
  };
}
