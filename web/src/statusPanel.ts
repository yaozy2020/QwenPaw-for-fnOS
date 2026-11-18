import type { ApiClient, StatusResponse } from "./api";
import { renderStatusCard } from "./components";
import { formatUptime, isStatusSnapshot } from "./statusStore";
import type { createVersionPanel } from "./versionPanel";

interface StatusPanelOptions {
  api: Pick<ApiClient, "status">;
  versionPanel: ReturnType<typeof createVersionPanel>;
  errorMessage(error: unknown): string;
  updateOverviewInstaller(): void;
}

export function createStatusPanel({ api, versionPanel, errorMessage, updateOverviewInstaller }: StatusPanelOptions) {
  let statusReqSeq = 0;
  let lastEventRecoveryAt = -Infinity;
  let eventRecovery: Promise<void> | null = null;
  let supervisorReachable: boolean | null = null;

  function setServiceActions(running: boolean, installed: boolean, degraded: boolean): void {
    const start = document.getElementById("btnStartService") as HTMLButtonElement | null;
    const restart = document.getElementById("btnRestartService") as HTMLButtonElement | null;
    const stop = document.getElementById("btnStopService") as HTMLButtonElement | null;
    if (!start || !restart || !stop) return;

    const usable = installed && !degraded;
    start.hidden = !(usable && !running);
    restart.hidden = !(usable && running);
    stop.hidden = !(usable && running);

    for (const button of [start, restart, stop]) {
      button.disabled = degraded;
      button.title = degraded ? "控制台无法连接监督者，暂不可操作" : "";
    }
    const changePort = document.getElementById("btnChangeServicePort") as HTMLButtonElement | null;
    if (changePort) {
      changePort.disabled = degraded;
      changePort.title = degraded ? "服务状态尚未同步，请先刷新" : "";
    }
    const entry = document.getElementById("btnWebConsole") as HTMLAnchorElement | null;
    const label = document.getElementById("webConsoleText");
    if (entry) entry.href = degraded || (installed && !running) ? "#overview"
      : !installed ? "#version" : "/app/qwenpaw/web/";
    if (label) label.innerText = degraded ? "检查服务状态" : !installed ? "安装并开始使用"
      : running ? "进入 Web 对话" : "先启动服务";
    const mobileLabel = document.getElementById("webConsoleMobileText");
    if (mobileLabel) mobileLabel.innerText = degraded ? "检查" : !installed ? "安装"
      : running ? "对话" : "启动";
    if (entry && label) entry.setAttribute("aria-label", label.innerText);
    const overviewEntry = document.getElementById("overviewWebConsole") as HTMLAnchorElement | null;
    if (overviewEntry) overviewEntry.hidden = !(!degraded && installed && running);
    const backup = document.getElementById("openBackupBtn") as HTMLButtonElement | null;
    if (backup) {
      const usable = !degraded && installed && running;
      backup.disabled = !usable;
      backup.title = usable ? "打开 QwenPaw 上游备份管理" : "启动 QwenPaw 后可使用备份管理";
    }
  }

  function syncOverviewDetails(degraded = false): void {
    const fields = {
      overviewVersion: "valVersion",
      overviewPort: "valPort",
      overviewAuth: "valAuthStatus",
    };
    for (const [target, source] of Object.entries(fields)) {
      const targetElement = document.getElementById(target);
      const sourceElement = document.getElementById(source);
      if (targetElement) targetElement.innerText = degraded ? "—" : sourceElement?.innerText || "—";
    }
    const overviewPid = document.getElementById("overviewPid");
    if (overviewPid && degraded) overviewPid.innerText = "—";
  }

  function setOverviewGuide(visible: boolean): void {
    const guide = document.getElementById("overviewGuide");
    if (guide) guide.hidden = !visible;
    const installer = document.getElementById("overviewInstaller");
    if (installer) installer.hidden = !visible;
    updateOverviewInstaller();
  }

  function setStatusUptime(text: string): void {
    const element = document.getElementById("statusUptime");
    if (element) element.innerText = text;
  }

  let runningStartAt: number | null = null;
  let uptimePlaceholder = "";

  function updateUptime(): void {
    setStatusUptime(runningStartAt
      ? "已运行 " + formatUptime(Math.floor(Date.now() / 1000) - runningStartAt)
      : uptimePlaceholder);
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
      const badge = document.getElementById("statusBadge");
      const valVersion = document.getElementById("valVersion");
      const valAuth = document.getElementById("valAuthStatus");

      const notInstalled = data.installed === false;
      const cardMount = document.getElementById("statusCardMount");
      if (cardMount) {
        renderStatusCard({
          installed: data.installed,
          running: Boolean(data.running),
          pid: String(data.pid ?? "-"),
          startAt: data.running && data.startAt ? data.startAt : null,
          cpu: typeof data.cpu === "number" ? data.cpu : null,
          memKB: typeof data.memKB === "number" ? data.memKB : null,
        }, cardMount);
      }
      const overviewPid = document.getElementById("overviewPid");
      if (overviewPid) overviewPid.innerText = data.running ? String(data.pid ?? "-") : "—";
      setServiceActions(Boolean(data.running), data.installed !== false, false);
      const checkedAt = document.getElementById("statusCheckedAt");
      if (checkedAt) checkedAt.innerText = `最近同步 ${new Date().toLocaleTimeString("zh-CN", { hour12: false })}`;
      supervisorReachable = true;

      if (notInstalled) {
        for (const id of ["valAuthStatus", "valAdminStatus"]) {
          const element = document.getElementById(id);
          if (element) element.style.color = "var(--text-muted)";
        }
        if (badge) {
          badge.className = "badge badge-warning";
          badge.innerHTML = `<span class="pulse-dot"></span> 内核未安装`;
        }
        if (valVersion) valVersion.innerText = "未安装";
        versionPanel.setCurrentRunningVersion("");
        setOverviewGuide(true);
      } else {
        setOverviewGuide(false);
      }
      const installTip = document.getElementById("kernelInstallTip");
      if (installTip) installTip.hidden = !notInstalled;
      const uninstallBtn = document.getElementById("btnUninstallKernel");
      if (uninstallBtn) uninstallBtn.hidden = notInstalled;
      versionPanel.updateUpgradeButtonText();

      const runTip = document.getElementById("installRunningTip");
      if (runTip) {
        const modalOpen = document.getElementById("upgradeModal")?.classList.contains("active");
        runTip.hidden = !(data.install && data.install.running && !modalOpen);
      }

      if (!notInstalled && data.running) {
        if (badge) {
          badge.className = "badge badge-success";
          badge.innerHTML = `<span class="pulse-dot"></span> 运行中`;
        }
      } else if (!notInstalled) {
        if (badge) {
          badge.className = "badge badge-danger";
          badge.innerHTML = `<span class="pulse-dot"></span> 已停止`;
        }
      }

      if (valVersion && data.version) {
        valVersion.innerText = data.version;
        versionPanel.setCurrentRunningVersion(data.version);
        versionPanel.updateUpgradeButtonText();
      }
      const valAdmin = document.getElementById("valAdminStatus");
      const valDesc = document.getElementById("valAuthDesc");
      const statusType = notInstalled ? "unknown"
        : data.authStatus || (data.authEnabled ? "protected" : "disabled");

      if (statusType === "uninitialized") {
        if (valAuth) {
          valAuth.innerText = "已开启（待设置密码）";
          valAuth.style.color = "var(--warning)";
        }
        if (valAdmin) {
          valAdmin.innerText = "尚未创建管理员";
          valAdmin.style.color = "var(--warning)";
        }
        if (valDesc) {
          valDesc.innerText = "对话端已要求登录验证，但还没有管理员。请进入对话端完成初始设置；该账号独立于飞牛账号。";
        }
      } else if (statusType === "protected") {
        if (valAuth) {
          valAuth.innerText = "已开启（需要密码）";
          valAuth.style.color = "var(--success)";
        }
        if (valAdmin) {
          valAdmin.innerText = "管理员已创建";
          valAdmin.style.color = "var(--success)";
        }
        if (valDesc) {
          valDesc.innerText = "对话端需要 QwenPaw 独立管理员密码。重置只清除此密码，不影响飞牛账号、用户配置或记忆。";
        }
      } else if (statusType === "disabled") {
        if (valAuth) {
          valAuth.innerText = "未开启（公开访问）";
          valAuth.style.color = "var(--danger-text)";
        }
        if (valAdmin) {
          valAdmin.innerText = "未启用账号验证";
          valAdmin.style.color = "var(--danger-text)";
        }
        if (valDesc) {
          valDesc.innerText = "任何能访问该端口的人无需密码即可使用对话端。如需保护，请在对话端设置 QwenPaw 管理员。";
        }
      } else {
        if (valAuth) {
          valAuth.innerText = "无法确认";
          valAuth.style.color = "var(--text-muted)";
        }
        if (valAdmin) {
          valAdmin.innerText = "无法读取";
          valAdmin.style.color = "var(--text-muted)";
        }
        if (valDesc) {
          valDesc.innerText = "内核未运行或状态接口不可达，暂时无法确认密码状态；这不代表验证已关闭。请启动内核后再确认。";
        }
      }

      runningStartAt = !notInstalled && data.running && data.startAt ? data.startAt : null;
      uptimePlaceholder = "";
      updateUptime();

      const portEl = document.getElementById("valPort");
      const activePort = data.port || "18088";
      if (portEl) portEl.innerText = String(activePort);
      syncOverviewDetails();
    } catch (error) {
      console.error("fetchStatus err:", error);
      if (mySeq !== statusReqSeq || document.hidden) return;
      const checkedAt = document.getElementById("statusCheckedAt");
      if (checkedAt) checkedAt.innerText = "同步失败 · 自动重试中";
      const badge = document.getElementById("statusBadge");
      if (badge) {
        badge.className = "badge badge-warning";
        badge.innerHTML = `<span class="pulse-dot"></span> 状态不可达`;
      }
      const cardMount = document.getElementById("statusCardMount");
      if (cardMount) {
        renderStatusCard({
          installed: null,
          running: false,
          pid: "-",
          startAt: null,
          cpu: null,
          memKB: null,
          degraded: true,
          degradedDetail: errorMessage(error),
        }, cardMount);
      }
      runningStartAt = null;
      uptimePlaceholder = "—";
      updateUptime();
      setServiceActions(false, false, true);

      supervisorReachable = false;
      versionPanel.updateUpgradeButtonText();
      const unBtn = document.getElementById("btnUninstallKernel");
      if (unBtn) unBtn.hidden = true;
      for (const tipId of ["kernelInstallTip", "installRunningTip"]) {
        const tip = document.getElementById(tipId);
        if (tip) tip.hidden = true;
      }
      for (const id of ["valAuthStatus", "valAdminStatus"]) {
        const element = document.getElementById(id);
        if (element) element.style.color = "var(--text-muted)";
      }
      const downWarn = document.getElementById("downgradeWarnBar");
      if (downWarn) downWarn.hidden = true;
      const valVersion = document.getElementById("valVersion");
      if (valVersion) valVersion.innerText = "无法获取";
      const valAuth = document.getElementById("valAuthStatus");
      if (valAuth) valAuth.innerText = "无法获取";
      const valAdmin = document.getElementById("valAdminStatus");
      if (valAdmin) valAdmin.innerText = "无法获取";
      const valPort = document.getElementById("valPort");
      if (valPort) valPort.innerText = "无法获取";
      const valDesc = document.getElementById("valAuthDesc");
      if (valDesc) {
        valDesc.innerText = "控制台无法连接监督者（可能服务重启中或会话已过期）。这不代表内核已停止，稍后会自动重试。";
      }
      syncOverviewDetails(true);
      setOverviewGuide(false);
    }
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
    recoverEvents: recoverInvalidEvent,
    reachable: () => supervisorReachable,
  };
}
