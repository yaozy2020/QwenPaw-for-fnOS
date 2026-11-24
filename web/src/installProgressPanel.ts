import { nextTick, shallowReactive } from "vue";
import type { ApiClient, LogEntry } from "./api";
import type { InstallStep, InstallStepsState } from "./views/InstallSteps.vue";
import { currentLocale, t } from "./i18n";
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

const emptyText = () => "";

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
  const raw = shallowReactive({
    open: false, owned: false, spinner: true, icon: false, success: false, minimize: true, close: false,
    titleText: emptyText as () => string,
    subtitleText: emptyText as () => string,
    lines: [] as LogEntry[],
    emptyText: emptyText as () => string,
    steps: { current: null, outcome: null } as InstallStepsState,
  });
  const view = {
    get open() { return raw.open; },
    get owned() { return raw.owned; },
    get spinner() { return raw.spinner; },
    get icon() { return raw.icon; },
    get success() { return raw.success; },
    get minimize() { return raw.minimize; },
    get close() { return raw.close; },
    get title() { return raw.titleText(); },
    get subtitle() { return raw.subtitleText(); },
    get lines() { return raw.lines; },
    get empty() { return raw.emptyText(); },
    get steps() { return raw.steps; },
  };
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
  let upgradeLogsRendered = false;
  function paintTaskLog(lines: LogEntry[], fallback: () => string) {
    raw.lines = lines;
    raw.emptyText = fallback;
  }

  function stopUpgradePolling(): void {
    if (upgradePollTimer !== null) {
      clearInterval(upgradePollTimer);
      upgradePollTimer = null;
    }
  }

  function showUpgradePollingFailure(timedOut: boolean): void {
    raw.spinner = false;
    raw.titleText = () => (timedOut ? t("progress.timedOutTitle") : t("progress.disconnectedTitle"));
    raw.subtitleText = () => (timedOut ? t("progress.timedOutSub") : t("progress.disconnectedSub"));
    raw.icon = raw.close = true;
    raw.success = false;
  }
  function installStepOf(phase: unknown): InstallStep | null {
    return INSTALL_STEP_NAMES.find(step => step === phase) ?? null;
  }

  function setInstallSteps(next: InstallStepsState): void {
    raw.steps = next;
  }

  function hasOwnedInstallTask(): boolean {
    return installTask.taskSeq !== null && installTask.taskInstance !== null && installTask.claimedBy !== null;
  }

  function updateTaskCenterButton(): void {
    raw.owned = hasOwnedInstallTask();
  }
  function showUpgradeDialog(): void {
    if (raw.open) return;
    upgradePreviousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    raw.open = true;
    document.documentElement.classList.add("upgrade-modal-open");
    updateTaskCenterButton();
  }

  function hideUpgradeDialog(): void {
    if (!raw.open) return;
    raw.open = false;
    document.documentElement.classList.remove("upgrade-modal-open");
    updateTaskCenterButton();
    const trigger = upgradePreviousFocus;
    void nextTick(() => trigger?.focus());
    upgradePreviousFocus = null;
  }

  function showTaskDetails(): void {
    if (!hasOwnedInstallTask() || raw.open) return;
    if (!upgradeTerminal && upgradePollTimer === null) {
      pollFailureStreak = 0;
      upgradeStartedAt = Date.now();
      raw.titleText = () => t("progress.viewingOwnedTitle");
      raw.subtitleText = () => t("progress.viewingOwnedSub");
      raw.spinner = true;
      raw.icon = false;
      raw.minimize = true;
      raw.close = false;
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
    const isAttach = installTask.attach;
    const version = selectedVersion();
    raw.titleText = isAttach
      ? () => t("progress.viewingRunningTitle")
      : () => t("progress.runningTitle", { action: pendingInstallAction() || currentInstallAction(), version });
    raw.subtitleText = isAttach
      ? () => t("progress.viewingRunningSub")
      : () => t("progress.runningSub");
    raw.spinner = true;
    raw.icon = raw.close = false;
    raw.minimize = true;
    upgradeLogsRendered = false;
    setInstallSteps({ current: null, outcome: null });
    paintTaskLog([], () => t("progress.fetchingStatus"));
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
    showToast(t("progress.minimizedToast"));
    void fetchStatus();
  }

  async function attachRunningInstall(): Promise<void> {
    try {
      const result = await api.upgradeStatus();
      if (!result.ok) { showToast(t("progress.readStatusFailed", { status: result.status })); return; }
      const status = result.data;
      if (!status.upgrading) {
        showToast(t("progress.noRunningTask"));
        void fetchStatus();
        return;
      }
      pollFailureStreak = 0;
      openUpgradeModal(true, status.seq, status.instance);
    } catch (error) {
      showToast(t("progress.cannotConnect", { error: errorMessage(error) }));
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
        paintTaskLog(data.logs, () => t("progress.waitingPip"));
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
      if (observedStep && observedStep !== raw.steps.current) {
        setInstallSteps({ current: observedStep, outcome: null });
      }

      if (observed.owned && data.upgrading && !upgradeLogsRendered) {
        const phase = typeof data.phase === "string" ? data.phase : "";
        paintTaskLog([], () => (
          phase === "downloading" ? t("progress.phase.downloading")
            : phase === "installing" ? t("progress.phase.installing")
              : phase === "starting" ? t("progress.phase.starting")
                : t("progress.phase.running")
        ));
      }

      if (observed.owned && !observed.terminal && Date.now() - upgradeStartedAt > UPGRADE_POLL_MAX_MS) {
        stopUpgradePolling();
        showUpgradePollingFailure(true);
        return;
      }

      if (!observed.owned && !data.upgrading && Date.now() - upgradeStartedAt > 10000) {
        stopUpgradePolling();
        const noReceipt = !installTask.attach && installTask.taskSeq === null;
        raw.spinner = false;
        raw.titleText = () => t("progress.unconfirmedTitle");
        raw.subtitleText = () => (noReceipt ? t("progress.unconfirmedNoReceipt") : t("progress.unconfirmedMismatch"));
        raw.close = true;
        const minimizeWasFocused = document.activeElement === document.getElementById("btnUpMinimize");
        raw.minimize = false;
        if (minimizeWasFocused) void nextTick(() => document.getElementById("btnUpClose")?.focus());
        return;
      }

      if (observed.terminal) {
        stopUpgradePolling();
        setInstallSteps({ current: raw.steps.current, outcome: observed.outcome });

        raw.spinner = false;
        raw.icon = true;
        raw.close = true;
        const minimizeWasFocused = document.activeElement === document.getElementById("btnUpMinimize");
        raw.minimize = false;
        if (minimizeWasFocused) void nextTick(() => document.getElementById("btnUpClose")?.focus());

        const isAttach = installTask.attach;
        const resolveTitleAction = () => (isAttach ? t("progress.taskLabel") : (pendingInstallAction() || currentInstallAction()));
        if (observed.outcome === "failed") {
          raw.titleText = () => t("progress.failedTitle", { action: resolveTitleAction() });
          const detail = (typeof data.error === "string" && data.error.trim())
            || (typeof data.message === "string" && data.message.trim())
            || "";
          const recoveryError = typeof data.recovery_error === "string" ? data.recovery_error.trim() : "";
          const recoveryRequired = data.recovery_required === true;
          raw.subtitleText = () => {
            const recoveryNote = recoveryRequired || recoveryError
              ? (recoveryError
                ? t("progress.recoveryError", { error: recoveryError })
                : t("progress.recoveryIncomplete"))
              : t("progress.checkLogsNote");
            const sep = currentLocale.value === "en" ? "; " : "；";
            return detail ? `${detail}${sep}${recoveryNote}` : recoveryNote;
          };
          raw.success = false;
        } else {
          raw.titleText = () => t("progress.doneTitle", { action: resolveTitleAction() });
          raw.subtitleText = () => t("progress.doneSub");
          raw.success = true;
          if (!upgradeLogsRendered) paintTaskLog([], () => t("progress.doneNoLogs"));
        }
        if (!raw.open) {
          showToast(observed.outcome === "failed"
            ? t("progress.bgFailedToast", { action: resolveTitleAction() })
            : t("progress.bgDoneToast", { action: resolveTitleAction() }));
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
    destroy: () => {
      ++upgradeRunId;
      stopUpgradePolling();
      document.documentElement.classList.remove("upgrade-modal-open");
    },
  };
}
