import { ApiClientError, type ApiClient } from "./api";
import type { createInstallProgressPanel } from "./installProgressPanel";
import type { ModalOptionsWithoutInput } from "./modal";
import type { createVersionPanel } from "./versionPanel";

interface InstallCoordinatorOptions {
  api: Pick<ApiClient, "upgradeStatus" | "upgrade">;
  progress: ReturnType<typeof createInstallProgressPanel>;
  versionPanel: ReturnType<typeof createVersionPanel>;
  currentMirror(): string;
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
  supervisorReachable,
  showModal,
  showToast,
  errorMessage,
}: InstallCoordinatorOptions) {
  async function checkOrUpgrade(): Promise<void> {
    if (!versionPanel.selectedVersion()) {
      showToast("可用版本尚未加载，请检查下载源后重试");
      return;
    }
    if (supervisorReachable() !== true) {
      showToast("控制台无法确认监督者状态，暂不可安装");
      return;
    }
    const requestedVersion = versionPanel.selectedVersion();
    const requestedMirror = currentMirror();
    let ok = false;

    if (!versionPanel.currentRunningVersion()) {
      versionPanel.setPendingInstallAction("安装");
      ok = await showModal({
        title: "安装内核确认",
        message: `即将为您下载并部署 QwenPaw v${versionPanel.selectedVersion()} 核心运行环境，首次安装约需 1~3 分钟。确认开始？`,
        type: "primary",
        confirmText: "开始安装",
        cancelText: "取消"
      });
    } else if (versionPanel.isSameVersion(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("重新安装");
      ok = await showModal({
        title: "重新安装当前版本",
        message: `将通过独立暂存环境重新安装 QwenPaw v${versionPanel.selectedVersion()}，期间服务会短暂重启。用户配置与记忆保持不变，失败时自动恢复当前环境。确认继续？`,
        type: "warn",
        confirmText: "重新安装",
        cancelText: "取消"
      });
    } else if (versionPanel.isDowngradeOf(versionPanel.currentRunningVersion(), versionPanel.selectedVersion())) {
      versionPanel.setPendingInstallAction("降级");
      ok = await showModal({
        title: "降级风险警告",
        message: `你正准备将 QwenPaw 降级至旧版本 v${versionPanel.selectedVersion()}（当前运行为 v${versionPanel.currentRunningVersion()}）。\n\n【已知风险】跨版本反向降级可能导致新版配置文件、会话记忆结构不兼容或依赖库冲突，甚至引起服务启动异常！\n\n如无明确排障需求，建议保持现状或升级。是否确认强行降级？`,
        type: "danger",
        confirmText: "已知风险，强行降级",
        cancelText: "取消并返回"
      });
    } else {
      versionPanel.setPendingInstallAction("升级");
      ok = await showModal({
        title: "在线升级确认",
        message: `即将安装 QwenPaw v${versionPanel.selectedVersion()} 并重新构建环境，期间服务将短暂重启。确认继续？`,
        type: "warn",
        confirmText: "开始升级",
        cancelText: "取消"
      });
    }
    if (!ok) {
      versionPanel.setPendingInstallAction("");
      return;
    }

    if (supervisorReachable() !== true) {
      versionPanel.setPendingInstallAction("");
      showToast("控制台无法确认监督者状态，暂不可安装");
      return;
    }
    try {
      const status = await api.upgradeStatus();
      if (status.ok && status.data.upgrading) {
        showToast("已有安装任务正在进行中,已切换到进度查看");
        await progress.attachRunning();
        return;
      }
    } catch {}
    if (supervisorReachable() !== true) {
      versionPanel.setPendingInstallAction("");
      showToast("控制台无法确认监督者状态，暂不可安装");
      return;
    }

    const requestRunId = progress.begin();

    try {
      const result = await api.upgrade(requestedVersion, requestedMirror);
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
