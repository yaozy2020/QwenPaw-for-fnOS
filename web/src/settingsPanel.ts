import type { ApiClient } from "./api";
import { operationSucceeded } from "./apiResult";
import { reactive } from "vue";
import type { ManagementState } from "./managementState";
import type { ShowModal } from "./modal";

const MIRRORS = [
  { value: "https://mirrors.aliyun.com/pypi/simple/", label: "阿里云镜像源（推荐 - 兼容公网/云机）" },
  { value: "https://pypi.tuna.tsinghua.edu.cn/simple", label: "清华大学镜像源" },
  { value: "https://pypi.mirrors.ustc.edu.cn/simple/", label: "中科大镜像源" },
  { value: "https://mirrors.cloud.tencent.com/pypi/simple", label: "腾讯云镜像源" },
  { value: "", label: "官方 PyPI（国内较慢）" },
];

interface SettingsPanelOptions {
  state: ManagementState;
  api: Pick<ApiClient,
    "changePort" | "webProxyPort" | "changeWebProxyPort" | "useNativeLogin" | "migrateEntry" | "resetAuth" | "getMirror" | "setMirror">;
  showModal: ShowModal;
  showToast(message: string): void;
  errorMessage(error: unknown): string;
  supervisorReachable(): boolean | null;
  fetchStatus(): void;
  fetchLogs(): void;
  fetchVersionList(sourceChanged?: boolean): Promise<boolean>;
}

