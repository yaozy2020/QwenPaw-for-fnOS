import type { ApiClient } from "./api";
import { operationSucceeded } from "./apiResult";
import { renderMirrorOptions } from "./components";
import type { ShowModal } from "./modal";

const MIRRORS = [
  { value: "https://mirrors.aliyun.com/pypi/simple/", label: "阿里云镜像源（推荐 - 兼容公网/云机）" },
  { value: "https://pypi.tuna.tsinghua.edu.cn/simple", label: "清华大学镜像源" },
  { value: "https://pypi.mirrors.ustc.edu.cn/simple/", label: "中科大镜像源" },
  { value: "https://mirrors.cloud.tencent.com/pypi/simple", label: "腾讯云镜像源" },
  { value: "", label: "官方 PyPI（国内较慢）" },
];

interface SettingsPanelOptions {
  api: Pick<ApiClient,
    "changePort" | "webProxyPort" | "changeWebProxyPort" | "resetAuth" | "getMirror" | "setMirror">;
  showModal: ShowModal;
  showToast(message: string): void;
  errorMessage(error: unknown): string;
  supervisorReachable(): boolean | null;
  fetchStatus(): void;
  fetchLogs(): void;
  fetchVersionList(): Promise<boolean>;
}

export function createSettingsPanel({
  api, showModal, showToast, errorMessage, supervisorReachable, fetchStatus, fetchLogs, fetchVersionList,
}: SettingsPanelOptions) {
  let webProxyPublic: boolean | null = null;
  let webProxyPort: number | null = null;
  let webProxyReadSeq = 0;
  let mirrorValue = "";

  function setWebProxyScope(value: boolean | null): void {
    webProxyPublic = value;
    const available = value !== null && webProxyPort !== null;
    const toggle = document.getElementById("webProxyPublic") as HTMLInputElement | null;
    if (!toggle) return;
    toggle.indeterminate = !available;
    toggle.disabled = !available;
    if (value !== null) toggle.checked = value;
    toggle.title = available ? "" : "无法确认反代端口或访问范围，请重试";
  }

  async function changeServicePort(): Promise<void> {
    if (supervisorReachable() !== true) {
      showToast("服务状态尚未同步，请先刷新后再修改端口");
      fetchStatus();
      return;
    }
    const currentPort = document.getElementById("valPort")?.innerText || "18088";
    const raw = await showModal({
      title: "修改服务端口",
      message: "请输入新的 QwenPaw Web 访问端口（1024~65535）。保存后服务将自动重启以应用新端口。",
      type: "warn",
      confirmText: "确认修改并重启",
      input: {
        label: "新端口",
        value: currentPort,
        inputMode: "numeric",
        placeholder: "1024-65535",
        validate: (value) => /^[0-9]+$/.test(value) && Number(value) >= 1024 && Number(value) <= 65535,
      },
    });
    if (raw === null || raw === false || raw === currentPort) return;
    const newPort = Number.parseInt(raw, 10);
    if (!Number.isInteger(newPort) || newPort < 1024 || newPort > 65535) {
      showToast("端口无效！必须为 1024~65535 之间的纯数字");
      return;
    }
    const ok = await showModal({
      title: "修改服务端口确认",
      message: `确定要将 QwenPaw Web 端口修改为 ${newPort} 吗？\n\n保存后服务将自动重启以应用新端口。`,
      type: "warn",
      confirmText: "确认修改并重启",
    });
    if (!ok) return;
    try {
      showToast(`正在保存新端口 ${newPort}...`);
      const data = operationSucceeded(await api.changePort(newPort));
      if (data.success === false) {
        showToast(`修改端口失败: ${data.message || "未知错误"}`);
      } else {
        showToast(data.message || `端口已变更为 ${newPort}，正在重启...`);
      }
      setTimeout(() => { fetchStatus(); fetchLogs(); }, 2500);
    } catch (error) {
      showToast(`修改端口失败: ${errorMessage(error)}`);
    }
  }

  async function fetchWebProxyPort(): Promise<void> {
    const seq = ++webProxyReadSeq;
    try {
      const result = await api.webProxyPort();
      if (seq !== webProxyReadSeq) return;
      const { data } = result;
      if (!result.ok || !data) {
        webProxyPort = null;
        setWebProxyScope(null);
        return;
      }
      const validPort = Number.isInteger(data.port) && data.port > 0 && data.port <= 65535;
      webProxyPort = validPort ? data.port : null;
      const element = document.getElementById("valWebProxyPort");
      if (element && webProxyPort !== null) element.innerText = String(webProxyPort);
      setWebProxyScope(typeof data.public === "boolean" ? data.public : null);
    } catch (error) {
      if (seq === webProxyReadSeq) {
        webProxyPort = null;
        setWebProxyScope(null);
      }
      console.error("fetchWebProxyPort err:", error);
    }
  }

  async function changeWebProxyPort(): Promise<void> {
    if (webProxyPublic === null || webProxyPort === null) {
      showToast("无法确认反代端口或访问范围，请重试");
      void fetchWebProxyPort();
      return;
    }
    const currentPort = String(webProxyPort);
    const raw = await showModal({
      title: "修改 Web 反代端口",
      message: "输入 0 自动选择空闲端口，或输入 1024~65535 的指定端口。新端口绑定成功后才会切换，失败保留原端口。",
      type: "warn",
      confirmText: "保存并切换",
      input: {
        label: "反代端口",
        value: currentPort,
        inputMode: "numeric",
        placeholder: "0 表示自动分配",
        validate: (value) => /^[0-9]+$/.test(value) && Number(value) <= 65535,
      },
    });
    if (raw === null || raw === false) return;
    const newPort = Number(raw);
    if (!Number.isInteger(newPort) || newPort < 0 || newPort > 65535 || (newPort > 0 && newPort < 1024)) {
      showToast("端口无效！请输入 0 或 1024~65535");
      return;
    }
    const ok = await showModal({
      title: "切换 Web 反代端口",
      message: newPort === 0 ? "确定自动选择空闲端口并切换 Web 反代吗？" : `确定将 Web 反代端口切换为 ${newPort} 吗？`,
      type: "warn",
      confirmText: "确认切换",
    });
    if (!ok) return;
    const publicAccess = webProxyPublic;
    if (publicAccess === null) {
      showToast("无法确认反代访问范围，请重试");
      return;
    }
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      const data = operationSucceeded(await api.changeWebProxyPort(newPort, publicAccess));
      if (!Number.isInteger(data.port) || data.port <= 0 || data.port > 65535) {
        throw new Error("后端未返回有效的反代端口");
      }
      webProxyPort = data.port;
      const element = document.getElementById("valWebProxyPort");
      if (element) element.innerText = String(data.port);
      setWebProxyScope(publicAccess);
      showToast(`Web 反代端口已切换为 ${data.port}`);
    } catch (error) {
      showToast(`修改反代端口失败: ${errorMessage(error)}`);
      void fetchWebProxyPort();
    }
  }

  async function setWebProxyPublic(): Promise<void> {
    const toggle = document.getElementById("webProxyPublic") as HTMLInputElement | null;
    if (!toggle) return;
    const previous = webProxyPublic;
    const port = webProxyPort;
    if (previous === null || port === null) {
      setWebProxyScope(null);
      void fetchWebProxyPort();
      return;
    }
    const publicAccess = toggle.checked;
    if (publicAccess) {
      const ok = await showModal({
        title: "允许外部访问 Web 反代",
        message: "开启后，同一网络中的其他设备可通过该端口访问 QwenPaw 主 Web。请确认已设置 QwenPaw 管理员密码。",
        type: "warn",
        confirmText: "允许外部访问",
      });
      if (!ok) {
        setWebProxyScope(previous);
        return;
      }
    }
    if (webProxyPublic !== previous) return;
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      operationSucceeded(await api.changeWebProxyPort(port, publicAccess));
      setWebProxyScope(publicAccess);
      showToast(publicAccess ? "已允许外部访问 Web 反代" : "Web 反代已改为仅本机访问");
    } catch (error) {
      showToast(`切换访问范围失败: ${errorMessage(error)}`);
      void fetchWebProxyPort();
    }
  }

  async function resetAuth(): Promise<void> {
    const ok = await showModal({
      title: "重置 QwenPaw 管理员密码",
      message: "这将清除 QwenPaw 对话端的管理员密码并使现有会话失效，下次进入对话端时需要重新设置。\n\n飞牛账号及其密码不受影响，用户配置与记忆也不会删除。确认继续？",
      type: "warn",
      confirmText: "确认重置",
    });
    if (!ok) return;
    try {
      showToast("正在请求重置...");
      const data = operationSucceeded(await api.resetAuth());
      if (data.success === false) {
        showToast(`重置失败: ${data.message || "服务重启异常,请查看日志"}`);
      } else {
        showToast(data.message || "密码已重置成功！");
      }
      setTimeout(fetchStatus, 1200);
    } catch (error) {
      showToast(`请求失败: ${errorMessage(error)}`);
    }
  }

  function renderMirrorUI(): void {
    const label = document.getElementById("mirrorLabel");
    if (label) label.innerText = (MIRRORS.find(mirror => mirror.value === mirrorValue) || MIRRORS[0]).label;
    const drop = document.getElementById("mirrorDropdown");
    if (drop) renderMirrorOptions(MIRRORS, mirrorValue, pickMirror, drop);
  }

  function toggleMirrorDropdown(): void {
    const drop = document.getElementById("mirrorDropdown");
    if (drop) drop.classList.toggle("show");
  }

  function pickMirror(value: string): void {
    mirrorValue = value;
    renderMirrorUI();
    const drop = document.getElementById("mirrorDropdown");
    if (drop) drop.classList.remove("show");
    void saveMirror();
  }

  async function loadMirror(): Promise<void> {
    try {
      const { data } = await api.getMirror();
      if (data.success) {
        mirrorValue = data.mirror || "";
        renderMirrorUI();
      }
    } catch (error) {
      console.warn("loadMirror err:", error);
    }
  }

  async function saveMirror(): Promise<void> {
    try {
      operationSucceeded(await api.setMirror(mirrorValue));
      showToast("下载源已保存，版本查询与内核安装将共用此源");
      await fetchVersionList();
    } catch (error) {
      showToast(`下载源保存失败: ${errorMessage(error)}`);
    }
  }

  function closeMirrorDropdown(event: MouseEvent): void {
    const drop = document.getElementById("mirrorDropdown");
    const button = document.getElementById("mirrorToggleBtn");
    if (drop && drop.classList.contains("show") && event.target instanceof Node
      && !drop.contains(event.target) && !(button && button.contains(event.target))) {
      drop.classList.remove("show");
    }
  }

  document.addEventListener("click", closeMirrorDropdown);

  return {
    changeServicePort,
    fetchWebProxyPort,
    changeWebProxyPort,
    setWebProxyPublic,
    setScope: setWebProxyScope,
    resetAuth,
    loadMirror,
    saveMirror,
    toggleMirrorDropdown,
    pickMirror,
    currentMirror: () => mirrorValue,
    destroy: () => document.removeEventListener("click", closeMirrorDropdown),
  };
}
