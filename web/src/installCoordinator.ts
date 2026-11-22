import { ApiClientError, type ApiClient } from "./api";
import type { createInstallProgressPanel } from "./installProgressPanel";
import type { ModalOptionsWithoutInput } from "./modal";
import type { createVersionPanel } from "./versionPanel";
import { localizeBackendMessage, t } from "./i18n";

interface InstallCoordinatorOptions {
  api: Pick<ApiClient, "upgradeStatus" | "upgrade">;
  progress: ReturnType<typeof createInstallProgressPanel>;
  versionPanel: ReturnType<typeof createVersionPanel>;
  currentMirror(): string;
  recoveryReason(): string;
  supervisorReachable(): boolean | null;
  showModal(options: ModalOptionsWithoutInput): Promise<boolean>;
  showToast(message: string): void;
  errorMessage(error: unknown): string;
}

export function createInstallCoordinator({
  api,
  progress,
  versionPanel,
  currentMirror,
  recoveryReason,
  supervisorReachable,
  showModal,
  showToast,
  errorMessage,
}: InstallCoordinatorOptions) {
  function blocked(): boolean {
    const reason = recoveryReason();
    if (!reason && supervisorReachable() === true) return false;
    versionPanel.setPendingInstallAction("");
    showToast(reason ? t("install.paused", { reason }) : t("install.unreachable"));
    return true;
  }

  async function checkOrUpgrade(): Promise<void> {
    if (!versionPanel.selectedVersion()) {
      showToast(t("install.pickFirst"));
      return;
    }
    if (blocked()) return;
    const requestedVersion = versionPanel.selectedVersion();
    const requestedMirror = currentMirror();
    const backup = versionPanel.view.backup;
    const compatibilityNote = (backup ? t("install.backupNote") : "") + (versionPanel.isRecommended() ? "" : t("install.unpatchedNote"));
    let ok = false;

    if (!versionPanel.currentRunningVersion()) {
      versionPanel.setPendingInstallAction("install");
      ok = await showModal({
        title: t("install.confirm.installTitle"),
        message: t("install.confirm.installMsg", { note: compatibilityNote, version: versionPanel.selectedVersion() }),
        type: "primary",
        confirmText: t("install.confirm.installBtn"),
        cancelText: t("common.cancel"),
      });
    } else if (versionPanel.isSameVersion(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("reinstall");
      ok = await showModal({
        title: t("install.confirm.reinstallTitle"),
        message: t("install.confirm.reinstallMsg", { note: compatibilityNote, version: versionPanel.selectedVersion() }),
        type: "warn",
        confirmText: t("install.confirm.reinstallBtn"),
        cancelText: t("common.cancel"),
      });
    } else if (versionPanel.isDowngradeOf(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("downgrade");
      ok = await showModal({
        title: t("install.confirm.downgradeTitle"),
        message: t("install.confirm.downgradeMsg", {
          note: compatibilityNote,
          current: versionPanel.currentRunningVersion(),
          version: versionPanel.selectedVersion(),
        }),
        type: "danger",
        confirmText: t("install.confirm.downgradeBtn"),
        cancelText: t("install.confirm.downgradeCancel"),
      });
    } else {
      versionPanel.setPendingInstallAction("upgrade");
      ok = await showModal({
        title: t("install.confirm.upgradeTitle"),
        message: t("install.confirm.upgradeMsg", { note: compatibilityNote, version: versionPanel.selectedVersion() }),
        type: "warn",
        confirmText: t("install.confirm.upgradeBtn"),
        cancelText: t("common.cancel"),
      });
    }
    if (!ok) {
      versionPanel.setPendingInstallAction("");
      return;
    }

    if (blocked()) return;
    try {
      const status = await api.upgradeStatus();
      if (status.ok && status.data.recovery_required === true) {
        versionPanel.setPendingInstallAction("");
        showToast(status.data.recovery_error || t("install.recoveryBlocked"));
        return;
      }
      if (status.ok && status.data.upgrading) {
        showToast(t("install.alreadyRunning"));
        await progress.attachRunning();
        return;
      }
    } catch {}
    if (blocked()) return;

    if (!versionPanel.hasVersion(requestedVersion) || requestedMirror !== currentMirror()) {
      showToast(t("install.changedRetry"));
      return;
    }
    const requestRunId = progress.begin();

    try {
      const result = await api.upgrade(requestedVersion, requestedMirror, backup);
      if (!progress.isCurrent(requestRunId)) return;
      const data = result.data;
      if (data.success === false) {
        const backendMsg = localizeBackendMessage(data.code, data.message);
        if (data.running) {
          showToast(backendMsg || t("install.alreadyRunningFallback"));
          progress.open(true, data.seq, data.instance);
          return;
        }
        showToast(t("install.notStarted", { reason: backendMsg || t("install.backendRejected") }));
        progress.close();
        return;
      }
      if (!result.ok || data.success !== true) {
        throw new Error(localizeBackendMessage(data.code, data.message) || t("install.acceptFailed", { status: result.status }));
      }
      if (typeof data.seq === "number") {
        progress.accept(data.seq, data.instance);
      }
    } catch (error) {
      if (!progress.isCurrent(requestRunId)) return;
      if (!(error instanceof ApiClientError) || error.kind !== "network") {
        showToast(t("install.requestFailed", { error: errorMessage(error) }));
        progress.close();
        return;
      }
      progress.open(true);
    }
  }

  return { checkOrUpgrade };
}