export function createSettingsPanel({
  api, state, showModal, showToast, errorMessage, supervisorReachable, fetchStatus, fetchLogs, fetchVersionList,
}: SettingsPanelOptions) {
  let webProxyReadSeq = 0;
  const mirror = reactive({ value: '', ready: false, open: false, saving: false });
  let mirrorRead = 0;

  function setWebProxyScope(value: boolean | null): void {
    state.proxyPublic = value;
    if (value !== null) state.proxyDraft = value;
  }

  async function runAction(action: () => Promise<void>): Promise<void> {
    if (state.pending) return;
    state.pending = true;
    try { await action(); }
    finally { state.pending = false; state.saving = false; }
  }

  async function changeServicePort(): Promise<void> {
    if (supervisorReachable() !== true) {
      showToast("服务状态尚未同步，请先刷新后再修改端口");
      fetchStatus();
      return;
    }
    const currentPort = state.servicePort;
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
    if (!ok || supervisorReachable() !== true) return;
    state.saving = true;
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
        state.proxyPasswordConfigured = null;
        state.proxyPort = null;
        setWebProxyScope(null);
        return;
      }
      state.entryLogin = data.entryLogin === true;
      state.proxyPasswordConfigured = typeof data.passwordConfigured === "boolean" ? data.passwordConfigured : null;
      const validPort = Number.isInteger(data.port) && data.port > 0 && data.port <= 65535;
      state.proxyPort = validPort ? data.port : null;
      setWebProxyScope(typeof data.public === "boolean" ? data.public : null);
    } catch (error) {
      if (seq === webProxyReadSeq) {
        state.proxyPasswordConfigured = null;
        state.proxyPort = null;
        setWebProxyScope(null);
      }
      console.error("fetchWebProxyPort err:", error);
    }
  }

  async function changeWebProxyPort(): Promise<void> {
    if (state.proxyPublic === null || state.proxyPort === null) {
      showToast("无法确认反代端口或访问范围，请重试");
      await fetchWebProxyPort();
      return;
    }
    const currentPort = String(state.proxyPort);
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
    const publicAccess = state.proxyPublic;
    if (publicAccess === null) {
      showToast("无法确认反代访问范围，请重试");
      return;
    }
    state.saving = true;
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      const data = operationSucceeded(await api.changeWebProxyPort(newPort, publicAccess));
      if (!Number.isInteger(data.port) || data.port <= 0 || data.port > 65535) {
        throw new Error("后端未返回有效的反代端口");
      }
      state.proxyPort = data.port;
      setWebProxyScope(publicAccess);
      showToast(`Web 反代端口已切换为 ${data.port}`);
    } catch (error) {
      showToast(`修改反代端口失败: ${errorMessage(error)}`);
      await fetchWebProxyPort();
    }
  }

  async function setWebProxyPublic(publicAccess: boolean): Promise<void> {
    const previous = state.proxyPublic;
    const port = state.proxyPort;
    if (previous === null || port === null) {
      setWebProxyScope(null);
      await fetchWebProxyPort();
      return;
    }
    state.proxyDraft = publicAccess;
    if (publicAccess) {
      const ok = await showModal({
        title: "允许外部访问 Web 反代",
        message: "开启后，同一网络中的其他设备可通过该端口访问 QwenPaw 主 Web。请先在本机完成 QwenPaw 原账号注册，再开放端口。公网请通过 HTTPS 反代访问。",
        type: "warn",
        confirmText: "允许外部访问",
      });
      if (!ok) {
        state.proxyDraft = state.proxyPublic ?? false;
        return;
      }
    }
    if (state.proxyPublic !== previous || state.proxyPort !== port) return;
    state.saving = true;
    ++webProxyReadSeq;
    setWebProxyScope(null);
    try {
      operationSucceeded(await api.changeWebProxyPort(port, publicAccess));
      setWebProxyScope(publicAccess);
      showToast(publicAccess ? "已允许外部访问 Web 反代" : "Web 反代已改为仅本机访问");
    } catch (error) {
      showToast(`切换访问范围失败: ${errorMessage(error)}`);
      await fetchWebProxyPort();
    }
  }

  async function useNativeLogin(username: string, password: string): Promise<void> {
    if (state.proxyPasswordConfigured !== true) return;
    state.saving = true;
    ++webProxyReadSeq;
    try {
      operationSucceeded(await api.useNativeLogin(username, password));
      state.proxyPasswordConfigured = false;
      showToast('已切换为 QwenPaw 原生登录，旧反代会话已失效');
    } catch (error) {
      showToast(`未确认切换：${errorMessage(error)}`);
      await fetchWebProxyPort();
    }
  }

  async function migrateEntry(): Promise<void> {
    if (state.entryLogin !== false || !await showModal({title: '迁移到入口登录', message: '请先从飞牛桌面用原 QwenPaw 账号登录一次。确认后，飞牛入口使用平台管理员身份。独立反代已设密码时使用实例密码，未设时使用 QwenPaw 原生认证；本机服务端口保留原生登录。', confirmText: '确认迁移', type: 'warn'})) return;
    state.saving = true;
    try {
      const token = localStorage.getItem('qwenpaw_auth_token');
      if (!token) throw new Error('未找到旧会话，请在同一浏览器的飞牛入口登录原账号后重试');
      operationSucceeded(await api.migrateEntry(token));
      state.entryLogin = true;
      if (localStorage.getItem('qwenpaw_auth_token') === token) localStorage.removeItem('qwenpaw_auth_token');
      showToast('已迁移，重新打开对话即可使用入口登录');
    } catch (error) { showToast(`迁移未确认：${errorMessage(error)}`); }
    await fetchWebProxyPort();
  }

  async function resetAuth(password: string): Promise<void> {
    state.saving = true;
    try {
      operationSucceeded(await api.resetAuth(password));
      showToast('密码已重置，请用原用户名和新密码登录');
    } catch (error) {
      showToast(`密码重置未确认：${errorMessage(error)}`);
    }
  }

  async function pickMirror(value: string): Promise<void> {
    if (mirror.saving || !MIRRORS.some(item => item.value === value)) return;
    mirror.saving = true;
    mirror.open = false;
    ++mirrorRead;
    try {
      operationSucceeded(await api.setMirror(value));
      mirror.value = value;
      mirror.ready = true;
      await fetchVersionList(true);
      showToast('下载源已保存');
    } catch (error) {
      showToast(`下载源保存失败: ${errorMessage(error)}`);
      await loadMirror();
    } finally { mirror.saving = false; }
  }

  async function loadMirror(): Promise<void> {
    const request = ++mirrorRead;
    try {
      const {data, ok} = await api.getMirror();
      if (request !== mirrorRead) return;
      mirror.ready = ok && data.success === true;
      if (mirror.ready) mirror.value = data.mirror || '';
    } catch { if (request === mirrorRead) mirror.ready = false; }
  }

  function closeMirrorDropdown(event: MouseEvent): void {
    const drop = document.getElementById("mirrorDropdown");
    const button = document.getElementById("mirrorToggleBtn");
    if (drop && drop.classList.contains("show") && event.target instanceof Node
      && !drop.contains(event.target) && !(button && button.contains(event.target))) {
      mirror.open = false;
    }
  }

  document.addEventListener("click", closeMirrorDropdown);

  return {
    changeServicePort: () => runAction(changeServicePort),
    fetchWebProxyPort: () => state.saving ? Promise.resolve() : fetchWebProxyPort(),
    changeWebProxyPort: () => runAction(changeWebProxyPort),
    setWebProxyPublic: (value: boolean) => runAction(() => setWebProxyPublic(value)),
    migrateEntry: () => runAction(migrateEntry),
    useNativeLogin: (username: string, password: string) => runAction(() => useNativeLogin(username, password)),
    resetAuth: (password: string) => runAction(() => resetAuth(password)),
    loadMirror,
    mirror, mirrors: MIRRORS,
    toggleMirrorDropdown: () => { mirror.open = !mirror.open; if (mirror.open && !mirror.ready) void loadMirror(); },
    pickMirror,
    currentMirror: () => mirror.value,
    destroy: () => document.removeEventListener("click", closeMirrorDropdown),
  };
}
