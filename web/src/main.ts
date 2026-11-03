import { renderStatusCard, renderLogLines, renderMirrorOptions, renderVersionCards, renderHistoryItems } from "./components";
import { formatUptime } from "./statusStore";

const API_BASE = (function() {
      const path = window.location.pathname;
      if (path.includes("index.cgi")) {
        return path.substring(0, path.indexOf("index.cgi")) + "api.cgi";
      }
      return "/cgi/ThirdParty/qwenpaw/api.cgi";
    })();

    // CSRF double-submit:index.cgi 签发 qwenpaw_csrf cookie(非 HttpOnly ——
    // JS 需读取同值回传,双提交设计所需),页面从 document.cookie 读取,
    // POST 回传 X-CSRF-Token 头,api.cgi 比对两者。
    // 与代理/NAT/Host 重写完全无关(fnOS 网关会把 Host 重写为 localhost,
    // 基于 Host/Origin 的同源校验在该形态下必然误杀 —— 2.0.0-13 实测)。
    function getCsrfToken(): string {
      return (document.cookie.match(/(?:^|;\s*)qwenpaw_csrf=([^;]+)/) || [])[1] || "";
    }
    const csrfHeaders = (): Record<string, string> => {
      const t = getCsrfToken();
      return t ? { "X-CSRF-Token": t } : {};
    };

    let refreshTimer = null;
    let upgradePollTimer = null;
    let upgradeStartedAt = 0;
    // 轮询总超时:任务真挂起(upgrading 恒 true / 后端失联)时给用户一条逃逸路径
    const UPGRADE_POLL_MAX_MS = 10 * 60 * 1000;
    // 连续失败阈值:api.cgi 502 会走 catch,累计到此值即切"连接中断"终态
    const UPGRADE_POLL_FAIL_MAX = 6; // 800ms 间隔 ×6 ≈ 5s
    let pollFailureStreak = 0;
    let upPollLastAt = 0;
    // 任务跟踪:不再靠"phase 是否跳变"猜结果(首拍已是终态会漏判),按 seq 归属
    let upgradeSeenPhase = null;          // 首拍 phase(变化检测)
    let upgradeFirstPollTerminal = false; // 首拍即终态(附加模式秒完成判据)
    let upgradeBaselineSeq = null;        // 触发前的 seq(null=未知,退回旧行为)
    let upgradeTaskSeq = null;            // 已确认归属本会话的任务 seq
    let upgradeBaselineFailed = false;    // 基线获取失败:终态判定退化为严格 seq 归属
    let upgradeAttachMode = false;        // 附加模式:查看已在后端运行的任务

    let rawVersionList = [];
    let userPickedVersion = false;
    let selectedVersion = "2.2.2.post1";
    let latestStableVer = "2.2.2.post1";
    let latestBetaVer = "2.3.0b2";

    
    // 语义化版本比对函数:分级权重 [major, minor, patch, stage, num]
    // 严格对齐 dev(0) < a(1) < b(2) < rc(3) < formal(4) < post(5),数字段按整数比对
    function compareVersion(v1: string, v2: string): number {
      function parse(v: string) {
        if (!v) return [0, 0, 0, 0, 0];
        const clean = v.replace(/^v/, "").trim();
        const m = clean.match(/^(\d+)\.(\d+)(?:\.(\d+))?(.*)$/);
        if (!m) return [0, 0, 0, 0, 0];
        const major = parseInt(m[1], 10);
        const minor = parseInt(m[2], 10);
        const patch = parseInt(m[3] || "0", 10);
        const extra = m[4] || "";
        let stage = 4; // 4 = 正式发版(无后缀)
        let num = 0;
        if (!extra) {
          stage = 4;
          num = 0;
        } else if (extra.includes(".post")) {
          stage = 5; // 5 = post 发版(高于正式版)
          num = parseInt(extra.replace(/\D/g, "") || "0", 10);
        } else {
          const c = extra.replace(/^[.-]/, "").toLowerCase();
          if (c.startsWith("dev")) stage = 0;
          else if (c.startsWith("a") || c.startsWith("alpha")) stage = 1;
          else if (c.startsWith("b") || c.startsWith("beta")) stage = 2;
          else if (c.startsWith("rc") || c.startsWith("c") || c.startsWith("pre")) stage = 3;
          num = parseInt(c.replace(/\D/g, "") || "0", 10);
        }
        return [major, minor, patch, stage, num];
      }
      const a = parse(v1);
      const b = parse(v2);
      for (let i = 0; i < 5; i++) {
        if (a[i] > b[i]) return 1;
        if (a[i] < b[i]) return -1;
      }
      return 0;
    }

    // 核心主题管理:默认跟随系统(prefers-color-scheme),仅用户点切换时才落盘。
    // 存储键 v2 —— 旧版曾把默认 dark 自动写入旧键,无法与用户显式选择区分。
    function systemTheme() {
      return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    }

    function applyTheme(t) {
      document.documentElement.setAttribute("data-theme", t);
      const sun = document.getElementById("themeIconSun");
      const moon = document.getElementById("themeIconMoon");
      const txt = document.getElementById("themeText");

      if (t === "light") {
        if (sun) sun.style.display = "inline-block";
        if (moon) moon.style.display = "none";
        if (txt) txt.innerText = "白天";
      } else {
        if (sun) sun.style.display = "none";
        if (moon) moon.style.display = "inline-block";
        if (txt) txt.innerText = "暗夜";
      }
    }

    function initTheme() {
      try { localStorage.removeItem("qwenpaw_theme"); } catch (e) {}
      // localStorage 访问被拒(禁 cookie/隐私模式/受限 iframe)会抛 SecurityError:
      // 顶层脚本中止 → window 导出全不执行,整页只剩静态壳。两处读取都必须兜。
      let saved = null;
      try { saved = localStorage.getItem("qwenpaw_theme_v2"); } catch (e) {}
      if (saved === "light" || saved === "dark") {
        applyTheme(saved);
      } else {
        applyTheme(systemTheme());
      }
    }

    function setTheme(t) {
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
    // 通用确认弹窗。传入 input 时渲染内联输入框,resolve 值为输入字符串或 null,
    // 这样需要填值的操作(如改端口)不必再用原生 prompt() —— 原生弹窗
    // 样式不可控、阻断渲染,且无法做校验反馈。
    // 无障碍:role=dialog + aria-modal 已在模板上,Esc 关闭、Tab 焦点陷阱、
    // 关闭后把焦点还给触发元素。
    function showModal({
      title = "提示确认", message = "", type = "primary",
      confirmText = "确认执行", cancelText = "取消",
      input = null,
    }) {
      return new Promise((resolve) => {
        const overlay = document.getElementById("customModal");
        const box = overlay?.querySelector(".modal-box");
        const modalTitle = document.getElementById("modalTitle");
        const modalMessage = document.getElementById("modalMessage");
        const modalBadge = document.getElementById("modalBadge");
        const confirmBtn = document.getElementById("modalConfirmBtn");
        const cancelBtn = document.getElementById("modalCancelBtn");
        const fieldWrap = document.getElementById("modalFieldWrap");
        const fieldLabel = document.getElementById("modalFieldLabel");
        const inputEl = document.getElementById("modalInput");
        if (!overlay || !box || !confirmBtn || !cancelBtn) return resolve(false);

        modalTitle.innerText = title;
        modalMessage.innerText = message;
        confirmBtn.innerText = confirmText;
        cancelBtn.innerText = cancelText;

        const hasInput = Boolean(input);
        if (fieldWrap && fieldLabel && inputEl) {
          fieldWrap.hidden = !hasInput;
          if (hasInput) {
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
        // 记录触发元素,关闭后归还焦点(键盘用户否则会被丢回页面顶部)
        const previousFocus = document.activeElement;

        function currentValue() {
          if (!hasInput || !inputEl) return null;
          const v = inputEl.value.trim();
          if (v === "") return null;
          if (input.validate && !input.validate(v)) return undefined; // 校验失败:不关闭
          return v;
        }

        function cleanup(res) {
          overlay.classList.remove("active");
          confirmBtn.removeEventListener("click", onConfirm);
          cancelBtn.removeEventListener("click", onCancel);
          overlay.removeEventListener("click", onBackdrop);
          document.removeEventListener("keydown", onKeydown, true);
          if (previousFocus && typeof previousFocus.focus === "function") {
            previousFocus.focus();
          }
          resolve(res);
        }

        function onConfirm() {
          if (!hasInput) return cleanup(true);
          const v = currentValue();
          if (v === undefined) return;       // 校验未通过,弹窗保持打开
          cleanup(v);
        }
        function onCancel() { cleanup(hasInput ? null : false); }
        function onBackdrop(e) {
          if (e.target === overlay && Date.now() - openTime > 250) onCancel();
        }
        function onKeydown(e) {
          if (e.key === "Escape") {
            e.preventDefault();
            onCancel();
            return;
          }
          if (e.key === "Enter" && hasInput) {
            // 输入框内回车等同确认,避免用户必须去点按钮
            e.preventDefault();
            onConfirm();
            return;
          }
          if (e.key !== "Tab") return;
          // 焦点陷阱:Tab 只在弹窗内的可聚焦元素之间循环
          const focusables = Array.from(
            box.querySelectorAll('button, input, [href], select, textarea, [tabindex]:not([tabindex="-1"])')
          ).filter((el) => !el.disabled && el.offsetParent !== null);
          if (!focusables.length) return;
          const first = focusables[0];
          const last = focusables[focusables.length - 1];
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
        // 有输入框时聚焦并全选,方便直接改写;否则聚焦确认按钮
        if (hasInput && inputEl) {
          inputEl.focus();
          inputEl.select?.();
        } else {
          confirmBtn.focus();
        }
      });
    }

    // 灵动岛风格置顶 Toast
    // 定时器必须单例:每次 showToast 都新建 setTimeout 会让先前那条的计时器
    // 继续跑,在后一条刚显示时把它移除 —— 连续提示(如"正在保存…"→"已保存")
    // 会出现闪一下就消失。改为始终重置同一个计时器。
    let toastTimer = null;
    function showToast(msg) {
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
    // 服务操作区按运行态切换。
    // 停止态必须给出"启动"入口 —— 否则用户只能点"重启"来恢复,
    // 虽然结果正确,但语义误导(用户以为在重启,实则是启动)。
    // degraded(后端不可达)时全部禁用:此时点任何操作都会失败,不如不给入口。
    function setServiceActions(running: boolean, installed: boolean, degraded: boolean): void {
      const start = document.getElementById("btnStartService");
      const restart = document.getElementById("btnRestartService");
      const stop = document.getElementById("btnStopService");
      if (!start || !restart || !stop) return;

      // 未安装内核时没有任何可执行的操作
      const usable = installed && !degraded;
      start.hidden = !(usable && !running);
      restart.hidden = !(usable && running);
      stop.hidden = !(usable && running);

      for (const btn of [start, restart, stop]) {
        btn.disabled = degraded;
        btn.title = degraded ? "控制台无法连接监督者，暂不可操作" : "";
      }
    }

    function setStatusUptime(text) {
      const el = document.getElementById("statusUptime");
      if (el) el.innerText = text;
    }

    // 服务状态监控 (全空值保护，稳健守护)
    // 轮询请求的序号守卫。fetch 的完成顺序不保证与发起顺序一致:
    // 后端短暂 502/超时后,旧请求可能晚于新请求返回,此时若直接渲染,
    // 就会把已经恢复的"运行中"翻回"不可达"(约 5s 后自愈,但用户看到闪跳)。
    // 只允许最新一次发起的请求写入 UI。
    let statusReqSeq = 0;
    async function fetchStatus() {
      if (document.hidden) return; // 后台标签页暂停轮询（对齐 DSH 实践）
      const mySeq = ++statusReqSeq;
      try {
        const res = await fetch(`${API_BASE}?action=status&_t=${Date.now()}`);
        if (!res.ok) throw new Error("HTTP " + res.status);
        const data = await res.json();
        if (mySeq !== statusReqSeq) return; // 已有更新的请求在途,丢弃过期结果

        const badge = document.getElementById("statusBadge");
        const valVersion = document.getElementById("valVersion");
        const valAuth = document.getElementById("valAuthStatus");

        const notInstalled = data.installed === false;
        // 每拍重渲染:服务状态/PID/运行时长/CPU/内存 由 StatusCardView 声明式渲染
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
        setServiceActions(Boolean(data.running), data.installed !== false, false);
        // 从降级态恢复时必须解禁,否则一次断连后按钮永久禁用
        const upBtnOk = document.getElementById("btnUpgradeAction");
        if (upBtnOk) {
          upBtnOk.disabled = false;
          upBtnOk.title = "";
        }

        if (notInstalled) {
          // 未安装提前返回,必须复位鉴权区颜色:
          // 否则上一次的绿色"已开启"会粘在"内核未安装"的状态上
          for (const id of ["valAuthStatus", "valAdminStatus"]) {
            const el = document.getElementById(id);
            if (el) el.style.color = "var(--text-muted)";
          }
          // 内核未安装（首次安装模式）：提示去版本管理安装
          if (badge) {
            badge.className = "badge badge-warning";
            badge.innerHTML = `<span class="pulse-dot"></span> 内核未安装`;
          }
          if (valVersion) valVersion.innerText = "未安装";
          currentRunningVersion = "";
          const tip = document.getElementById("kernelInstallTip");
          if (tip) tip.style.display = "block";
        } else {
          const tip = document.getElementById("kernelInstallTip");
          if (tip) tip.style.display = "none";
        }
        const uninstallBtn = document.getElementById("btnUninstallKernel");
        if (uninstallBtn) uninstallBtn.style.display = notInstalled ? "none" : "inline-block";
        updateUpgradeButtonText();

        // 后端安装任务进行中(重开控制台/换设备):恢复现场入口。
        // 弹窗已打开时不重复提示;点击进入附加模式而非再次创建任务
        const runTip = document.getElementById("installRunningTip");
        if (runTip) {
          const modalOpen = document.getElementById("upgradeModal")?.classList.contains("active");
          runTip.style.display = (data.install && data.install.running && !modalOpen) ? "block" : "none";
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

        if (valVersion && data.version) { valVersion.innerText = data.version; currentRunningVersion = data.version; updateUpgradeButtonText(); }
        const valAdmin = document.getElementById("valAdminStatus");
        const valDesc = document.getElementById("valAuthDesc");
        const statusType = data.authStatus || (data.authEnabled ? "protected" : "disabled");

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

        setStatusUptime((!notInstalled && data.running && data.startAt)
          ? "已运行 " + formatUptime(Math.floor(Date.now() / 1000) - data.startAt)
          : "");

        const portEl = document.getElementById("valPort");
        const activePort = data.port || "18088";
        if (portEl) portEl.innerText = activePort;

        const host = window.location.hostname || "127.0.0.1";
        const btnConsole = document.getElementById("btnWebConsole");
        if (btnConsole) btnConsole.href = `http://${host}:${activePort}`;

      } catch (e) {
        console.error("fetchStatus err:", e);
        // 无法连接后端(502/网络错误/会话过期)时必须把徽章切到未知态。
        // 否则徽章保留 index.html 里的初始红色/上一次的绿色,用户会以为服务在跑。
        const badge = document.getElementById("statusBadge");
        if (badge) {
          badge.className = "badge badge-warning";
          badge.innerHTML = `<span class="pulse-dot"></span> 状态不可达`;
        }
        const cardMount = document.getElementById("statusCardMount");
        if (cardMount) {
          renderStatusCard({
            // 不可达时安装状态"未知",不能写 false。
            // false 的语义是"确定未安装",会被下游当作事实派生逻辑,
            // 造成"状态不可达"与"内核未安装"同时出现的矛盾提示。
            installed: null,
            running: false,
            pid: "-",
            startAt: null,
            cpu: null,
            memKB: null,
            degraded: true, // 如实显示"状态不可达",不得伪装成"内核未安装"
          }, cardMount);
        }
        setStatusUptime("—"); // 保持 meta 位稳定,与卡内"状态不可达"呼应
        setServiceActions(false, false, true); // 不可达时不给操作入口

        // degraded 不变量必须覆盖全部入口,不能只收三个服务按钮 ——
        // 否则会出现"状态不可达"与"可发起安装/卸载"同屏的矛盾
        const upBtn = document.getElementById("btnUpgradeAction");
        if (upBtn) {
          upBtn.disabled = true;
          upBtn.title = "控制台无法连接监督者，暂不可操作";
        }
        const unBtn = document.getElementById("btnUninstallKernel");
        if (unBtn) unBtn.style.display = "none";
        for (const tipId of ["kernelInstallTip", "installRunningTip"]) {
          const tip = document.getElementById(tipId);
          if (tip) tip.style.display = "none";
        }
        // 鉴权区的颜色也是状态派生的,不可达时要复位,否则残留上一次的绿/红
        for (const id of ["valAuthStatus", "valAdminStatus"]) {
          const el = document.getElementById(id);
          if (el) el.style.color = "var(--text-muted)";
        }
        const downWarn = document.getElementById("downgradeWarnBar");
        if (downWarn) downWarn.style.display = "none";
        const valVersion = document.getElementById("valVersion");
        if (valVersion) valVersion.innerText = "无法获取";
        const valAuth = document.getElementById("valAuthStatus");
        if (valAuth) valAuth.innerText = "无法获取";
        const valDesc = document.getElementById("valAuthDesc");
        if (valDesc) {
          valDesc.innerText = "控制台无法连接监督者（可能服务重启中或会话已过期）。这不代表内核已停止，稍后会自动重试。";
        }
      }
    }

    
    // 修改 Web 服务端口
    async function changeServicePort() {
      const currentPort = document.getElementById("valPort")?.innerText || "18088";
      // 用站内弹窗替代原生 prompt():样式统一、可校验、不阻断渲染。
      // 校验在弹窗内完成,非法输入不关闭弹窗也不触发后端。
      const raw = await showModal({
        title: "修改服务端口",
        message: "请输入新的 QwenPaw Web 访问端口（范围 1024~65535）。保存后服务将自动重启以应用新端口。",
        type: "warn",
        confirmText: "确认修改并重启",
        input: {
          label: "新端口",
          value: currentPort,
          inputMode: "numeric",
          placeholder: "1024-65535",
          validate: (v) => /^[0-9]+$/.test(v) && Number(v) >= 1024 && Number(v) <= 65535,
        },
      });
      if (raw === null || raw === false) return;
      if (raw === currentPort) return;

      const newPort = parseInt(raw, 10);
      if (isNaN(newPort) || newPort < 1024 || newPort > 65535) {
        showToast("端口无效！必须为 1024~65535 之间的纯数字");
        return;
      }

      const ok = await showModal({
        title: "修改服务端口确认",
        message: `确定要将 QwenPaw Web 端口修改为 ${newPort} 吗？\n\n保存后服务将自动重启以应用新端口。`,
        type: "warn",
        confirmText: "确认修改并重启"
      });
      if (!ok) return;

      try {
        showToast(`正在保存新端口 ${newPort}...`);
        const res = await fetch(`${API_BASE}?action=change_port&port=${newPort}`, { method: "POST", headers: csrfHeaders() });
        const data = await res.json();
        if (data.success === false) {
          // 后端事务失败(端口冲突/回滚完成)必须如实呈现,不提示"已更改"
          showToast("修改端口失败: " + (data.message || "未知错误"));
          setTimeout(() => { fetchStatus(); fetchLogs(); }, 2500);
          return;
        }
        showToast(data.message || `端口已变更为 ${newPort}，正在重启...`);
        setTimeout(() => { fetchStatus(); fetchLogs(); }, 2500);
      } catch (e) {
        showToast("修改端口失败: " + e.message);
      }
    }

    // 鉴权管理
    async function resetAuth() {
      const ok = await showModal({
        title: "重置 QwenPaw 管理员密码",
        message: "这将清除 QwenPaw 对话端的管理员密码并使现有会话失效，下次进入对话端时需要重新设置。\n\n飞牛账号及其密码不受影响，用户配置与记忆也不会删除。确认继续？",
        type: "warn",
        confirmText: "确认重置"
      });
      if (!ok) return;

      try {
        showToast("正在请求重置...");
        const res = await fetch(`${API_BASE}?action=reset_auth`, { method: "POST", headers: csrfHeaders() });
        const data = await res.json();
        if (data.success === false) {
          showToast("重置失败: " + (data.message || "服务重启异常,请查看日志"));
        } else {
          showToast(data.message || "密码已重置成功！");
        }
        setTimeout(fetchStatus, 1200);
      } catch (e) {
        showToast("请求失败: " + e.message);
      }
    }

    async function uninstallKernel() {
      if (!currentRunningVersion) {
        showToast("当前未安装 QwenPaw 内核");
        return;
      }
      const ok = await showModal({
        title: "仅卸载 QwenPaw 内核",
        message: `将停止服务并删除 QwenPaw v${currentRunningVersion} 的运行环境与版本钉。\n\n• 用户配置、会话记忆与管理员认证均完整保留，之后可随时重新安装。\n• 注意：受飞牛 fnOS 桌面会话缓存限制，桌面图标隐藏可能需刷新浏览器页面生效。确认继续？`,
        type: "danger",
        confirmText: "仅卸载内核，保留数据"
      });
      if (!ok) return;
      try {
        showToast("正在卸载内核并保留用户数据...");
        const res = await fetch(`${API_BASE}?action=uninstall_kernel`, { method: "POST", headers: csrfHeaders() });
        const data = await res.json().catch(() => ({ success: false, message: "后端响应异常" }));
        showToast(data.message || (data.success ? "内核已卸载" : "卸载失败"));
        await fetchStatus();
        fetchLogs();
      } catch (e) {
        showToast("卸载请求失败: " + e.message);
      }
    }

    // 启停控制
    async function controlService(act) {
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
        const res = await fetch(`${API_BASE}?action=${act}`, { method: "POST", headers: csrfHeaders() });
        const data = await res.json();
        showToast(data.message || `${actionName}指令已成功执行`);
        setTimeout(() => { fetchStatus(); fetchLogs(); }, 2000);
      } catch (e) {
        showToast("操作失败: " + e.message);
      }
    }

    // 版本管理与在线检查
    // ── 下载源:自定义下拉(原生 select 在移动端弹系统白框,与主题割裂) ──
    // 值落盘服务端(VAR_DIR/pip-mirror)全设备共享;控制台每次升级显式携带
    const MIRRORS = [
      { value: "https://mirrors.aliyun.com/pypi/simple/", label: "阿里云镜像源（推荐 - 兼容公网/云机）" },
      { value: "https://pypi.tuna.tsinghua.edu.cn/simple", label: "清华大学镜像源" },
      { value: "https://pypi.mirrors.ustc.edu.cn/simple/", label: "中科大镜像源" },
      { value: "https://mirrors.cloud.tencent.com/pypi/simple", label: "腾讯云镜像源" },
      { value: "", label: "官方 PyPI（国内较慢）" },
    ];
    let mirrorValue = "";

    function renderMirrorUI() {
      const label = document.getElementById("mirrorLabel");
      if (label) label.innerText = (MIRRORS.find(m => m.value === mirrorValue) || MIRRORS[0]).label;
      const drop = document.getElementById("mirrorDropdown");
      if (drop) {
        renderMirrorOptions(MIRRORS, mirrorValue, pickMirror, drop);
      }
    }

    function toggleMirrorDropdown() {
      const drop = document.getElementById("mirrorDropdown");
      if (drop) drop.classList.toggle("show");
    }

    function pickMirror(v) {
      mirrorValue = v;
      renderMirrorUI();
      const drop = document.getElementById("mirrorDropdown");
      if (drop) drop.classList.remove("show");
      saveMirror();
    }

    async function loadMirror() {
      try {
        const res = await fetch(`${API_BASE}?action=get_mirror&_t=${Date.now()}`);
        const d = await res.json();
        if (d.success) {
          mirrorValue = d.mirror || "";
          renderMirrorUI();
        }
      } catch (e) { console.warn("loadMirror err:", e); }
    }

    async function saveMirror() {
      try {
        const res = await fetch(`${API_BASE}?action=set_mirror&mirror=${encodeURIComponent(mirrorValue)}`, { method: "POST", headers: csrfHeaders() });
        const d = await res.json();
        if (d.success) {
          showToast("下载源已保存，版本查询与内核安装将共用此源");
          await fetchVersionList();
        } else {
          showToast("保存失败: " + (d.message || ""));
        }
      } catch (e) {
        showToast("下载源保存失败");
      }
    }

    function currentMirror() {
      return mirrorValue;
    }

    // 点击触发器与面板之外自动收起(触发器内部子元素点击也算触发器)
    document.addEventListener("click", e => {
      const drop = document.getElementById("mirrorDropdown");
      const btn = document.getElementById("mirrorToggleBtn");
      if (drop && drop.classList.contains("show")
        && !drop.contains(e.target) && !(btn && btn.contains(e.target))) {
        drop.classList.remove("show");
      }
    });

    function showVersionSourceError(detail = "") {
      const alert = document.getElementById("versionSourceStatus");
      const message = "当前下载源不可用，无法获取版本列表。请更换下载源，或检查 NAS 网络、DNS 与代理设置后重试。";
      if (alert) alert.innerText = detail ? `${message}（${detail}）` : message;
      if (alert) alert.style.display = "block";
      showToast(message);
      return false;
    }

    async function fetchVersionList(): Promise<boolean> {
      try {
        const res = await fetch(`${API_BASE}?action=version_list&_t=${Date.now()}`);
        const data = await res.json().catch(() => null);
        if (res.ok && data && data.success && Array.isArray(data.versions)) {
          rawVersionList = data.versions;
          latestStableVer = data.latest_stable || "2.2.2.post1";
          const firstBeta = rawVersionList.find(v => v.is_beta);
          if (firstBeta) latestBetaVer = firstBeta.version;
          if (!userPickedVersion) {
            selectedVersion = latestStableVer;
          }
          const alert = document.getElementById("versionSourceStatus");
          if (alert) alert.style.display = "none";
          refreshVersionCards();
          return true;
        }
        return showVersionSourceError(data && data.message ? data.message : `HTTP ${res.status}`);
      } catch (e) {
        console.warn("fetchVersionList err:", e);
        return showVersionSourceError((e as Error).message);
      }
    }

    async function checkOnlineVersions() {
      showToast("正在在线检查 PyPI 最新版本...");
      // 检查失败必须如实提示,不伪装"检查完成"(历史版本恒报成功)
      const ok = await fetchVersionList();
      if (ok) {
        showToast(`检查完成！最新稳定: v${latestStableVer}，最新测试: v${latestBetaVer}`);
      }
    }

    function refreshVersionCards() {
      const container = document.getElementById("versionCardsContainer");
      const historyDrop = document.getElementById("historyDropdown");
      if (!container) return;


      renderVersionCards(latestBetaVer, latestStableVer, selectedVersion, pickVersion, container);

      if (historyDrop) {
        const historyList = rawVersionList.filter(v => v.version !== latestBetaVer && v.version !== latestStableVer);
        renderHistoryItems(historyList.slice(0, 10), pickVersion, historyDrop);
      }

      updateUpgradeButtonText();
    }

    function pickVersion(ver) {
      userPickedVersion = true;
      selectedVersion = ver;
      refreshVersionCards();
      const drop = document.getElementById("historyDropdown");
      if (drop) drop.classList.remove("show");
    }

    function toggleHistoryDropdown() {
      const drop = document.getElementById("historyDropdown");
      if (drop) drop.classList.toggle("show");
    }

    let currentRunningVersion = "";
    // 操作发起时的动作快照("安装"/"重新安装"/"升级"/"降级")。
    // 终态标题必须用它,不能在终态重算 —— 安装成功后当前版本已变成所选版本,
    // 重算会把"安装"误判成"重新安装"。
    let pendingInstallAction = "";

    // 降级判定:优先读取后端权威 action (upgrade/downgrade/reinstall/install);
    // 列表外版本退回索引与比较
    function isDowngradeOf(current: string, selected: string): boolean {
      const item = rawVersionList.find(v => v.version === selected);
      if (item && item.action) {
        return item.action === "downgrade";
      }
      const ci = rawVersionList.findIndex(v => v.version === current);
      const si = rawVersionList.findIndex(v => v.version === selected);
      if (ci >= 0 && si >= 0) return si > ci;
      return compareVersion(current, selected) > 0;
    }

    function isSameVersion(current: string, selected: string): boolean {
      if (!current || !selected) return false;
      const item = rawVersionList.find(v => v.version === selected);
      if (item && item.action === "reinstall") return true;
      return current.replace(/^v/i, "").trim() === selected.replace(/^v/i, "").trim();
    }

    function currentInstallAction(): string {
      if (!currentRunningVersion) return "安装";
      const item = rawVersionList.find(v => v.version === selectedVersion);
      if (item && item.action) {
        if (item.action === "reinstall") return "重新安装";
        if (item.action === "downgrade") return "降级";
        if (item.action === "upgrade") return "升级";
        if (item.action === "install") return "安装";
      }
      if (isSameVersion(currentRunningVersion, selectedVersion)) return "重新安装";
      return isDowngradeOf(currentRunningVersion, selectedVersion) ? "降级" : "升级";
    }

    function updateUpgradeButtonText() {
      const btn = document.getElementById("btnUpgradeAction");
      const warnBar = document.getElementById("downgradeWarnBar");
      if (!btn) return;

      const activeSelected = selectedVersion || latestStableVer || "2.2.2.post1";
      // 内核未安装：按钮语义是「安装」而非「更新/降级」
      if (!currentRunningVersion) {
        btn.innerText = `安装内核 v${activeSelected}`;
        btn.className = "btn btn-primary btn-sm";
        if (warnBar) warnBar.style.display = "none";
        return;
      }

      if (isSameVersion(currentRunningVersion, selectedVersion)) {
        btn.innerText = `重新安装 v${selectedVersion}`;
        btn.className = "btn btn-primary btn-sm";
        if (warnBar) warnBar.style.display = "none";
        return;
      }

      const isDowngrade = isDowngradeOf(currentRunningVersion, selectedVersion);
      if (isDowngrade) {
        btn.innerText = `降级至 v${selectedVersion}`;
        btn.className = "btn btn-danger btn-sm";
        if (warnBar) warnBar.style.display = "block";
      } else {
        btn.innerText = `执行更新至 v${selectedVersion}`;
        btn.className = "btn btn-primary btn-sm";
        if (warnBar) warnBar.style.display = "none";
      }
    }

    // 升级执行与专属流式终端
    async function checkOrUpgrade() {
      let ok = false;

      if (!currentRunningVersion) {
        // 初装分支: 内核尚未安装,明确使用"安装"语义,绝不误提示为升级/降级
        pendingInstallAction = "安装";
        ok = await showModal({
          title: "安装内核确认",
          message: `即将为您下载并部署 QwenPaw v${selectedVersion} 核心运行环境，首次安装约需 1~3 分钟。确认开始？`,
          type: "primary",
          confirmText: "开始安装",
          cancelText: "取消"
        });
      } else if (isSameVersion(currentRunningVersion, selectedVersion)) {
        pendingInstallAction = "重新安装";
        ok = await showModal({
          title: "重新安装当前版本",
          message: `将通过独立暂存环境重新安装 QwenPaw v${selectedVersion}，期间服务会短暂重启。用户配置与记忆保持不变，失败时自动恢复当前环境。确认继续？`,
          type: "warn",
          confirmText: "重新安装",
          cancelText: "取消"
        });
      } else if (isDowngradeOf(currentRunningVersion, selectedVersion)) {
        pendingInstallAction = "降级";
        ok = await showModal({
          title: "降级风险警告",
          message: `你正准备将 QwenPaw 降级至旧版本 v${selectedVersion}（当前运行为 v${currentRunningVersion}）。\n\n【已知风险】跨版本反向降级可能导致新版配置文件、会话记忆结构不兼容或依赖库冲突，甚至引起服务启动异常！\n\n如无明确排障需求，建议保持现状或升级。是否确认强行降级？`,
          type: "danger",
          confirmText: "已知风险，强行降级",
          cancelText: "取消并返回"
        });
      } else {
        pendingInstallAction = "升级";
        ok = await showModal({
          title: "在线升级确认",
          message: `即将安装 QwenPaw v${selectedVersion} 并重新构建环境，期间服务将短暂重启。确认继续？`,
          type: "warn",
          confirmText: "开始升级",
          cancelText: "取消"
        });
      }
      if (!ok) {
        pendingInstallAction = "";
        return;
      }

      // 触发前取基线:seq + 是否已有任务在跑。已有任务直接附加查看,禁止重复创建
      let baseline = null;
      try {
        const sres = await fetch(`${API_BASE}?action=upgrade_status&_t=${Date.now()}`);
        if (sres.ok) {
          const s = await sres.json();
          if (s.upgrading) {
            showToast("已有安装任务正在进行中,已切换到进度查看");
            attachRunningInstall();
            return;
          }
          if (typeof s.seq === "number") baseline = s.seq;
        } else {
          upgradeBaselineFailed = true; // sres 不 ok:基线未知
        }
      } catch (e) { upgradeBaselineFailed = true; }

      pollFailureStreak = 0;
      openUpgradeModal(false, baseline);

      // 触发安装:fetch(keepalive) 为主 —— 能读到受理响应(成功 seq / busy 提示),
      // 失败才退回 sendBeacon(浏览器保证尽力送达,但响应不可读)。
      // 后端接单与页面生死解耦:墓碑只中断界面,NAS 侧任务照常执行
      const url = `${API_BASE}?action=upgrade&version=${encodeURIComponent(selectedVersion)}&mirror=${encodeURIComponent(currentMirror())}`;
      let accepted = false;
      try {
        const res = await fetch(url, { method: "POST", keepalive: true, headers: csrfHeaders() });
        const d = await res.json().catch(() => null);
        if (d && d.success === false) {
          if (d.running) {
            // 触发瞬间别人已建任务:自动转入附加模式,不重复创建
            showToast(d.message || "已有安装任务进行中,已切换到进度查看");
            openUpgradeModal(true, d.seq);
            return;
          }
          showToast("安装未开始: " + (d.message || "后端拒绝"));
          closeUpgradeModal();
          return;
        }
        if (d && typeof d.seq === "number") {
          upgradeTaskSeq = d.seq; // 受理确认:后端已接单
        }
        accepted = true;
      } catch (e) {
        // fetch 失败(切后台/网络抖动):退回 sendBeacon,送达与否交给弹窗对 seq 的轮询
        try { accepted = Boolean(navigator.sendBeacon && navigator.sendBeacon(url + "&_csrf=" + encodeURIComponent(getCsrfToken()))); } catch (e2) { accepted = false; }
      }
      if (!accepted) {
        document.getElementById("upSubTitle").innerText = "触发请求可能未送达,正在确认...";
      }
    }

    // 重开控制台/换设备进入时恢复现场:后端任务独立运行,直接附加查看
    async function attachRunningInstall() {
      try {
        const res = await fetch(`${API_BASE}?action=upgrade_status&_t=${Date.now()}`);
        if (!res.ok) { showToast("无法读取安装状态: HTTP " + res.status); return; }
        const s = await res.json();
        if (!s.upgrading) {
          showToast("当前没有进行中的安装任务");
          fetchStatus();
          return;
        }
        pollFailureStreak = 0; // 上次会话遗留的失败连击不得连坐到附加会话
        openUpgradeModal(true, typeof s.seq === "number" ? s.seq : null);
      } catch (e) {
        showToast("无法连接后端: " + e.message);
      }
    }

    function openUpgradeModal(attach, baselineSeq) {
      // 重新打开时复位:minimize 可见、close 隐藏
      const minBtnReset = document.getElementById("btnUpMinimize");
      if (minBtnReset) minBtnReset.style.display = "block";
      const closeBtnReset = document.getElementById("btnUpClose");
      if (closeBtnReset) closeBtnReset.style.display = "none";
      const modal = document.getElementById("upgradeModal");
      if (!modal) return;
      // 跟踪状态统一在此初始化(先于首次轮询,避免竞态)
      upgradeAttachMode = Boolean(attach);
      upgradeBaselineSeq = (baselineSeq === undefined || baselineSeq === null) ? null : baselineSeq;
      upgradeTaskSeq = upgradeAttachMode && upgradeBaselineSeq !== null ? upgradeBaselineSeq : null;
      upgradeBaselineFailed = false;
      upgradeSeenPhase = null;
      upgradeFirstPollTerminal = false;
      upgradeStartedAt = Date.now();
      document.getElementById("upTitle").innerText = upgradeAttachMode
        ? "正在查看进行中的安装任务..."
        : `QwenPaw 正在${currentInstallAction()} v${selectedVersion}...`;
      document.getElementById("upSubTitle").innerText = upgradeAttachMode
        ? "NAS 后端独立执行中,与本页面是否在线无关"
        : "正在执行 pip 下载与环境重构，期间服务将自动重启";
      document.getElementById("upSpinner").style.display = "inline-block";
      document.getElementById("upDoneIcon").style.display = "none";
      document.getElementById("btnUpClose").style.display = "none";
      document.getElementById("upTerminal").innerHTML = "正在获取实时安装状态...\n";
      modal.classList.add("active");

      if (upgradePollTimer) clearInterval(upgradePollTimer);
      pollUpgradeLogs();
      upgradePollTimer = setInterval(pollUpgradeLogs, 800);
    }

    function closeUpgradeModal() {
      const modal = document.getElementById("upgradeModal");
      if (modal) modal.classList.remove("active");
      if (upgradePollTimer) {
        clearInterval(upgradePollTimer);
        upgradePollTimer = null;
      }
      upgradeAttachMode = false;
      upgradeTaskSeq = null;
      upgradeBaselineFailed = false;
      upgradeBaselineSeq = null;
      fetchStatus();
      fetchLogs();
    }

    // 后台运行:关闭弹窗但**不中断**任务,轮询继续,回到控制台可通过
    // installRunningTip 重新进入。与 closeUpgradeModal 的区别就在这里。
    function minimizeUpgradeModal() {
      const modal = document.getElementById("upgradeModal");
      if (modal) modal.classList.remove("active");
      showToast("已转入后台运行，可在控制台查看进度");
      fetchStatus();
    }

    async function pollUpgradeLogs() {
      // 节流守卫:上游慢时不叠加请求(800ms 轮询 × 慢响应曾是界面卡顿源之一)
      const now = Date.now();
      if (now - upPollLastAt < 1200) return;
      upPollLastAt = now;
      if (document.hidden) return; // 后台标签页暂停轮询
      try {
        const [logsRes, statusRes] = await Promise.all([
          fetch(`${API_BASE}?action=upgrade_logs&_t=${Date.now()}`),
          fetch(`${API_BASE}?action=upgrade_status&_t=${Date.now()}`)
        ]);

        if (logsRes.ok) {
          const ldata = await logsRes.json();
          if (ldata.success && Array.isArray(ldata.logs)) {
            const terminal = document.getElementById("upTerminal");
            if (terminal) {
              renderLogLines(ldata.logs, terminal, "正在等待 pip 安装输出...", true);
              terminal.scrollTop = terminal.scrollHeight;
            }
          }
        }

        pollFailureStreak = 0;

        if (statusRes.ok) {
          const sdata = await statusRes.json();
          const terminalPhase = sdata.phase === "done" || sdata.phase === "failed";
          if (upgradeSeenPhase === null) {
            upgradeSeenPhase = sdata.phase;
            upgradeFirstPollTerminal = terminalPhase;
          }
          const phaseChanged = sdata.phase !== upgradeSeenPhase;

          // 任务归属:受理响应给出的 seq,或基线之后的 seq 变化 = 本次任务已接单;
          // 附加模式直接跟随当前任务。首拍即终态 + seq 已归属 = 秒完成(修复漏判)
          if (!upgradeTaskSeq && typeof sdata.seq === "number") {
            if (upgradeAttachMode) {
              upgradeTaskSeq = sdata.seq;
            } else if (upgradeBaselineSeq !== null && sdata.seq !== upgradeBaselineSeq) {
              upgradeTaskSeq = sdata.seq;
            }
          }
          const ours = upgradeAttachMode || upgradeTaskSeq !== null || (upgradeBaselineSeq === null && !upgradeBaselineFailed);
          const isDone = ours && terminalPhase && (phaseChanged || upgradeFirstPollTerminal);
          const isFailed = ours && sdata.phase === "failed" && (phaseChanged || upgradeFirstPollTerminal);

          // 未送达守卫:非附加、任务未确认接单、后端无任务在跑 —— 指令没到达
          // (切后台被杀/网络断)。10s 后如实呈现,不再永久转圈
          const pending = !upgradeAttachMode && upgradeTaskSeq === null && !sdata.upgrading
            && (sdata.phase === "idle" || terminalPhase || sdata.success === false);
          if (pending && Date.now() - upgradeStartedAt > 10000) {
            clearInterval(upgradePollTimer);
            upgradePollTimer = null;
            document.getElementById("upSpinner").style.display = "none";
            document.getElementById("upTitle").innerText = "升级指令未送达";
            document.getElementById("upSubTitle").innerText = "请检查网络后关闭本窗口重试";
            document.getElementById("btnUpClose").style.display = "block";
            const minBtn = document.getElementById("btnUpMinimize");
            if (minBtn) minBtn.style.display = "none";
            return;
          }

          if (!sdata.upgrading && isDone) {
            clearInterval(upgradePollTimer);
            upgradePollTimer = null;

            document.getElementById("upSpinner").style.display = "none";
            document.getElementById("upDoneIcon").style.display = "inline-block";
            document.getElementById("btnUpClose").style.display = "block";
            const minBtn = document.getElementById("btnUpMinimize");
            if (minBtn) minBtn.style.display = "none";

            // 终态必须用"操作发起时"的动作,不能重算:
            // 安装成功后当前版本已变成所选版本,currentInstallAction() 会返回"重新安装",
            // 导致空环境首次安装也显示"重新安装已顺利完成"。
            const doneAction = pendingInstallAction || currentInstallAction();
            if (isFailed) {
              document.getElementById("upTitle").innerText = `${doneAction}遇到问题`;
              document.getElementById("upSubTitle").innerText = "请查看详细日志；若下载源不可用，请换源或检查 NAS 网络后重试。原内核环境已保留或恢复";
              document.getElementById("upDoneIcon").style.color = "var(--danger-text)";
            } else {
              document.getElementById("upTitle").innerText = `${doneAction}已顺利完成！`;
              document.getElementById("upSubTitle").innerText = "所选版本已就绪，QwenPaw 后台服务已恢复运行";
              document.getElementById("upDoneIcon").style.color = "var(--success)";
            }
          }
        }
      } catch (e) {
        console.error("pollUpgradeLogs err:", e);
        // 关键路径:supervisor 挂掉时 statusRes.ok 为 false 或直接抛错,
        // 上面那个"未送达守卫"位于 if (statusRes.ok) 内,根本不会执行。
        // 这里必须自己兜底,否则弹窗唯一出口 btnUpClose 永远 display:none,
        // 整个页面被永久锁死。
        pollFailureStreak += 1;
        const elapsed = Date.now() - upgradeStartedAt;
        const timedOut = elapsed > UPGRADE_POLL_MAX_MS;
        if (timedOut || pollFailureStreak >= UPGRADE_POLL_FAIL_MAX) {
          clearInterval(upgradePollTimer);
          upgradePollTimer = null;
          document.getElementById("upSpinner").style.display = "none";
          document.getElementById("upTitle").innerText =
            timedOut ? "任务仍未结束" : "与控制台连接中断";
          document.getElementById("upSubTitle").innerText = timedOut
            ? "安装任务仍在后台执行，可关闭本窗口；稍后在控制台查看进度或日志"
            : "无法连接监督者。任务可能仍在后台执行，可关闭本窗口稍后重试";
          document.getElementById("upDoneIcon").style.display = "inline-block";
          document.getElementById("upDoneIcon").style.color = "var(--danger-text)";
          document.getElementById("btnUpClose").style.display = "block";
        }
      }
    }

    // ── 运行日志分页 ──
    // 页号按阅读方向:第 1 页最旧,末页 = 最新 = 实时;翻离末页即进入回看模式,
    // 实时轮询暂停,翻回末页自动恢复。回看窗口上限 2000 行(监督者 /logs tail
    // 上限),到界提示;更早内容请直接查看服务器日志文件。
    const LOG_PAGE_SIZE = 100;
    const LOG_WINDOW_MAX = 2000;
    let logLines = [];
    let logFetchSeq = 0; // 日志拉取序号:乱序响应丢弃
    let logPageFromEnd = 0; // 距末页的页数(0 = 实时)

    function renderLogPage() {
      const terminal = document.getElementById("logTerminal");
      if (!terminal) return;
      const end = logLines.length - logPageFromEnd * LOG_PAGE_SIZE;
      const start = Math.max(0, end - LOG_PAGE_SIZE);
      const pageLines = logLines.slice(start, end);
      renderLogLines(pageLines, terminal, "（当前无日志输出）");

      const totalPages = Math.max(1, Math.ceil(logLines.length / LOG_PAGE_SIZE));
      const displayPage = totalPages - logPageFromEnd;
      const indicator = document.getElementById("logPageIndicator");
      if (indicator) indicator.innerText = logLines.length ? `第 ${displayPage} / ${totalPages} 页` : "暂无日志";
      const prev = document.getElementById("logPrevBtn");
      const next = document.getElementById("logNextBtn");
      if (prev) prev.disabled = displayPage <= 1;
      if (next) next.disabled = logPageFromEnd <= 0;
      const hint = document.getElementById("logWindowHint");
      if (hint) hint.style.display = (logLines.length >= LOG_WINDOW_MAX && displayPage <= 1) ? "inline" : "none";

      if (logPageFromEnd === 0) {
        const autoScroll = document.getElementById("autoScrollCheck");
        if (autoScroll && autoScroll.checked) terminal.scrollTop = terminal.scrollHeight;
      } else {
        terminal.scrollTop = 0;
      }
    }

    async function fetchLogs() {
      if (document.hidden) return;    // 后台标签页暂停轮询
      if (logPageFromEnd > 0) return; // 回看模式:暂停实时刷新
      try {
        const res = await fetch(`${API_BASE}?action=logs&lines=${LOG_PAGE_SIZE}&_t=${Date.now()}`);
        if (!res.ok) return;
        const data = await res.json();
        if (data.success && Array.isArray(data.logs)) {
          logLines = data.logs;
          renderLogPage();
        }
      } catch (e) {
        console.error("fetchLogs err:", e);
      }
    }

    async function loadLogPage(fromEnd) {
      const mySeq = ++logFetchSeq; // 乱序守卫:小响应后到不得覆盖新结果
      const target = Math.max(0, fromEnd);
      const needed = (target + 1) * LOG_PAGE_SIZE;
      if (needed > logLines.length) {
        try {
          const res = await fetch(`${API_BASE}?action=logs&lines=${Math.min(needed, LOG_WINDOW_MAX)}&_t=${Date.now()}`);
          if (!res.ok) return;
          const data = await res.json();
          if (data.success && Array.isArray(data.logs) && mySeq === logFetchSeq) logLines = data.logs;
        } catch (e) {
          console.error("loadLogPage err:", e);
          return;
        }
      }
      logPageFromEnd = target;
      renderLogPage();
    }

    function logGoPrev() { loadLogPage(logPageFromEnd + 1); }
    function logGoNext() { loadLogPage(logPageFromEnd - 1); }
    function logGoLatest() { loadLogPage(0); }

    async function clearTerminalLogs() {
      logLines = [];
      logPageFromEnd = 0;
      renderLogPage();
      try {
        await fetch(`${API_BASE}?action=clear_logs`, { method: "POST", headers: csrfHeaders() });
      } catch (e) {
        console.error("clear_logs error:", e);
      }
    }

    function toggleAutoRefresh(enable) {
      if (enable) {
        if (!refreshTimer) refreshTimer = setInterval(fetchLogs, 3500);
      } else {
        if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
      }
    }


    // 初始执行全套自愈
    initTheme();
    refreshVersionCards(); // 立即按默认推荐稳定版初始化界面,杜绝空白与幽灵版本
    fetchStatus();
    fetchLogs();
    fetchVersionList();
    loadMirror();
    refreshTimer = setInterval(fetchLogs, 3500);
    setInterval(fetchStatus, 5000);

    // 标签页切回前台时立即补一次:轮询在 hidden 期间是暂停的,
    // 若只等下一个节拍,用户看到的最多是 5s 前的旧状态(可能是"运行中"却已停)。
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) return;
      fetchStatus();
      fetchLogs();
    });

    // 内联 onclick/onchange 处理器需全局可见 —— esbuild IIFE 会封住顶层作用域
    Object.assign(window, {
      fetchStatus, fetchLogs, controlService, changeServicePort, resetAuth,
      checkOrUpgrade, checkOnlineVersions, pickVersion, toggleHistoryDropdown,
      saveMirror, toggleTheme, toggleAutoRefresh, clearTerminalLogs,
      closeUpgradeModal, minimizeUpgradeModal, logGoPrev, logGoNext, logGoLatest,
      toggleMirrorDropdown, pickMirror, attachRunningInstall,
      // 遗漏会直接报 "uninstallKernel is not defined"(内联 onclick 找不到该符号)
      uninstallKernel,
    });
