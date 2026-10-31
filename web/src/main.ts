const API_BASE = (function() {
      const path = window.location.pathname;
      if (path.includes("index.cgi")) {
        return path.substring(0, path.indexOf("index.cgi")) + "api.cgi";
      }
      return "/cgi/ThirdParty/qwenpaw/api.cgi";
    })();

    let refreshTimer = null;
    let upgradePollTimer = null;

    let rawVersionList = [];
    let selectedVersion = "2.3.0b1";
    let latestStableVer = "2.2.2.post1";
    let latestBetaVer = "2.3.0b1";

    
    // 语义化版本比对函数
    function compareVersion(v1, v2) {
      function parse(v) {
        if (!v) return [0, 0, 0, 0];
        const clean = v.replace(/^v/, "").trim();
        const m = clean.match(/^(\d+)\.(\d+)(?:\.(\d+))?(.*)$/);
        if (!m) return [0, 0, 0, 0];
        const major = parseInt(m[1], 10);
        const minor = parseInt(m[2], 10);
        const patch = parseInt(m[3] || "0", 10);
        const extra = m[4] || "";
        let extraScore = 0;
        if (extra.includes(".post")) extraScore = 2;
        else if (/^[ab]|rc|dev|pre/.test(extra.replace(/^[.-]/, ""))) {
          const num = parseInt(extra.replace(/\D/g, "") || "0", 10);
          extraScore = -100 + num;
        }
        return [major, minor, patch, extraScore];
      }
      const a = parse(v1);
      const b = parse(v2);
      for (let i = 0; i < 4; i++) {
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
      const saved = localStorage.getItem("qwenpaw_theme_v2");
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
      const saved = localStorage.getItem("qwenpaw_theme_v2");
      if (saved !== "light" && saved !== "dark") {
        applyTheme(e.matches ? "dark" : "light");
      }
    });

    // 现代模态弹窗 (返回 Promise，置顶展示)
    function showModal({ title = "提示确认", message = "", type = "primary", confirmText = "确认执行", cancelText = "取消" }) {
      return new Promise((resolve) => {
        const overlay = document.getElementById("customModal");
        const modalTitle = document.getElementById("modalTitle");
        const modalMessage = document.getElementById("modalMessage");
        const modalBadge = document.getElementById("modalBadge");
        const confirmBtn = document.getElementById("modalConfirmBtn");
        const cancelBtn = document.getElementById("modalCancelBtn");

        modalTitle.innerText = title;
        modalMessage.innerText = message;
        confirmBtn.innerText = confirmText;
        cancelBtn.innerText = cancelText;

        modalBadge.className = "modal-icon-badge " + (type === "danger" ? "danger" : (type === "warn" ? "warn" : "primary"));
        confirmBtn.className = type === "danger" ? "btn btn-danger" : "btn btn-primary";

        const openTime = Date.now();
        function cleanup(res) {
          overlay.classList.remove("active");
          confirmBtn.removeEventListener("click", onConfirm);
          cancelBtn.removeEventListener("click", onCancel);
          overlay.removeEventListener("click", onBackdrop);
          resolve(res);
        }

        function onConfirm() { cleanup(true); }
        function onCancel() { cleanup(false); }
        function onBackdrop(e) {
          if (e.target === overlay && Date.now() - openTime > 250) cleanup(false);
        }

        confirmBtn.addEventListener("click", onConfirm);
        cancelBtn.addEventListener("click", onCancel);
        overlay.addEventListener("click", onBackdrop);

        overlay.classList.add("active");
      });
    }

    // 灵动岛风格置顶 Toast
    function showToast(msg) {
      const t = document.getElementById("toast");
      if (!t) return;
      t.innerText = msg;
      t.classList.add("show");
      setTimeout(() => t.classList.remove("show"), 2500);
    }

    function formatUptime(seconds) {
      if (!seconds || seconds <= 0) return "-";
      const days = Math.floor(seconds / 86400);
      const hours = Math.floor((seconds % 86400) / 3600);
      const minutes = Math.floor((seconds % 3600) / 60);
      const secs = Math.floor(seconds % 60);
      // 进程运行时长须显示到秒:刚重启的 59 秒内只显示"0分钟"会被误读为卡住
      if (days > 0) return `${days}天 ${hours}小时 ${minutes}分`;
      if (hours > 0) return `${hours}小时 ${minutes}分`;
      if (minutes > 0) return `${minutes}分 ${secs}秒`;
      return `${secs}秒`;
    }

    function formatMem(kb) {
      if (!kb || kb <= 0) return "-";
      if (kb >= 1024 * 1024) return (kb / 1024 / 1024).toFixed(2) + " GB";
      if (kb >= 1024) return (kb / 1024).toFixed(1) + " MB";
      return `${kb} KB`;
    }

    // 服务状态监控 (全空值保护，稳健守护)
    async function fetchStatus() {
      if (document.hidden) return; // 后台标签页暂停轮询（对齐 DSH 实践）
      try {
        const res = await fetch(`${API_BASE}?action=status&_t=${Date.now()}`);
        if (!res.ok) throw new Error("HTTP " + res.status);
        const data = await res.json();
        
        const badge = document.getElementById("statusBadge");
        const valStatus = document.getElementById("valStatus");
        const valPid = document.getElementById("valPid");
        const valUptime = document.getElementById("valUptime");
        const valVersion = document.getElementById("valVersion");
        const valAuth = document.getElementById("valAuthStatus");
        const valCpu = document.getElementById("valCpu");
        const valMem = document.getElementById("valMem");

        const notInstalled = data.installed === false;
        // CPU/内存默认 "-",运行中再覆写(口径:监督者 300ms 采样,占总 CPU 百分比)
        if (valCpu) valCpu.innerText = "-";
        if (valMem) valMem.innerText = "-";

        if (notInstalled) {
          // 内核未安装（首次安装模式）：提示去版本管理安装
          if (badge) {
            badge.className = "badge badge-warning";
            badge.innerHTML = `<span class="pulse-dot"></span> 内核未安装`;
          }
          if (valStatus) {
            valStatus.innerText = "内核未安装";
            valStatus.style.color = "var(--text-sub)";
          }
          if (valPid) valPid.innerText = "-";
          if (valUptime) valUptime.innerText = "-";
          if (valVersion) valVersion.innerText = "未安装";
          currentRunningVersion = "";
          const tip = document.getElementById("kernelInstallTip");
          if (tip) tip.style.display = "block";
        } else {
          const tip = document.getElementById("kernelInstallTip");
          if (tip) tip.style.display = "none";
        }

        if (!notInstalled && data.running) {
          if (badge) {
            badge.className = "badge badge-success";
            badge.innerHTML = `<span class="pulse-dot"></span> 运行中`;
          }
          if (valStatus) {
            valStatus.innerText = "运行中 (Active)";
            valStatus.style.color = "var(--success)";
          }
          if (valPid) valPid.innerText = data.pid || "-";
          
          if (valUptime) {
            if (data.startAt) {
              const nowSec = Math.floor(Date.now() / 1000);
              valUptime.innerText = formatUptime(nowSec - data.startAt);
            } else {
              valUptime.innerText = "已就绪";
            }
          }
          if (valCpu) valCpu.innerText = typeof data.cpu === "number" ? data.cpu.toFixed(1) + "%" : "-";
          if (valMem) valMem.innerText = formatMem(data.memKB);
        } else if (!notInstalled) {
          if (badge) {
            badge.className = "badge badge-danger";
            badge.innerHTML = `<span class="pulse-dot"></span> 已停止`;
          }
          if (valStatus) {
            valStatus.innerText = "已停止 (Stopped)";
            valStatus.style.color = "var(--danger)";
          }
          if (valPid) valPid.innerText = "-";
          if (valUptime) valUptime.innerText = "-";
        }

        if (valVersion && data.version) { valVersion.innerText = data.version; currentRunningVersion = data.version; updateUpgradeButtonText(); }
        if (valAuth) valAuth.innerText = data.authEnabled ? "已启用密码保护" : "免密模式";

        const portEl = document.getElementById("valPort");
        const activePort = data.port || "18088";
        if (portEl) portEl.innerText = activePort;

        const host = window.location.hostname || "127.0.0.1";
        const btnConsole = document.getElementById("btnWebConsole");
        if (btnConsole) btnConsole.href = `http://${host}:${activePort}`;

      } catch (e) {
        console.error("fetchStatus err:", e);
      }
    }

    
    // 修改 Web 服务端口
    async function changeServicePort() {
      const currentPort = document.getElementById("valPort")?.innerText || "18088";
      const input = prompt("请输入新的 QwenPaw Web 访问端口 (范围: 1024~65535):", currentPort);
      if (!input || input.trim() === currentPort) return;
      
      const newPort = parseInt(input.trim(), 10);
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
        const res = await fetch(`${API_BASE}?action=change_port&port=${newPort}`, { method: "POST" });
        const data = await res.json();
        showToast(data.message || `端口已变更为 ${newPort}，正在重启...`);
        setTimeout(() => { fetchStatus(); fetchLogs(); }, 2500);
      } catch (e) {
        showToast("修改端口失败: " + e.message);
      }
    }

    // 鉴权管理
    async function resetAuth() {
      const ok = await showModal({
        title: "重置管理密码",
        message: "确定要重置管理员密码吗？重置后此前会话将失效，下次打开 Web 端时将提示重新设定密码。",
        type: "warn",
        confirmText: "确认重置"
      });
      if (!ok) return;

      try {
        showToast("正在请求重置...");
        const res = await fetch(`${API_BASE}?action=reset_auth`, { method: "POST" });
        const data = await res.json();
        showToast(data.message || "密码已重置成功！");
        setTimeout(fetchStatus, 1200);
      } catch (e) {
        showToast("请求失败: " + e.message);
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
        const res = await fetch(`${API_BASE}?action=${act}`, { method: "POST" });
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
      { value: "", label: "官方 PyPI（国内较慢）" },
      { value: "https://pypi.tuna.tsinghua.edu.cn/simple", label: "清华大学镜像源" },
      { value: "https://mirrors.aliyun.com/pypi/simple/", label: "阿里云镜像源" },
      { value: "https://pypi.mirrors.ustc.edu.cn/simple/", label: "中科大镜像源" },
      { value: "https://mirrors.cloud.tencent.com/pypi/simple", label: "腾讯云镜像源" },
    ];
    let mirrorValue = "";

    function renderMirrorUI() {
      const label = document.getElementById("mirrorLabel");
      if (label) label.innerText = (MIRRORS.find(m => m.value === mirrorValue) || MIRRORS[0]).label;
      const drop = document.getElementById("mirrorDropdown");
      if (drop) {
        drop.innerHTML = MIRRORS.map(m => `
          <div class="history-item" style="${m.value === mirrorValue ? "color:var(--primary);" : ""}" onclick="pickMirror('${m.value}')">
            <span>${m.label}</span>
            ${m.value === mirrorValue ? '<span style="color:var(--primary);">✓</span>' : ""}
          </div>`).join("");
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
        const res = await fetch(`${API_BASE}?action=set_mirror&mirror=${encodeURIComponent(mirrorValue)}`, { method: "POST" });
        const d = await res.json();
        showToast(d.success ? "下载源已保存，安装/升级内核时生效" : ("保存失败: " + (d.message || "")));
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

    async function fetchVersionList() {
      try {
        const res = await fetch(`${API_BASE}?action=version_list&_t=${Date.now()}`);
        if (!res.ok) throw new Error("HTTP " + res.status);
        const data = await res.json();
        if (data.success && Array.isArray(data.versions)) {
          rawVersionList = data.versions;
          latestStableVer = data.latest_stable || "2.2.2.post1";
          const firstBeta = rawVersionList.find(v => v.is_beta);
          if (firstBeta) latestBetaVer = firstBeta.version;
          renderVersionCards();
        }
      } catch (e) {
        console.warn("fetchVersionList err:", e);
        renderVersionCards();
      }
    }

    async function checkOnlineVersions() {
      showToast("正在在线检查 PyPI 最新版本...");
      await fetchVersionList();
      showToast(`检查完成！最新稳定: v${latestStableVer}，最新测试: v${latestBetaVer}`);
    }

    function renderVersionCards() {
      const container = document.getElementById("versionCardsContainer");
      const historyDrop = document.getElementById("historyDropdown");
      if (!container) return;

      let isHistorySelected = selectedVersion !== latestBetaVer && selectedVersion !== latestStableVer;

      let cardsHtml = `
        <div class="ver-card-item ${selectedVersion === latestBetaVer ? 'active' : ''}" onclick="pickVersion('${latestBetaVer}')">
          <div class="ver-info-left">
            <span class="ver-number">v${latestBetaVer}</span>
            <span class="ver-tag-pill beta">最新测试 Beta</span>
          </div>
          <div class="custom-radio"></div>
        </div>
        <div class="ver-card-item ${selectedVersion === latestStableVer ? 'active' : ''}" onclick="pickVersion('${latestStableVer}')">
          <div class="ver-info-left">
            <span class="ver-number">v${latestStableVer}</span>
            <span class="ver-tag-pill stable">推荐稳定 Stable</span>
          </div>
          <div class="custom-radio"></div>
        </div>
      `;

      if (isHistorySelected) {
        cardsHtml = `
          <div class="ver-card-item active" onclick="pickVersion('${selectedVersion}')">
            <div class="ver-info-left">
              <span class="ver-number">v${selectedVersion}</span>
              <span class="ver-tag-pill history">自选历史版本</span>
            </div>
            <div class="custom-radio"></div>
          </div>
        ` + cardsHtml;
      }

      container.innerHTML = cardsHtml;

      if (historyDrop) {
        const historyList = rawVersionList.filter(v => v.version !== latestBetaVer && v.version !== latestStableVer);
        historyDrop.innerHTML = historyList.slice(0, 10).map(item => `
          <div class="history-item" onclick="pickVersion('${item.version}')">
            <span style="font-family:monospace; font-weight:600;">v${item.version}</span>
            <span class="ver-tag-pill ${item.is_beta ? 'beta' : 'stable'}" style="font-size:10px;">${item.is_beta ? 'Beta' : '稳定版'}</span>
          </div>
        `).join("");
      }

      updateUpgradeButtonText();
    }

    function pickVersion(ver) {
      selectedVersion = ver;
      renderVersionCards();
      const drop = document.getElementById("historyDropdown");
      if (drop) drop.classList.remove("show");
    }

    function toggleHistoryDropdown() {
      const drop = document.getElementById("historyDropdown");
      if (drop) drop.classList.toggle("show");
    }

    let currentRunningVersion = "2.3.0b1";

    function updateUpgradeButtonText() {
      const btn = document.getElementById("btnUpgradeAction");
      const warnBar = document.getElementById("downgradeWarnBar");
      if (!btn) return;

      // 内核未安装：按钮语义是「安装」而非「更新/降级」
      if (!currentRunningVersion) {
        btn.innerText = `安装内核 v${selectedVersion}`;
        btn.className = "btn btn-primary btn-sm";
        if (warnBar) warnBar.style.display = "none";
        return;
      }

      const isDowngrade = compareVersion(currentRunningVersion, selectedVersion) > 0;
      if (isDowngrade) {
        btn.innerText = `⚠️ 降级至 v${selectedVersion}`;
        btn.className = "btn btn-danger btn-sm";
        if (warnBar) warnBar.style.display = "block";
      } else {
        btn.innerText = `执行更新至 v${selectedVersion}`;
        btn.className = "btn btn-primary btn-sm";
        if (warnBar) warnBar.style.display = "none";
      }
    }

    // 升级执行与专属流式终端 (第 0 毫秒置顶秒开)
    async function checkOrUpgrade() {
      const isDowngrade = compareVersion(currentRunningVersion, selectedVersion) > 0;
      let ok = false;

      if (isDowngrade) {
        ok = await showModal({
          title: "⚠️ 降级风险警告",
          message: `你正准备将 QwenPaw 降级至旧版本 v${selectedVersion}（当前运行为 v${currentRunningVersion}）。\n\n【已知风险】跨版本反向降级可能导致新版配置文件、会话记忆结构不兼容或依赖库冲突，甚至引起服务启动异常！\n\n如无明确排障需求，建议保持现状或升级。是否确认强行降级？`,
          type: "danger",
          confirmText: "已知风险，强行降级",
          cancelText: "取消并返回"
        });
      } else {
        ok = await showModal({
          title: "在线升级确认",
          message: `即将安装 QwenPaw v${selectedVersion} 并重新构建环境，期间服务将短暂重启。确认继续？`,
          type: "warn",
          confirmText: "开始升级",
          cancelText: "取消"
        });
      }
      if (!ok) return;

      openUpgradeModal();

      try {
        const res = await fetch(`${API_BASE}?action=upgrade&version=${encodeURIComponent(selectedVersion)}&mirror=${encodeURIComponent(currentMirror())}`, { method: "POST" });
        const data = await res.json();
        if (!data.success && !data.message.includes("正在进行中")) {
          showToast("升级提示: " + (data.message || "任务派发受阻"));
        }
      } catch (e) {
        console.warn("升级请求发送响应（服务可能正在重启）:", e);
      }
    }

    function openUpgradeModal() {
      const modal = document.getElementById("upgradeModal");
      if (!modal) return;
      document.getElementById("upTitle").innerText = `QwenPaw 正在更新至 v${selectedVersion}...`;
      document.getElementById("upSubTitle").innerText = "正在执行 pip 下载与环境重构，期间服务将自动重启";
      document.getElementById("upSpinner").style.display = "inline-block";
      document.getElementById("upDoneIcon").style.display = "none";
      document.getElementById("btnUpClose").style.display = "none";
      document.getElementById("upTerminal").innerHTML = "正在启动升级进程并拉取实时日志流...\n";
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
      fetchStatus();
      fetchLogs();
    }

    async function pollUpgradeLogs() {
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
            const html = ldata.logs.map(item => {
              const msg = (typeof item === "string" ? item : (item.message || JSON.stringify(item)));
              let cls = "log-info";
              if (msg.includes("ERROR") || msg.includes("Error") || msg.includes("失败")) cls = "log-error";
              else if (msg.includes("WARN") || msg.includes("Warning")) cls = "log-warn";
              else if (msg.includes("成功") || msg.includes("Successfully")) cls = "log-success";
              return `<div class="${cls}">${escapeHtml(msg)}</div>`;
            }).join("");
            
            terminal.innerHTML = html || "正在等待 pip 安装输出...\n";
            terminal.scrollTop = terminal.scrollHeight;
          }
        }

        if (statusRes.ok) {
          const sdata = await statusRes.json();
          const terminalText = document.getElementById("upTerminal").innerText;
          // v2 完成判据 = 桥转发的 install.phase(done/failed);v1 靠日志文本找
          // 中文标记在 v2 的 pip 流水里永不出现,弹窗曾永转(独立审计 major)
          const isDone = sdata.phase === "done" || sdata.phase === "failed"
            || terminalText.includes("升级成功") || terminalText.includes("升级失败");
          const isFailed = sdata.phase === "failed" || terminalText.includes("升级失败");

          if (!sdata.upgrading && isDone) {
            clearInterval(upgradePollTimer);
            upgradePollTimer = null;

            document.getElementById("upSpinner").style.display = "none";
            document.getElementById("upDoneIcon").style.display = "inline-block";
            document.getElementById("btnUpClose").style.display = "block";

            if (isFailed) {
              document.getElementById("upTitle").innerText = "升级遇到问题";
              document.getElementById("upSubTitle").innerText = "执行遇到异常退出，请查看上方详细日志";
              document.getElementById("upDoneIcon").style.color = "var(--danger)";
            } else {
              document.getElementById("upTitle").innerText = "升级已顺利完成！";
              document.getElementById("upSubTitle").innerText = "新版本已就绪，QwenPaw 后台服务已恢复运行";
              document.getElementById("upDoneIcon").style.color = "var(--success)";
            }
          }
        }
      } catch (e) {
        console.error("pollUpgradeLogs err:", e);
      }
    }

    // ── 运行日志分页 ──
    // 页号按阅读方向:第 1 页最旧,末页 = 最新 = 实时;翻离末页即进入回看模式,
    // 实时轮询暂停,翻回末页自动恢复。回看窗口上限 2000 行(监督者 /logs tail
    // 上限),到界提示;更早内容请直接查看服务器日志文件。
    const LOG_PAGE_SIZE = 100;
    const LOG_WINDOW_MAX = 2000;
    let logLines = [];
    let logPageFromEnd = 0; // 距末页的页数(0 = 实时)

    function renderLogPage() {
      const terminal = document.getElementById("logTerminal");
      if (!terminal) return;
      const end = logLines.length - logPageFromEnd * LOG_PAGE_SIZE;
      const start = Math.max(0, end - LOG_PAGE_SIZE);
      const pageLines = logLines.slice(start, end);
      const html = pageLines.map(item => {
        const msg = (typeof item === "string" ? item : (item.message || JSON.stringify(item)));
        let cls = "log-info";
        if (msg.includes("ERROR") || msg.includes("Error") || msg.includes("failed")) cls = "log-error";
        else if (msg.includes("WARN") || msg.includes("Warning")) cls = "log-warn";
        return `<div class="${cls}">${escapeHtml(msg)}</div>`;
      }).join("");
      terminal.innerHTML = html || "<span style='color:var(--text-muted);'>（当前无日志输出）</span>";

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
      const target = Math.max(0, fromEnd);
      const needed = (target + 1) * LOG_PAGE_SIZE;
      if (needed > logLines.length) {
        try {
          const res = await fetch(`${API_BASE}?action=logs&lines=${Math.min(needed, LOG_WINDOW_MAX)}&_t=${Date.now()}`);
          if (!res.ok) return;
          const data = await res.json();
          if (data.success && Array.isArray(data.logs)) logLines = data.logs;
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

    function clearTerminalLogs() {
      logLines = [];
      logPageFromEnd = 0;
      renderLogPage();
    }

    function toggleAutoRefresh(enable) {
      if (enable) {
        if (!refreshTimer) refreshTimer = setInterval(fetchLogs, 3500);
      } else {
        if (refreshTimer) { clearInterval(refreshTimer); refreshTimer = null; }
      }
    }

    function escapeHtml(str) {
      return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
    }

    // 初始执行全套自愈
    initTheme();
    fetchStatus();
    fetchLogs();
    fetchVersionList();
    loadMirror();
    refreshTimer = setInterval(fetchLogs, 3500);
    setInterval(fetchStatus, 5000);

    // 内联 onclick/onchange 处理器需全局可见 —— esbuild IIFE 会封住顶层作用域
    Object.assign(window, {
      fetchStatus, fetchLogs, controlService, changeServicePort, resetAuth,
      checkOrUpgrade, checkOnlineVersions, pickVersion, toggleHistoryDropdown,
      saveMirror, toggleTheme, toggleAutoRefresh, clearTerminalLogs,
      closeUpgradeModal, logGoPrev, logGoNext, logGoLatest,
      toggleMirrorDropdown, pickMirror,
    });
