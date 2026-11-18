import { createApiClient, type StatusResponse } from "./api";
import { createAuthorizedDirsPanel } from "./authorizedDirsPanel";
import { createInstallCoordinator } from "./installCoordinator";
import { createInstallProgressPanel } from "./installProgressPanel";
import { createLogPanel } from "./logPanel";
import { createPollingLifecycle } from "./pollingLifecycle";
import { createMobileNavigation } from "./navigation";
import { operationSucceeded } from "./apiResult";
import type { ModalOptions, ModalOptionsWithInput, ModalOptionsWithoutInput } from "./modal";
import { createSettingsPanel } from "./settingsPanel";
import { createStatusPanel } from "./statusPanel";
import { createVersionPanel } from "./versionPanel";

const api = createApiClient({ pathname: window.location.pathname, cookie: () => document.cookie });

    // 核心主题管理:默认跟随系统(prefers-color-scheme),仅用户点切换时才落盘。
    // 存储键 v2 —— 旧版曾把默认 dark 自动写入旧键,无法与用户显式选择区分。
    type Theme = "dark" | "light";

    function systemTheme(): Theme {
      return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    }

    function applyTheme(t: Theme): void {
      document.documentElement.setAttribute("data-theme", t);
      const sun = document.getElementById("themeIconSun");
      const moon = document.getElementById("themeIconMoon");
      const txt = document.getElementById("themeText");
      const toggle = document.getElementById("themeToggleBtn");
      const light = t === "light";

      if (sun) sun.style.display = light ? "inline-block" : "none";
      if (moon) moon.style.display = light ? "none" : "inline-block";
      if (txt) txt.innerText = light ? "白天" : "暗夜";
      // 按钮文字是当前模式,可访问名称同时说明点击后的动作
      const label = light ? "白天模式，切换到暗夜模式" : "暗夜模式，切换到白天模式";
      toggle?.setAttribute("aria-label", label);
      toggle?.setAttribute("title", label);
    }

    function initTheme(): void {
      try { localStorage.removeItem("qwenpaw_theme"); } catch (e) {}
      // localStorage 访问被拒(禁 cookie/隐私模式/受限 iframe)会抛 SecurityError:
      // 顶层脚本中止 → window 导出全不执行,整页只剩静态壳。两处读取都必须兜。
      let saved: string | null = null;
      try { saved = localStorage.getItem("qwenpaw_theme_v2"); } catch (e) {}
      if (saved === "light" || saved === "dark") {
        applyTheme(saved);
      } else {
        applyTheme(systemTheme());
      }
    }

    function setTheme(t: Theme): void {
      try { localStorage.setItem("qwenpaw_theme_v2", t); } catch (e) {}
      applyTheme(t);
    }

    function toggleTheme() {
      const curr = document.documentElement.getAttribute("data-theme") || systemTheme();
      setTheme(curr === "dark" ? "light" : "dark");
    }

    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", e => {
      let saved = null;
      try { saved = localStorage.getItem("qwenpaw_theme_v2"); } catch (e) {}
      if (saved !== "light" && saved !== "dark") {
        applyTheme(e.matches ? "dark" : "light");
      }
    });

    // 现代模态弹窗 (返回 Promise，置顶展示)
    // 通用确认弹窗。传入 input 时渲染内联输入框,resolve 值为输入字符串、null 或 false(模态框缺失),
    // 这样需要填值的操作(如改端口)不必再用原生 prompt() —— 原生弹窗
    // 样式不可控、阻断渲染,且无法做校验反馈。
    // 无障碍:role=dialog + aria-modal 已在模板上,Esc 关闭、Tab 焦点陷阱、
    // 关闭后把焦点还给触发元素。
    function showModal(options: ModalOptionsWithInput): Promise<string | null | false>;
    function showModal(options?: ModalOptionsWithoutInput): Promise<boolean>;
    function showModal({
      title = "提示确认", message = "", type = "primary",
      confirmText = "确认执行", cancelText = "取消",
      input = null,
    }: ModalOptions = {}): Promise<boolean | string | null> {
      return new Promise((resolve) => {
        const overlay = document.getElementById("customModal");
        const box = overlay?.querySelector<HTMLElement>(".modal-box");
        const modalTitle = document.getElementById("modalTitle");
        const modalMessage = document.getElementById("modalMessage");
        const modalBadge = document.getElementById("modalBadge");
        const confirmBtn = document.getElementById("modalConfirmBtn") as HTMLButtonElement | null;
        const cancelBtn = document.getElementById("modalCancelBtn") as HTMLButtonElement | null;
        const fieldWrap = document.getElementById("modalFieldWrap");
        const fieldLabel = document.getElementById("modalFieldLabel");
        const inputEl = document.getElementById("modalInput") as HTMLInputElement | null;
        if (!overlay || !box || !modalTitle || !modalMessage || !modalBadge || !confirmBtn || !cancelBtn) return resolve(false);
        const modalOverlay = overlay;
        const modalBox = box;
        const modalConfirmButton = confirmBtn;
        const modalCancelButton = cancelBtn;

        modalTitle.innerText = title;
        modalMessage.innerText = message;
        confirmBtn.innerText = confirmText;
        cancelBtn.innerText = cancelText;

        const hasInput = Boolean(input);
        if (fieldWrap && fieldLabel && inputEl) {
          fieldWrap.hidden = !hasInput;
          if (input) {
            fieldLabel.innerText = input.label || "输入";
            inputEl.value = input.value ?? "";
            inputEl.type = input.type || "text";
            inputEl.inputMode = input.inputMode || "text";
            inputEl.placeholder = input.placeholder || "";
            inputEl.setAttribute("pattern", input.pattern || "");
          } else {
            inputEl.value = "";
          }
        }

        modalBadge.className = "modal-icon-badge " + (type === "danger" ? "danger" : (type === "warn" ? "warn" : "primary"));
        confirmBtn.className = type === "danger" ? "btn btn-danger" : "btn btn-primary";

        const openTime = Date.now();
        const previousFocus = document.activeElement as (Element & { focus?: () => void }) | null;

        function currentValue(): string | null | undefined {
          if (!input || !inputEl) return null;
          const v = inputEl.value.trim();
          if (v === "") return null;
          if (input.validate && !input.validate(v)) return undefined;
          return v;
        }

        function cleanup(res: boolean | string | null): void {
          modalOverlay.classList.remove("active");
          modalConfirmButton.removeEventListener("click", onConfirm);
          modalCancelButton.removeEventListener("click", onCancel);
          modalOverlay.removeEventListener("click", onBackdrop);
          document.removeEventListener("keydown", onKeydown, true);
          if (previousFocus && typeof previousFocus.focus === "function") previousFocus.focus();
          resolve(res);
        }

        function onConfirm(): void {
          if (!input) return cleanup(true);
          const v = currentValue();
          if (v === undefined) return;
          cleanup(v);
        }
        function onCancel(): void { cleanup(input ? null : false); }
        function onBackdrop(e: MouseEvent): void {
          if (e.target === modalOverlay && Date.now() - openTime > 250) onCancel();
        }
        function onKeydown(e: KeyboardEvent): void {
          if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
            return;
          }
          if (e.key === "Enter" && hasInput) {
            const ae = document.activeElement;
            if (ae && (ae.id === "modalInput" || ae.id === "modalConfirmBtn")) {
              e.preventDefault();
              onConfirm();
            }
            return;
          }
          if (e.key !== "Tab") return;
          const focusables = Array.from(
            modalBox.querySelectorAll<HTMLElement>('button, input, [href], select, textarea, [tabindex]:not([tabindex="-1"])')
          ).filter((el) => !("disabled" in el && el.disabled) && el.offsetParent !== null);
          const first = focusables[0];
          const last = focusables[focusables.length - 1];
          if (!first || !last) return;
          if (e.shiftKey && document.activeElement === first) {
            e.preventDefault();
            last.focus();
          } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
          }
        }

        confirmBtn.addEventListener("click", onConfirm);
        cancelBtn.addEventListener("click", onCancel);
        overlay.addEventListener("click", onBackdrop);
        document.addEventListener("keydown", onKeydown, true);

        overlay.classList.add("active");
        if (hasInput && inputEl) {
          inputEl.focus();
          inputEl.select();
        } else {
          confirmBtn.focus();
        }
      });
    }

    // 灵动岛风格置顶 Toast
    // 定时器必须单例:每次 showToast 都新建 setTimeout 会让先前那条的计时器
    // 继续跑,在后一条刚显示时把它移除 —— 连续提示(如"正在保存…"→"已保存")
    // 会出现闪一下就消失。改为始终重置同一个计时器。
    let toastTimer: ReturnType<typeof setTimeout> | null = null;
    function showToast(msg: string) {
      const t = document.getElementById("toast");
      if (!t) return;
      t.innerText = msg;
      t.classList.add("show");
      if (toastTimer !== null) clearTimeout(toastTimer);
      toastTimer = setTimeout(() => {
        t.classList.remove("show");
        toastTimer = null;
      }, 2500);
    }

    // 卡片标题 meta 位(运行时长)的唯一写入点:正常/降级两分支共用,防口径漂移
    function showAbout(): void {
      const dialog = document.getElementById("aboutDialog") as HTMLDialogElement | null;
      if (!dialog || dialog.open) return;
      dialog.showModal();
    }

    function updateOverviewInstaller(): void {
      const version = versionPanel.selectedVersion() || versionPanel.latestStableVersion();
      const versionElement = document.getElementById("overviewInstallVersion");
      if (versionElement) versionElement.innerText = version ? `v${version}` : "推荐 Stable";
    }

    async function refreshDashboard(): Promise<void> {
      const button = document.getElementById("refreshDashboardBtn") as HTMLButtonElement | null;
      if (!button || button.disabled) return;
      button.disabled = true;
      button.setAttribute("aria-busy", "true");
      try {
        await Promise.all([fetchStatus(), fetchLogs()]);
      } finally {
        button.disabled = false;
        button.removeAttribute("aria-busy");
      }
    }

    function errorMessage(error: unknown): string {
      return error instanceof Error ? error.message : String(error);
    }


    const settingsPanel = createSettingsPanel({
      api, showModal, showToast, errorMessage,
      supervisorReachable,
      fetchStatus, fetchLogs: () => fetchLogs(), fetchVersionList,
    });
    const setWebProxyScope = settingsPanel.setScope;
    const changeServicePort = settingsPanel.changeServicePort;
    const fetchWebProxyPort = settingsPanel.fetchWebProxyPort;
    const changeWebProxyPort = settingsPanel.changeWebProxyPort;
    const setWebProxyPublic = settingsPanel.setWebProxyPublic;
    const resetAuth = settingsPanel.resetAuth;
    const loadMirror = settingsPanel.loadMirror;
    const saveMirror = settingsPanel.saveMirror;
    const toggleMirrorDropdown = settingsPanel.toggleMirrorDropdown;
    const pickMirror = settingsPanel.pickMirror;
    const currentMirror = settingsPanel.currentMirror;

    async function uninstallKernel() {
      if (!versionPanel.currentRunningVersion()) {
        showToast("当前未安装 QwenPaw 内核");
        return;
      }
      const ok = await showModal({
        title: "仅卸载 QwenPaw 内核",
        message: `将停止服务并删除 QwenPaw v${versionPanel.currentRunningVersion()} 的运行环境与版本钉。\n\n• 用户配置、会话记忆与管理员认证均完整保留，之后可随时重新安装。\n• 注意：受飞牛 fnOS 桌面会话缓存限制，桌面图标隐藏可能需刷新浏览器页面生效。确认继续？`,
        type: "danger",
        confirmText: "仅卸载内核，保留数据"
      });
      if (!ok) return;
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

    async function controlService(act: "start" | "stop" | "restart"): Promise<void> {
      const actionName = act === "restart" ? "重启" : (act === "stop" ? "停止" : "启动");
      const ok = await showModal({
        title: `${actionName}服务确认`,
        message: `确定要立即${actionName} QwenPaw 服务吗？${act === 'restart' ? '重启期间连接会短暂中断。' : ''}`,
        type: act === 'stop' ? 'danger' : 'primary',
        confirmText: `确认${actionName}`
      });
      if (!ok) return;

      try {
        showToast(`正在发送${actionName}指令...`);
        const data = operationSucceeded(await api.controlService(act));
        showToast(data.message || `${actionName}指令已成功执行`);
        setTimeout(() => { fetchStatus(); fetchLogs(); }, 2000);
      } catch (e) {
        showToast("操作失败: " + errorMessage(e));
      }
    }

    const versionPanel = createVersionPanel({
      api, showToast, supervisorReachable, updateOverviewInstaller,
    });
    function fetchVersionList(): Promise<boolean> { return versionPanel.fetchVersionList(); }
    const checkOnlineVersions = versionPanel.checkOnlineVersions;
    const refreshVersionCards = versionPanel.refreshVersionCards;
    const pickVersion = versionPanel.pickVersion;
    const toggleHistoryDropdown = versionPanel.toggleHistoryDropdown;
    const currentInstallAction = versionPanel.currentInstallAction;

    const statusPanel = createStatusPanel({ api, versionPanel, errorMessage, updateOverviewInstaller });
    function fetchStatus(snapshot?: StatusResponse): Promise<void> { return statusPanel.refresh(snapshot); }
    function supervisorReachable(): boolean | null { return statusPanel.reachable(); }

    const installProgress = createInstallProgressPanel({
      api, showToast, errorMessage, fetchStatus, fetchLogs: () => fetchLogs(),
      currentInstallAction,
      selectedVersion: versionPanel.selectedVersion,
      pendingInstallAction: versionPanel.pendingInstallAction,
    });
    const installCoordinator = createInstallCoordinator({
      api, progress: installProgress, versionPanel, currentMirror,
      supervisorReachable, showModal, showToast, errorMessage,
    });
    const checkOrUpgrade = installCoordinator.checkOrUpgrade;
    const attachRunningInstall = installProgress.attachRunning;
    const closeUpgradeModal = installProgress.close;
    const minimizeUpgradeModal = installProgress.minimize;
    const showTaskDetails = installProgress.showTaskDetails;
    const updateTaskCenterButton = installProgress.updateTaskCenterButton;


    const logPanel = createLogPanel({ api, showToast, confirm: showModal, errorMessage });
    const fetchLogs = logPanel.refresh;
    const clearTerminalLogs = logPanel.clear;
    const toggleAutoRefresh = logPanel.setAutoRefresh;


    const mobileNavigation = createMobileNavigation();

    let webEntryHistoryPushed = false;

    function isFnosApp(): boolean {
      return /FNAppType\//.test(navigator.userAgent || "");
    }

    function closeWebEntryOverlay(): void {
      const overlay = document.getElementById("webEntryOverlay");
      const frame = document.getElementById("webEntryFrame") as HTMLIFrameElement | null;
      if (!overlay || overlay.hidden) return;
      overlay.hidden = true;
      document.documentElement.classList.remove("web-entry-open");
      if (frame) frame.src = "about:blank";
      webEntryHistoryPushed = false;
    }

    function openWebPage(url: string): void {
      if (!isFnosApp()) {
        window.open(url, "_blank", "noopener");
        return;
      }
      const overlay = document.getElementById("webEntryOverlay");
      const frame = document.getElementById("webEntryFrame") as HTMLIFrameElement | null;
      if (!overlay || !frame) return;
      frame.src = url;
      overlay.hidden = false;
      document.documentElement.classList.add("web-entry-open");
      if (!webEntryHistoryPushed) {
        const state = history.state && typeof history.state === "object" ? history.state : {};
        history.pushState({ ...state, qwenpawWebEntry: true }, "", location.href);
        webEntryHistoryPushed = true;
      }
    }

    for (const entry of document.querySelectorAll<HTMLAnchorElement>("[data-web-console]")) {
      entry.addEventListener("click", event => {
        if (entry.getAttribute("href") !== "/app/qwenpaw/web/") return;
        event.preventDefault();
        openWebPage("/app/qwenpaw/web/");
      });
    }
    document.getElementById("openBackupBtn")?.addEventListener("click", () => {
      openWebPage("/app/qwenpaw/web/backups");
    });
    window.addEventListener("popstate", closeWebEntryOverlay);
    document.addEventListener("keydown", event => {
      if (event.key === "Escape") closeWebEntryOverlay();
    });

    function makeKeyboardClickable(element: HTMLElement | null): void {
      if (!element || element instanceof HTMLButtonElement || element instanceof HTMLAnchorElement) return;
      element.tabIndex = 0;
      element.setAttribute("role", "button");
      element.addEventListener("keydown", event => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        element.click();
      });
    }

    const historyToggle = document.querySelector<HTMLElement>(".history-versions-toggle");
    makeKeyboardClickable(historyToggle);
    historyToggle?.setAttribute("aria-controls", "historyDropdown");
    historyToggle?.setAttribute("aria-expanded", "false");
    const runningTip = document.getElementById("installRunningTip");
    makeKeyboardClickable(runningTip);
    runningTip?.setAttribute("aria-haspopup", "dialog");
    document.getElementById("taskCenterButton")?.addEventListener("click", showTaskDetails);
    updateTaskCenterButton();
    setWebProxyScope(null);

    let statusEvents: EventSource | null = null;

    function syncStatusEvents(): void {
      if (document.hidden) {
        statusEvents?.close();
        statusEvents = null;
        return;
      }
      if (statusEvents) return;
      const stream = new EventSource(`${api.base}/events`);
      statusEvents = stream;
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

    // 初始执行全套自愈
    initTheme();
    refreshVersionCards(); // 立即按默认推荐稳定版初始化界面,杜绝空白与幽灵版本
    fetchStatus();
    syncStatusEvents();

    fetchLogs();
    fetchVersionList();
    loadMirror();
    fetchWebProxyPort();
    void authorizedDirsPanel.restorePending().then(() => authorizedDirsPanel.refresh());
    logPanel.start();
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

    mobileNavigation.start();

    // 内联 onclick/onchange 处理器需全局可见 —— esbuild IIFE 会封住顶层作用域
    Object.assign(window, {
      fetchStatus, fetchLogs, refreshDashboard, controlService, changeServicePort, changeWebProxyPort, setWebProxyPublic, resetAuth,
      checkOrUpgrade, checkOnlineVersions, pickVersion, toggleHistoryDropdown,
      saveMirror, toggleTheme, showAbout, toggleAutoRefresh, clearTerminalLogs,
      closeUpgradeModal, minimizeUpgradeModal,
      toggleMirrorDropdown, pickMirror, attachRunningInstall,
      // 遗漏会直接报 "uninstallKernel is not defined"(内联 onclick 找不到该符号)
      uninstallKernel,
    });
