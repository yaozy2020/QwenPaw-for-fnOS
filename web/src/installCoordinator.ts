import { ApiClientError, type ApiClient } from "./api";
import type { createInstallProgressPanel } from "./installProgressPanel";
import type { ModalOptionsWithoutInput } from "./modal";
import type { createVersionPanel } from "./versionPanel";

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
    showToast(reason ? "安装已暂停：" + reason : "控制台无法确认监督者状态，暂不可安装");
    return true;
  }

  async function checkOrUpgrade(): Promise<void> {
    if (!versionPanel.selectedVersion()) {
      showToast("请先获取版本列表并选择目标版本。");
      return;
    }
    if (blocked()) return;
    const requestedVersion = versionPanel.selectedVersion();
    const requestedMirror = currentMirror();
    const backup = versionPanel.view.backup;
    const compatibilityNote = (backup ? '先停机备份已有数据；备份或校验失败将中止安装。\n\n' : '') + (versionPanel.isRecommended() ? '' : '本安装包没有此版本的适配补丁。从飞牛桌面打开 QwenPaw Web 时，页面可能无法加载。\n\n');
    let ok = false;

    if (!versionPanel.currentRunningVersion()) {
      versionPanel.setPendingInstallAction("安装");
      ok = await showModal({
        title: "安装内核确认",
        message: `${compatibilityNote}将下载并安装 QwenPaw v${versionPanel.selectedVersion()}。确认开始安装？`,
        type: "primary",
        confirmText: "开始安装",
        cancelText: "取消"
      });
    } else if (versionPanel.isSameVersion(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("重新安装");
      ok = await showModal({
        title: "重新安装当前版本",
        message: `${compatibilityNote}将通过独立暂存环境重新安装 QwenPaw v${versionPanel.selectedVersion()}，运行中的服务会中断。安装失败时会尝试恢复运行环境，不会自动还原用户数据。确认继续？`,
        type: "warn",
        confirmText: "重新安装",
        cancelText: "取消"
      });
    } else if (versionPanel.isDowngradeOf(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("降级");
      ok = await showModal({
        title: "降级风险警告",
        message: `${compatibilityNote}将从 v${versionPanel.currentRunningVersion()} 降级到 v${versionPanel.selectedVersion()}。旧版本可能无法读取现有配置或会话数据，也可能无法启动。确认降级？`,
        type: "danger",
        confirmText: "确认降级",
        cancelText: "取消并返回"
      });
    } else {
      versionPanel.setPendingInstallAction("升级");
      ok = await showModal({
        title: "在线升级确认",
        message: `${compatibilityNote}即将安装 QwenPaw v${versionPanel.selectedVersion()} 并重新构建环境，运行中的服务会中断。确认继续？`,
        type: "warn",
        confirmText: "开始升级",
        cancelText: "取消"
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
        showToast(status.data.recovery_error || "上一次事务需要恢复，暂不可安装");
        return;
      }
      if (status.ok && status.data.upgrading) {
        showToast("已有安装任务正在进行中,已切换到进度查看");
        await progress.attachRunning();
        return;
      }
    } catch {}
    if (blocked()) return;

    if (!versionPanel.hasVersion(requestedVersion) || requestedMirror !== currentMirror()) {
      showToast("版本或下载源已变更，请重新确认安装");
      return;
    }
    const requestRunId = progress.begin();

    try {
      const result = await api.upgrade(requestedVersion, requestedMirror, backup);
      if (!progress.isCurrent(requestRunId)) return;
      const data = result.data;
      if (data.success === false) {
        if (data.running) {
          showToast(data.message || "已有安装任务进行中,已切换到进度查看");
          progress.open(true, data.seq, data.instance);
          return;
        }
        showToast("安装未开始: " + (data.message || "后端拒绝"));
        progress.close();
        return;
      }
      if (!result.ok || data.success !== true) {
        throw new Error(data.message || `安装受理失败（HTTP ${result.status}）`);
      }
      if (typeof data.seq === "number") {
        progress.accept(data.seq, data.instance);
      }
    } catch (error) {
      if (!progress.isCurrent(requestRunId)) return;
      if (!(error instanceof ApiClientError) || error.kind !== "network") {
        showToast("安装请求失败: " + errorMessage(error));
        progress.close();
        return;
      }
      progress.open(true);
    }
  }

  return { checkOrUpgrade };
}
