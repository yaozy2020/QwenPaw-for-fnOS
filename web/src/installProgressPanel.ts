import { nextTick, shallowReactive } from "vue";
import type { ApiClient, LogEntry } from "./api";
import type { InstallStep, InstallStepsState } from "./views/InstallSteps.vue";
import {
  createInstallTask,
  acceptInstallTask,
  observeInstallStatus,
  persistInstallTask,
  restoreInstallTask,
  clearStoredInstallTask,
} from "./installTask.mjs";

interface InstallProgressPanelOptions {
  api: Pick<ApiClient, "upgradeStatus" | "upgradeLogs">;
  showToast(message: string): void;
  errorMessage(error: unknown): string;
  fetchStatus(): Promise<void>;
  fetchLogs(): void;
  currentInstallAction(): string;
  selectedVersion(): string;
  pendingInstallAction(): string;
}

export function createInstallProgressPanel({
  api,
  showToast,
  errorMessage,
  fetchStatus,
  fetchLogs,
  currentInstallAction,
  selectedVersion,
  pendingInstallAction,
}: InstallProgressPanelOptions) {
  const view = shallowReactive({ open: false, owned: false, spinner: true, icon: false, success: false, minimize: true, close: false,
    title: '', subtitle: '', lines: [] as LogEntry[], empty: '', steps: { current: null, outcome: null } as InstallStepsState });
  const UPGRADE_POLL_MAX_MS = 10 * 60 * 1000;
  const UPGRADE_POLL_FAIL_MAX = 6;
  const INSTALL_STEP_NAMES: InstallStep[] = ["downloading", "installing", "starting"];

  function installTaskStorage(): Storage | null {
    try { return window.sessionStorage; } catch { return null; }
  }

  let upgradePollTimer: ReturnType<typeof setInterval> | null = null;
  let upgradeStartedAt = 0;
  let pollFailureStreak = 0;
  let upPollLastAt = 0;
  let upStatusPollInFlight = false;
  let upLogsPollInFlight = false;
  let installTask = restoreInstallTask(installTaskStorage()) || createInstallTask({ attach: false });
  let upgradeRunId = 0;
  let upgradeTerminal = false;
  let upgradePreviousFocus: HTMLElement | null = null;
  let upgradeModalOpenedAt = 0;
  let upgradeLogsRendered = false;
  function paintTaskLog(lines: LogEntry[], fallback: string) {
    view.lines = lines;
    view.empty = fallback;
  }

  function stopUpgradePolling(): void {
    if (upgradePollTimer !== null) {
      clearInterval(upgradePollTimer);
      upgradePollTimer = null;
    }
  }

  function showUpgradePollingFailure(timedOut: boolean): void {
    view.spinner = false;
    view.title = timedOut ? '任务仍未结束' : '与控制台连接中断';
    view.subtitle = timedOut ? '安装任务仍在后台执行，可关闭本窗口；稍后在控制台查看进度或日志' : '无法连接监督者。任务可能仍在后台执行，可关闭本窗口稍后重试';
    view.icon = view.close = true;
    view.success = false;
  }
  function installStepOf(phase: unknown): InstallStep | null {
    return INSTALL_STEP_NAMES.find(step => step === phase) ?? null;
  }

  function setInstallSteps(next: InstallStepsState): void {
    view.steps = next;
  }

  function hasOwnedInstallTask(): boolean {
    return installTask.taskSeq !== null && installTask.taskInstance !== null && installTask.claimedBy !== null;
  }

  function updateTaskCenterButton(): void {
    view.owned = hasOwnedInstallTask();
  }
  function dismissUpgradeDialog(): void {
    if (view.minimize) minimizeUpgradeModal();
    else closeUpgradeModal();
  }

  function upgradeDialogKeydown(event: KeyboardEvent): void {
    const modal = document.getElementById("upgradeModal");
    const box = modal?.querySelector<HTMLElement>(".modal-box");
    if (!modal?.classList.contains("active") || !box) return;
    if (event.key === "Escape") {
      event.preventDefault();
      dismissUpgradeDialog();
      return;
    }
    if (event.key !== "Tab") return;
    const focusables = Array.from(box.querySelectorAll<HTMLElement>('button, input, [href], select, textarea, [tabindex]:not([tabindex="-1"])'))
      .filter(el => !("disabled" in el && el.disabled) && el.offsetParent !== null);
    const first = focusables[0];
    const last = focusables[focusables.length - 1];
    if (!first || !last) {
      event.preventDefault();
      box.focus();
    } else if (!box.contains(document.activeElement) || (event.shiftKey && document.activeElement === first)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function upgradeDialogBackdrop(event: MouseEvent): void {
    if (event.target === event.currentTarget && Date.now() - upgradeModalOpenedAt > 250) dismissUpgradeDialog();
  }

  function showUpgradeDialog(): void {
    const modal = document.getElementById("upgradeModal");
    const box = modal?.querySelector<HTMLElement>(".modal-box");
    if (!modal || !box || view.open) return;
    upgradePreviousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    upgradeModalOpenedAt = Date.now();
    box.setAttribute("tabindex", "-1");
    box.setAttribute("aria-describedby", "upSubTitle");
    view.open = true;
    document.documentElement.classList.add("upgrade-modal-open");

    updateTaskCenterButton();
    modal.addEventListener("click", upgradeDialogBackdrop);
    document.addEventListener("keydown", upgradeDialogKeydown, true);
    const focusDialog = () => {
      if (!modal.classList.contains("active") || box.contains(document.activeElement)) return;
      const minimize = document.getElementById("btnUpMinimize");
      const close = document.getElementById("btnUpClose");
      (minimize && minimize.style.display !== "none" ? minimize : close || box).focus();
    };
    void nextTick(focusDialog);
    requestAnimationFrame(focusDialog);
    modal.addEventListener("transitionend", focusDialog, { once: true });
    setTimeout(focusDialog, 300);
  }

  function hideUpgradeDialog(): void {
    const modal = document.getElementById("upgradeModal");
    if (!modal || !view.open) return;
    view.open = false;
    document.documentElement.classList.remove("upgrade-modal-open");

    updateTaskCenterButton();
    modal.removeEventListener("click", upgradeDialogBackdrop);
    document.removeEventListener("keydown", upgradeDialogKeydown, true);
    const trigger = upgradePreviousFocus;
    void nextTick(() => trigger?.focus());
    upgradePreviousFocus = null;
  }

  function showTaskDetails(): void {
    if (!hasOwnedInstallTask()) return;
    const modal = document.getElementById("upgradeModal");
    if (!modal || view.open) return;
    if (!upgradeTerminal && upgradePollTimer === null) {
      pollFailureStreak = 0;
      upgradeStartedAt = Date.now();
      view.title = "正在查看已归属的安装任务...";
      view.subtitle = "正在重新读取任务状态";
      view.spinner = true;
      view.icon = false;
      view.minimize = true;
      view.close = false;
      void pollUpgradeLogs(true);
      upgradePollTimer = setInterval(pollUpgradeLogs, 800);
    } else if (upgradeTerminal) {
      void pollUpgradeLogs(true);
    }
    showUpgradeDialog();
  }

  function openUpgradeModal(
    attach: boolean,
    attachSeq?: number | null,
    attachInstance?: string | null,
  ): number {
    ++upgradeRunId;
    upgradeTerminal = false;
    installTask = createInstallTask({ attach, attachSeq, attachInstance });
    if (hasOwnedInstallTask()) persistInstallTask(installTaskStorage(), installTask);
    else clearStoredInstallTask(installTaskStorage());
    updateTaskCenterButton();
    upgradeStartedAt = Date.now();
    view.title = installTask.attach
      ? "正在查看进行中的安装任务..."
      : `QwenPaw 正在${currentInstallAction()} v${selectedVersion()}...`;
    view.subtitle = installTask.attach
      ? "NAS 后端独立执行中,与本页面是否在线无关"
      : "正在执行 pip 下载与环境重构，期间服务将自动重启";
    view.spinner = true;
    view.icon = view.close = false;
    view.minimize = true;
    upgradeLogsRendered = false;
    setInstallSteps({ current: null, outcome: null });
    paintTaskLog([], "正在获取实时安装状态...");
    showUpgradeDialog();
    stopUpgradePolling();
    void pollUpgradeLogs();
    upgradePollTimer = setInterval(pollUpgradeLogs, 800);
    return upgradeRunId;
  }

  function begin(): number {
    pollFailureStreak = 0;
    return openUpgradeModal(false);
  }

  function accept(seq: unknown, instance: unknown): void {
    if (typeof seq !== "number") return;
    installTask = acceptInstallTask(installTask, seq, instance);
    persistInstallTask(installTaskStorage(), installTask);
    updateTaskCenterButton();
  }

  function closeUpgradeModal(): void {
    hideUpgradeDialog();
    stopUpgradePolling();
    if (!hasOwnedInstallTask()) {
      ++upgradeRunId;
      clearStoredInstallTask(installTaskStorage());
      installTask = createInstallTask({ attach: false });
    }
    updateTaskCenterButton();
    void fetchStatus();
  }

  function minimizeUpgradeModal(): void {
    hideUpgradeDialog();
    showToast("已转入后台运行，可在控制台查看进度");
    void fetchStatus();
  }

  async function attachRunningInstall(): Promise<void> {
    try {
      const result = await api.upgradeStatus();
      if (!result.ok) { showToast("无法读取安装状态: HTTP " + result.status); return; }
      const status = result.data;
      if (!status.upgrading) {
        showToast("当前没有进行中的安装任务");
        void fetchStatus();
        return;
      }
      pollFailureStreak = 0;
      openUpgradeModal(true, status.seq, status.instance);
    } catch (error) {
      showToast("无法连接后端: " + errorMessage(error));
    }
  }

  async function pollUpgradeLogs(immediate = false): Promise<void> {
    const now = Date.now();
    if (!immediate && now - upPollLastAt < 1200) return;
    if (document.hidden) return;
    upPollLastAt = now;
    const runId = upgradeRunId;
    const taskSeq = installTask.taskSeq;
    const taskInstance = installTask.taskInstance;

    if (!upLogsPollInFlight && taskSeq !== null && taskInstance !== null) {
      upLogsPollInFlight = true;
      void api.upgradeLogs(taskSeq, taskInstance).then(logsRes => {
        if (runId !== upgradeRunId || installTask.taskSeq !== taskSeq || installTask.taskInstance !== taskInstance || !logsRes.ok) return;
        const data = logsRes.data;
        if (!data.success || !Array.isArray(data.logs)) return;
        if (data.logs.length === 0) return;
        const terminal = document.getElementById("upTerminal");
        if (!terminal) return;
        paintTaskLog(data.logs, "正在等待 pip 安装输出...");
        void nextTick(() => { terminal.scrollTop = terminal.scrollHeight; });
        upgradeLogsRendered = true;
      }).catch(error => {
        console.warn("pollUpgradeLogs err:", error);
      }).finally(() => {
        upLogsPollInFlight = false;
      });
    }

    if (upStatusPollInFlight) return;
    upStatusPollInFlight = true;
    try {
      const statusRes = await api.upgradeStatus();
      if (runId !== upgradeRunId) return;
      if (!statusRes.ok) {
        recordUpgradeStatusFailure();
        return;
      }

      const data = statusRes.data;
      pollFailureStreak = 0;
      const previousTask = installTask;
      const observed = observeInstallStatus(installTask, data);
      installTask = observed.state;
      if (installTask.taskSeq !== previousTask.taskSeq || installTask.taskInstance !== previousTask.taskInstance) {
        persistInstallTask(installTaskStorage(), installTask);
      }
      if (observed.terminal) {
        if (!upgradeTerminal) void fetchStatus();
        upgradeTerminal = true;
        void pollUpgradeLogs(true);
        stopUpgradePolling();
        updateTaskCenterButton();
        clearStoredInstallTask(installTaskStorage());
      } else if (observed.owned && installTask.taskSeq !== null && installTask.taskInstance !== null) {
        updateTaskCenterButton();
      }

      const observedStep = observed.owned && !observed.terminal ? installStepOf(data.phase) : null;
      if (observedStep && observedStep !== view.steps.current) {
        setInstallSteps({ current: observedStep, outcome: null });
      }

      if (observed.owned && data.upgrading && !upgradeLogsRendered) {
        const phase = typeof data.phase === "string" ? data.phase : "";
        const progress = phase === "downloading" ? "正在下载依赖包..."
          : phase === "installing" ? "正在安装内核..."
            : phase === "starting" ? "正在启动并检查服务..."
              : "安装任务正在执行...";
        paintTaskLog([], progress);
      }

      if (observed.owned && !observed.terminal && Date.now() - upgradeStartedAt > UPGRADE_POLL_MAX_MS) {
        stopUpgradePolling();
        showUpgradePollingFailure(true);
        return;
      }

      if (!observed.owned && !data.upgrading && Date.now() - upgradeStartedAt > 10000) {
        stopUpgradePolling();
        const noReceipt = !installTask.attach && installTask.taskSeq === null;
        view.spinner = false;
        view.title = "无法确认安装任务结果";
        view.subtitle = noReceipt
            ? "触发请求可能已送达并完成；请关闭本窗口后在控制台查看当前版本与安装日志确认"
            : "后端任务与本次操作不匹配，请关闭本窗口后在控制台查看安装日志";
        view.close = true;
        const minimizeWasFocused = document.activeElement === document.getElementById("btnUpMinimize");
        view.minimize = false;
        if (minimizeWasFocused) void nextTick(() => document.getElementById("btnUpClose")?.focus());
        return;
      }

      if (observed.terminal) {
        stopUpgradePolling();
        setInstallSteps({ current: view.steps.current, outcome: observed.outcome });

        view.spinner = false;
        view.icon = true;
        view.close = true;
        const minimizeWasFocused = document.activeElement === document.getElementById("btnUpMinimize");
        view.minimize = false;
        if (minimizeWasFocused) void nextTick(() => document.getElementById("btnUpClose")?.focus());

        const doneAction = pendingInstallAction() || currentInstallAction();
        const titleAction = installTask.attach ? "安装任务" : doneAction;
        if (observed.outcome === "failed") {
          view.title = `${titleAction}遇到问题`;
          const detail = (typeof data.error === "string" && data.error.trim())
            || (typeof data.message === "string" && data.message.trim())
            || "";
          const recoveryError = typeof data.recovery_error === "string" ? data.recovery_error.trim() : "";
          const recoveryNote = data.recovery_required === true || recoveryError
            ? (recoveryError
              ? "环境恢复未完成：" + recoveryError
              : "环境恢复未完成，请查看日志并按提示处理")
            : "请查看日志确认环境恢复状态";
          view.subtitle = detail ? `${detail}；${recoveryNote}` : recoveryNote;
          view.success = false;
        } else {
          view.title = `${titleAction}已顺利完成！`;
          view.subtitle = "所选版本已就绪，QwenPaw 后台服务已恢复运行";
          view.success = true;
          if (!upgradeLogsRendered) paintTaskLog([], '安装任务已完成，未获取到日志输出。');

        }
        const upModal = document.getElementById("upgradeModal");
        if (upModal && !upModal.classList.contains("active")) {
          showToast(observed.outcome === "failed" ? `${titleAction}失败,点击控制台查看详情` : `${titleAction}已完成`);
        }
      }
    } catch (error) {
      if (runId !== upgradeRunId) return;
      console.error("pollUpgradeStatus err:", error);
      recordUpgradeStatusFailure();
    } finally {
      upStatusPollInFlight = false;
    }
  }

  function recordUpgradeStatusFailure(): void {
    pollFailureStreak += 1;
    const elapsed = Date.now() - upgradeStartedAt;
    const timedOut = elapsed > UPGRADE_POLL_MAX_MS;
    if (timedOut || pollFailureStreak >= UPGRADE_POLL_FAIL_MAX) {
      stopUpgradePolling();
      showUpgradePollingFailure(timedOut);
    }
  }

  function resume(): void {
    if (upgradePollTimer !== null) void pollUpgradeLogs(true);
  }

  function restore(): void {
    if (installTask.taskSeq !== null && installTask.taskInstance !== null) {
      openUpgradeModal(true, installTask.taskSeq, installTask.taskInstance);
    }
  }

  return {
    view, begin,
    accept,
    open: openUpgradeModal,
    isCurrent: (runId: number) => runId === upgradeRunId,
    close: closeUpgradeModal,
    minimize: minimizeUpgradeModal,
    attachRunning: attachRunningInstall,
    showTaskDetails,
    updateTaskCenterButton,
    resume,
    restore,
  };
}
