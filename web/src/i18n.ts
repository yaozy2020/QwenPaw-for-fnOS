import { ref } from "vue";

export type Locale = "zh-CN" | "en";
export type LocaleMode = "system" | Locale;

export const LOCALE_STORAGE_KEY = "qwenpaw_locale_v1";

const zhCN = {
  // Page / Header
  "app.title": "QwenPaw 控制台",
  "nav.ariaLabel": "控制台分区",
  "nav.overview": "概览",
  "nav.configuration": "配置",
  "nav.authorization": "授权与备份",
  "nav.logs": "日志",

  // Header entry & badge
  "header.entry.connecting": "正在连接服务",
  "header.entry.connectingShort": "连接",
  "header.entry.check": "检查服务状态",
  "header.entry.checkShort": "检查",
  "header.entry.recovery": "查看恢复原因",
  "header.entry.recoveryShort": "恢复",
  "header.entry.install": "安装并开始使用",
  "header.entry.installShort": "安装",
  "header.entry.start": "先启动服务",
  "header.entry.startShort": "启动",
  "header.entry.chat": "进入 Web 对话",
  "header.entry.chatShort": "对话",

  "header.badge.checking": "检测中…",
  "header.badge.unreachable": "状态不可达",
  "header.badge.recovery": "需要恢复",
  "header.badge.uninstalled": "内核未安装",
  "header.badge.running": "运行中",
  "header.badge.stopped": "已停止",

  "header.about": "关于",
  "header.aboutTitle": "关于与常见问题",
  "header.taskCenter": "查看安装任务",
  "header.taskCenterAria": "查看安装任务详情",
  "header.taskCenterShort": "任务",

  // Theme
  "theme.system": "跟随系统",
  "theme.light": "白天",
  "theme.dark": "夜晚",
  "theme.aria.systemToDark": "白天模式，切换到夜晚模式",
  "theme.aria.systemToLight": "夜晚模式，切换到白天模式",
  "theme.aria.lightToDark": "白天模式，切换到夜晚模式",
  "theme.aria.lightToSystem": "白天模式，切换到跟随系统",
  "theme.aria.darkToLight": "夜晚模式，切换到白天模式",
  "theme.aria.darkToSystem": "夜晚模式，切换到跟随系统",

  // Locale toggle
  "locale.label": "中文",
  "locale.short": "中",
  "locale.switchAria": "当前语言：简体中文，切换到 English",

  // Overview
  "overview.uptimePrefix": "已运行 {uptime}",
  "overview.refreshTitle": "刷新服务状态",
  "overview.connecting": "正在连接服务",
  "overview.connectingDesc": "正在获取服务状态与资源使用…",
  "overview.metricsAria": "运行指标",
  "overview.cpu": "CPU 使用率",
  "overview.memory": "内存占用",
  "overview.pid": "进程 PID",
  "overview.uptime": "运行时间",
  "overview.detailsAria": "当前配置摘要",
  "overview.currentVersion": "当前版本",
  "overview.servicePort": "服务端口",
  "overview.authStatus": "登录验证",
  "overview.targetVersion": "目标版本",
  "overview.bundledPatch": "随包适配补丁",
  "overview.selectVersion": "请选择版本",
  "overview.startInstall": "开始安装",
  "overview.changeTargetNote": "可在「部署版本」更换目标版本。",
  "overview.runningInstall": "安装任务进行中 · 查看进度",
  "overview.startService": "启动服务",
  "overview.restartService": "重启服务",
  "overview.stopService": "停止服务",
  "overview.firstUse": "首次使用",
  "overview.guideStep1": "在「部署版本」点击「检查更新」，选择目标版本并安装",
  "overview.guideStep2": "等待下载依赖、安装内核与启动检查完成",
  "overview.guideStep3": "点击「进入 Web 对话」开始使用",

  // Status Card & Panel
  "status.unreachable": "状态不可达",
  "status.recovery": "需要恢复",
  "status.uninstalled": "内核未安装",
  "status.running": "运行中",
  "status.stopped": "已停止",
  "status.desc.unreachable": "暂时无法获取服务状态，请检查网络或重新登录飞牛后重试。{detail}",
  "status.desc.recovery": "暂勿重新安装或启停服务。请查看运行日志并保留现场。原因：{reason}",
  "status.desc.uninstalled": "先安装 QwenPaw 内核，即可开始使用 AI 对话。",
  "status.desc.running": "内核进程正在运行，可在下方查看 CPU、内存、进程 PID 与运行时间。",
  "status.desc.stopped": "内核已安装，启动服务后即可打开 Web 对话。",
  "status.viewLogs": "查看运行日志",
  "status.waitingSync": "等待首次同步",
  "status.lastSync": "最近同步 {time}",
  "status.syncFailed": "同步失败 · 自动重试中",
  "status.versionPending": "待确认",
  "status.versionUninstalled": "未安装",
  "status.unavailable": "无法获取",
  "status.invalidSnapshot": "无效的服务状态",
  "status.recoveryFallback": "上一次事务尚未安全收尾",

  // Deployment & Version Picker
  "deploy.title": "部署版本",
  "deploy.checkUpdates": "检查更新",
  "deploy.currentKernelVersion": "当前内核版本",
  "deploy.uninstalledTip": "尚未安装内核。选择目标版本后点击下方「安装内核」；列表为空时可先点「检查更新」。",
  "deploy.runningTip": "安装任务正在进行中，查看进度",
  "deploy.backupBeforeInstall": "安装前备份已有数据",
  "deploy.downgradeWarn": "目标版本低于当前内核，旧版本可能无法读取新版配置或记忆。",
  "deploy.unreachableTitle": "暂时无法连接管理服务，暂不可操作",
  "deploy.uninstallKernel": "仅卸载内核",
  "deploy.selectTarget": "请选择目标版本",
  "deploy.action.install": "安装内核",
  "deploy.action.downgrade": "降级至",
  "deploy.action.upgrade": "执行更新至",
  "deploy.action.reinstall": "重新安装",
  "deploy.actionShort.install": "安装",
  "deploy.actionShort.downgrade": "降级",
  "deploy.actionShort.upgrade": "升级",
  "deploy.actionShort.reinstall": "重新安装",
  "deploy.sort.allDates": "按发布时间从新到旧",
  "deploy.sort.partialDates": "按发布时间从新到旧；缺少时间的版本按版本号排在后面",
  "deploy.sort.noDates": "此源未提供发布时间，按版本号从高到低",

  "version.targetLabel": "目标版本",
  "version.loading": "正在读取版本…",
  "version.pickHint": "请选择目标版本",
  "version.emptyHint": "点「检查更新」获取版本列表",
  "version.beta": "测试版",
  "version.stable": "正式版",
  "version.currentInstalled": "当前安装",
  "version.sourceUnavailable": "当前下载源不可用，无法获取版本列表。请更换下载源，或检查 NAS 网络、DNS 与代理设置后重试。",
  "version.staleWarning": "当前显示上一次查询结果，不能用于安装。",
  "version.noRecommended": "当前下载源未列出本安装包带有适配补丁的版本。可从列表中选择其他版本，或更换下载源。",
  "version.emptyList": "当前下载源没有可选版本。请更换下载源或稍后重试。",
  "version.checkingToast": "正在在线检查 PyPI 最新版本...",
  "version.listedToast": "当前下载源列出 {count} 个版本。",

  // Mirror Picker
  "mirror.label": "下载源",
  "mirror.view": "查看下载源",
  "mirror.aliyun": "阿里云镜像源",
  "mirror.tsinghua": "清华大学镜像源",
  "mirror.ustc": "中科大镜像源",
  "mirror.tencent": "腾讯云镜像源",
  "mirror.pypi": "PyPI 官方源",
  "mirror.saved": "下载源已保存",
  "mirror.saveFailed": "下载源保存失败：{error}",

  // Configuration
  "config.title": "服务配置",
  "config.language": "界面语言",
  "config.languageSystem": "跟随系统",
  "config.languageZh": "简体中文",
  "config.languageEn": "English",
  "config.theme": "外观模式",
  "config.themeSystem": "跟随系统",
  "config.themeLight": "白天",
  "config.themeDark": "夜晚",
  "config.uiStyle": "界面风格",
  "config.uiStyleDefault": "默认",
  "config.uiStyleMiuix": "Miuix",
  "config.servicePort": "服务端口",
  "config.changePort": "修改端口",
  "config.portNotSyncedTitle": "服务状态尚未同步，请先刷新",
  "config.proxyPort": "独立 Web 反代端口",
  "config.autoAssign": "自动分配",
  "config.changeProxyPort": "修改反代端口",
  "config.externalLogin": "外部 Web 登录",
  "config.proxyPasswordLegacy": "旧反代密码保护中",
  "config.proxyPasswordNative": "QwenPaw 原生账号",
  "config.proxyPasswordUnknown": "无法确认",
  "config.switchToNative": "切换为原生登录",
  "config.allowExternalProxy": "允许从其他设备访问独立 Web 反代（默认仅本机）",
  "config.proxyUnknownTitle": "无法确认反代端口或访问范围，请重试",
  "config.authSection": "访问鉴权",
  "config.webLoginAuth": "Web 登录验证",
  "config.adminIdentity": "管理员身份",
  "config.entryMigrated": "已切换为入口登录",
  "config.migrateEntry": "迁移旧账号到入口登录",
  "config.resetAdminPassword": "重置 QwenPaw 管理员密码",

  // Auth states
  "auth.initial.label": "状态获取中...",
  "auth.initial.admin": "-",
  "auth.initial.desc": "本控制台使用飞牛账号登录；QwenPaw 对话端使用独立管理员账号，两套密码不会自动同步。",
  "auth.uninitialized.label": "已开启（待设置密码）",
  "auth.uninitialized.admin": "尚未创建管理员",
  "auth.uninitialized.desc": "对话端已要求登录验证，但还没有管理员。请进入对话端完成初始设置；该账号独立于飞牛账号。",
  "auth.protected.label": "已开启（需要密码）",
  "auth.protected.admin": "管理员已创建",
  "auth.protected.desc": "忘记密码时，可在此为原 QwenPaw 管理员设置新密码。无需重新注册，不影响飞牛账号、配置或记忆。",
  "auth.disabled.label": "未开启（公开访问）",
  "auth.disabled.admin": "未启用账号验证",
  "auth.disabled.desc": "任何能访问该端口的人无需密码即可使用对话端。如需保护，请在对话端设置 QwenPaw 管理员。",
  "auth.unknown.label": "无法确认",
  "auth.unknown.admin": "无法读取",
  "auth.unknown.desc": "内核未运行或状态接口不可达，暂时无法确认密码状态；这不代表验证已关闭。请启动内核后再确认。",
  "auth.offline.label": "无法获取",
  "auth.offline.admin": "无法获取",
  "auth.offline.desc": "暂时无法连接管理服务（可能服务重启中或会话已过期）。这不代表内核已停止，稍后会自动重试。",

  // Settings Modals & Toasts
  "settings.portNotSyncedToast": "服务状态尚未同步，请先刷新后再修改端口",
  "settings.changePortTitle": "修改服务端口",
  "settings.changePortMsg": "请输入新的 QwenPaw Web 访问端口（1024~65535）。保存后服务将自动重启以应用新端口。",
  "settings.changePortConfirm": "确认修改并重启",
  "settings.newPortLabel": "新端口",
  "settings.invalidPort": "端口无效，请输入 1024~65535 之间的数字",
  "settings.changePortVerifyTitle": "修改服务端口确认",
  "settings.changePortVerifyMsg": "确定要将 QwenPaw Web 端口修改为 {port} 吗？\n\n保存后服务将自动重启以应用新端口。",
  "settings.savingPort": "正在保存新端口 {port}...",
  "settings.changePortFailed": "修改端口失败：{error}",
  "settings.unknownError": "未知错误",
  "settings.portChanged": "端口已变更为 {port}，正在重启...",
  "settings.proxyUnknownToast": "无法确认反代端口或访问范围，请重试",
  "settings.changeProxyTitle": "修改 Web 反代端口",
  "settings.changeProxyMsg": "输入 0 自动选择空闲端口，或输入 1024~65535 的指定端口。新端口绑定成功后才会切换，失败保留原端口。",
  "settings.changeProxyConfirm": "保存并切换",
  "settings.proxyPortLabel": "反代端口",
  "settings.proxyPortPlaceholder": "0 表示自动分配",
  "settings.invalidProxyPort": "端口无效，请输入 0 或 1024~65535",
  "settings.switchProxyTitle": "切换 Web 反代端口",
  "settings.switchProxyAutoMsg": "确定自动选择空闲端口并切换 Web 反代吗？",
  "settings.switchProxyFixedMsg": "确定将 Web 反代端口切换为 {port} 吗？",
  "settings.switchProxyConfirm": "确认切换",
  "settings.proxyScopeUnknown": "无法确认反代访问范围，请重试",
  "settings.proxyInvalidBackend": "后端未返回有效的反代端口",
  "settings.proxyChanged": "Web 反代端口已切换为 {port}",
  "settings.proxyChangeFailed": "修改反代端口失败：{error}",
  "settings.allowPublicTitle": "允许外部访问 Web 反代",
  "settings.allowPublicMsg": "开启后，同一网络中的其他设备可通过该端口访问 QwenPaw 主 Web。请先在本机完成 QwenPaw 原账号注册，再开放端口。公网请通过 HTTPS 反代访问。",
  "settings.allowPublicConfirm": "允许外部访问",
  "settings.publicEnabled": "已允许外部访问 Web 反代",
  "settings.publicDisabled": "Web 反代已改为仅本机访问",
  "settings.scopeChangeFailed": "切换访问范围失败：{error}",
  "settings.nativeLoginSwitched": "已切换为 QwenPaw 原生登录，旧反代会话已失效",
  "settings.nativeLoginFailed": "未确认切换：{error}",
  "settings.migrateTitle": "迁移到入口登录",
  "settings.migrateMsg": "请先从飞牛桌面用原 QwenPaw 账号登录一次。确认后，飞牛入口使用平台管理员身份。独立反代已设密码时使用实例密码，未设时使用 QwenPaw 原生认证；本机服务端口保留原生登录。",
  "settings.migrateConfirm": "确认迁移",
  "settings.migrateNoToken": "未找到旧会话，请在同一浏览器的飞牛入口登录原账号后重试",
  "settings.migrateDone": "已迁移，重新打开对话即可使用入口登录",
  "settings.migrateFailed": "迁移未确认：{error}",
  "settings.resetDone": "密码已重置，请用原用户名和新密码登录",
  "settings.resetFailed": "密码重置未确认：{error}",

  // Native Login & Reset Password Dialogs
  "nativeLogin.title": "切换为原生登录",
  "nativeLogin.hint": "请输入 QwenPaw 原账号，不是飞牛账号。验证成功才停用旧反代密码，已登录反代的设备需要重新登录。",
  "nativeLogin.username": "QwenPaw 用户名",
  "nativeLogin.password": "QwenPaw 密码",
  "nativeLogin.submit": "验证并切换",

  "resetPwd.title": "重置 QwenPaw 管理员密码",
  "resetPwd.hint": "不需要旧密码。保存后请用原用户名和新密码登录，已有的 QwenPaw 原生登录会话将失效。飞牛账号和聊天记录不受影响。",
  "resetPwd.newPassword": "新密码",
  "resetPwd.confirmPassword": "确认新密码",
  "resetPwd.submit": "保存新密码",
  "resetPwd.blankError": "新密码不能全为空格",
  "resetPwd.mismatchError": "两次输入不一致，请重新确认",

  // Authorization & Backup
  "authz.title": "授权与备份",
  "authz.workspaceDirs": "工作区目录",
  "authz.refresh": "刷新",
  "authz.addDir": "选择飞牛目录",
  "authz.subtitle": "通过飞牛授权添加项目目录；移除绑定不会删除文件。",
  "authz.loading": "正在读取工作区目录…",
  "authz.empty": "尚未绑定项目目录。选择飞牛目录后，可在对话和文件区中使用。",
  "authz.openDir": "打开目录",
  "authz.reauthorize": "重新授权",
  "authz.removeBind": "移除绑定",
  "authz.backupSection": "数据备份",
  "authz.backupTitle": "QwenPaw 备份",
  "authz.backupDesc": "使用上游备份页面创建、导入和恢复；备份可能包含敏感凭证。",
  "authz.openBackup": "打开备份管理",
  "authz.openBackupReadyTitle": "打开 QwenPaw 上游备份管理",
  "authz.openBackupDisabledTitle": "启动 QwenPaw 后可使用备份管理",
  "authz.offlineRecoverySummary": "安装失败，如何离线恢复？",
  "authz.offlineRecoveryP1": "安装前可在「概览 → 部署版本」勾选备份。已有数据会保存到应用私有 var 目录的 ",
  "authz.offlineRecoveryP1End": "，不覆盖旧备份。空安装不会生成文件。",
  "authz.offlineRecoveryP2a": "用 NAS 管理员权限将 tar 复制到应用目录外的私有位置。停用应用，解到一个空目录，核对 ",
  "authz.offlineRecoveryP2b": " 和原有的 ",
  "authz.offlineRecoveryP2c": "，先另存现有目录，再按原属主权限还原。不要把它导入上游 ZIP 备份页。",
  "authz.offlineRecoveryP3": "备份可能含密钥，不要公开分享。授权的外部目录、外壳设置和运行环境不在其中；彻底卸载前必须移出备份。",

  "authz.bindFailed": "目录绑定失败：{error}",
  "authz.readFailed": "读取工作区目录失败：{error}",
  "authz.opening": "正在打开飞牛授权，等待回调…",
  "authz.noSelectableDir": "授权结果中没有可选目录",
  "authz.receivedBinding": "已收到授权目录 {path}，正在绑定…",
  "authz.readingExisting": "正在读取现有工作区目录…",
  "authz.savingWorkspace": "正在保存到 QwenPaw 工作区…",
  "authz.bindSuccess": "目录已授权并绑定到工作区",
  "authz.noReturn": "无返回值",
  "authz.emptyResult": "空结果",
  "authz.openReturned": "openFileManager 返回：{detail}；请确认文件管理器是否打开",
  "authz.openFailed": "打开目录失败：{error}{diagnostics}",
  "authz.reauthOk": "目录已重新授权，可读写",
  "authz.reauthLimited": "目录已重新授权，但读写权限仍不足",
  "authz.removeTitle": "移除目录绑定",
  "authz.removeMsg": "仅从 QwenPaw 工作区移除 {path}，不会删除目录或文件。",
  "authz.removeConfirm": "移除绑定",
  "authz.removedToast": "已移除目录绑定，文件未删除",
  "authz.restoring": "检测到已完成的授权，正在恢复目录绑定…",
  "authz.restored": "已恢复授权目录绑定",
  "authz.err.stateMismatch": "授权回调状态不匹配",
  "authz.err.cancelled": "已取消授权",
  "authz.err.missingDir": "授权回调缺少目录",
  "authz.err.invalidFormat": "授权目录格式无效",
  "authz.err.missingValidDir": "授权回调缺少有效目录",
  "authz.err.absoluteRequired": "目录必须是绝对路径",
  "authz.err.invalidSegment": "目录包含非法路径段",
  "authz.err.loadFailed": "读取工作区目录失败 ({status})",
  "authz.err.saveFailed": "保存工作区目录失败 ({status})",
  "authz.err.checkFailed": "目录权限复查失败 ({status})",
  "authz.err.popupBlocked": "浏览器拦截了授权窗口，请允许弹窗后重试",
  "authz.err.timeout": "授权超时，请重试",
  "authz.surfaceTitle": "飞牛目录授权",
  "authz.diag.type": "getAppMessage类型={type}",
  "authz.diag.missing": "缺失",
  "authz.diag.none": "无",
  "authz.diag.fields": "getAppMessage字段={keys}，appApi={appApi}",
  "authz.diag.readFailed": "getAppMessage读取失败={error}",

  // Logs
  "logs.title": "运行日志",
  "logs.export": "导出",
  "logs.fileTitle": "QwenPaw 运行日志文件",
  "logs.autoScroll": "自动滚底",
  "logs.tools": "筛选与管理",
  "logs.lineCount": "{visible} / {total} 行",
  "logs.clearFilter": "清除",
  "logs.autoRefresh": "实时刷新",
  "logs.copyFiltered": "复制当前结果",
  "logs.clearLogs": "清空日志",
  "logs.filterAria": "日志筛选",
  "logs.levelAria": "日志级别",
  "logs.level.all": "全部",
  "logs.level.warn": "警告+",
  "logs.level.error": "错误",
  "logs.keywordLabel": "关键字筛选",
  "logs.keywordPlaceholder": "输入日志关键字",
  "logs.resetFilter": "重置",
  "logs.showingCount": "显示 {visible} / {total} 行",
  "logs.retry": "重试",
  "logs.terminalAria": "QwenPaw 运行日志",
  "logs.emptyError": "暂时无法读取日志",
  "logs.emptyFiltered": "当前筛选条件下无匹配日志",
  "logs.empty": "（当前无日志输出）",
  "logs.readError": "读取日志失败，可重试。",
  "logs.clearTitle": "删除服务日志",
  "logs.clearMsg": "这将删除后端保存的服务日志，而不仅仅清空当前显示。删除后无法恢复，确认继续？",
  "logs.clearConfirm": "确认删除日志",
  "logs.clearFailedStatus": "删除日志失败 ({status})",
  "logs.clearedToast": "已请求清理服务日志",
  "logs.clearFailed": "删除日志失败：{error}",
  "logs.readFailedStatus": "读取日志失败（{status}）",
  "logs.exportedToast": "已导出 {count} 行日志",
  "logs.exportFailed": "导出日志失败：{error}",
  "logs.nothingToCopy": "当前无日志可复制",
  "logs.clipboardUnsupported": "浏览器不支持剪贴板写入",
  "logs.copiedToast": "已复制 {count} 行日志",
  "logs.copyFailed": "复制日志失败：{error}",

  // Service & Uninstall
  "service.versionUnconfirmed": "尚未确认当前内核版本，请刷新后再操作",
  "service.notInstalled": "当前未安装 QwenPaw 内核",
  "service.uninstallTitle": "仅卸载 QwenPaw 内核",
  "service.uninstallMsg": "将停止服务并删除 QwenPaw v{version} 的运行环境与版本钉。\n\n• 用户配置、会话记忆与管理员认证均完整保留，之后可随时重新安装。\n• 注意：受飞牛 fnOS 桌面会话缓存限制，桌面图标隐藏可能需刷新浏览器页面生效。确认继续？",
  "service.uninstallConfirm": "仅卸载内核，保留数据",
  "service.uninstalling": "正在卸载内核并保留用户数据...",
  "service.uninstalled": "内核已卸载",
  "service.uninstallFailed": "卸载请求失败：{error}",
  "service.act.start": "启动",
  "service.act.stop": "停止",
  "service.act.restart": "重启",
  "service.confirmTitle": "{action}服务确认",
  "service.confirmMsg": "确定要立即{action} QwenPaw 服务吗？{extra}",
  "service.restartExtra": "重启期间连接会短暂中断。",
  "service.confirmBtn": "确认{action}",
  "service.pending": "正在{action}服务…",
  "service.done": "{action}指令已成功执行",
  "service.failed": "操作失败：{error}",

  // Install Coordinator & Progress
  "install.paused": "安装已暂停：{reason}",
  "install.unreachable": "暂时无法确认管理服务状态，暂不可安装",
  "install.pickFirst": "请先获取版本列表并选择目标版本。",
  "install.backupNote": "先停机备份已有数据；备份或校验失败将中止安装。\n\n",
  "install.unpatchedNote": "本安装包没有此版本的适配补丁。从飞牛桌面打开 QwenPaw Web 时，页面可能无法加载。\n\n",
  "install.confirm.installTitle": "安装内核确认",
  "install.confirm.installMsg": "{note}将下载并安装 QwenPaw v{version}。确认开始安装？",
  "install.confirm.installBtn": "开始安装",
  "install.confirm.reinstallTitle": "重新安装当前版本",
  "install.confirm.reinstallMsg": "{note}将通过独立暂存环境重新安装 QwenPaw v{version}，运行中的服务会中断。安装失败时会尝试恢复运行环境，不会自动还原用户数据。确认继续？",
  "install.confirm.reinstallBtn": "重新安装",
  "install.confirm.downgradeTitle": "降级风险警告",
  "install.confirm.downgradeMsg": "{note}将从 v{current} 降级到 v{version}。旧版本可能无法读取现有配置或会话数据，也可能无法启动。确认降级？",
  "install.confirm.downgradeBtn": "确认降级",
  "install.confirm.downgradeCancel": "取消并返回",
  "install.confirm.upgradeTitle": "在线升级确认",
  "install.confirm.upgradeMsg": "{note}即将安装 QwenPaw v{version} 并重新构建环境，运行中的服务会中断。确认继续？",
  "install.confirm.upgradeBtn": "开始升级",
  "install.recoveryBlocked": "上一次事务需要恢复，暂不可安装",
  "install.alreadyRunning": "已有安装任务正在进行中，已切换到进度查看",
  "install.changedRetry": "版本或下载源已变更，请重新确认安装",
  "install.alreadyRunningFallback": "已有安装任务正在进行中，已切换到进度查看",
  "install.notStarted": "安装未开始：{reason}",
  "install.backendRejected": "后端拒绝",
  "install.acceptFailed": "安装受理失败（HTTP {status}）",
  "install.requestFailed": "安装请求失败：{error}",

  "progress.stepsAria": "安装进度",
  "progress.step.downloading": "下载依赖",
  "progress.step.installing": "安装内核",
  "progress.step.starting": "启动检查",
  "progress.minimize": "转到后台运行",
  "progress.close": "完成并返回控制台",
  "progress.timedOutTitle": "任务仍未结束",
  "progress.disconnectedTitle": "与控制台连接中断",
  "progress.timedOutSub": "安装任务仍在后台执行，可关闭本窗口；稍后在控制台查看进度或日志",
  "progress.disconnectedSub": "无法连接监督者。任务可能仍在后台执行，可关闭本窗口稍后重试",
  "progress.viewingOwnedTitle": "正在查看已归属的安装任务...",
  "progress.viewingOwnedSub": "正在重新读取任务状态",
  "progress.viewingRunningTitle": "正在查看进行中的安装任务...",
  "progress.runningTitle": "QwenPaw 正在{action} v{version}...",
  "progress.viewingRunningSub": "NAS 后端独立执行中，与本页面是否在线无关",
  "progress.runningSub": "正在执行 pip 下载与环境重构，期间服务将自动重启",
  "progress.fetchingStatus": "正在获取实时安装状态...",
  "progress.minimizedToast": "已转入后台运行，可在控制台查看进度",
  "progress.readStatusFailed": "无法读取安装状态（HTTP {status}）",
  "progress.noRunningTask": "当前没有进行中的安装任务",
  "progress.cannotConnect": "无法连接后端：{error}",
  "progress.waitingPip": "正在等待 pip 安装输出...",
  "progress.phase.downloading": "正在下载依赖包...",
  "progress.phase.installing": "正在安装内核...",
  "progress.phase.starting": "正在启动并检查服务...",
  "progress.phase.running": "安装任务正在执行...",
  "progress.unconfirmedTitle": "无法确认安装任务结果",
  "progress.unconfirmedNoReceipt": "触发请求可能已送达并完成；请关闭本窗口后在控制台查看当前版本与安装日志确认",
  "progress.unconfirmedMismatch": "后端任务与本次操作不匹配，请关闭本窗口后在控制台查看安装日志",
  "progress.taskLabel": "安装任务",
  "progress.failedTitle": "{action}遇到问题",
  "progress.recoveryError": "环境恢复未完成：{error}",
  "progress.recoveryIncomplete": "环境恢复未完成，请查看日志并按提示处理",
  "progress.checkLogsNote": "请查看日志确认环境恢复状态",
  "progress.doneTitle": "{action}已顺利完成！",
  "progress.doneSub": "所选版本已就绪，QwenPaw 后台服务已恢复运行",
  "progress.doneNoLogs": "安装任务已完成，未获取到日志输出。",
  "progress.bgFailedToast": "{action}失败，请打开控制台查看详情",
  "progress.bgDoneToast": "{action}已完成",

  // Common Modal & About
  "common.cancel": "取消",
  "common.confirm": "确认执行",
  "common.close": "关闭",
  "common.promptTitle": "提示确认",
  "common.inputLabel": "输入",
  "common.invalidInput": "输入格式不正确，请检查后重试。",
  "common.opFailed": "操作失败（HTTP {status}）",

  "about.title": "关于 QwenPaw",
  "about.desc": "QwenPaw for fnOS 是面向飞牛 NAS 的第三方部署外壳，负责内核安装、进程监管与运维管理；QwenPaw 本体由所选 PyPI 镜像按需安装。",
  "about.linksAria": "项目地址",
  "about.upstreamTitle": "上游 QwenPaw 官网 / 项目",
  "about.wrapperTitle": "QwenPaw for fnOS 项目",
  "about.faqTitle": "常见问题处理",
  "about.faq1.q": "无法打开 Web 对话",
  "about.faq1.a": "先在「概览」确认内核已安装且正在运行；仍无法打开时，到「日志」查看启动输出、端口或网关报错。",
  "about.faq2.q": "安装或更新失败",
  "about.faq2.a": "检查 NAS 网络与 DNS，在「概览 → 部署版本」切换下载源后重试。已有安装任务时请先打开「查看安装任务」，不要重复提交。",
  "about.faq3.q": "忘记 QwenPaw 管理员密码",
  "about.faq3.a": "内核运行中时，用飞牛管理员打开管理台，在「配置 → 访问鉴权」点击「重置 QwenPaw 管理员密码」，再用原用户名和新密码登录。无需旧密码，不影响飞牛账号、配置或聊天记录。",
  "about.faq4.q": "升级后出现权限或运行异常",
  "about.faq4.a": "请保留运行日志与用户数据，不要手工删除应用运行环境或共享数据目录；导出日志并记录报错时间与提示，到项目 Issues 反馈。",

  // Backend API code messages
  "backend.operation_in_progress": "已有管理操作正在进行中，请稍后再试",
  "backend.recovery_pending": "上一次事务尚未完成恢复，暂不可执行该操作",
  "backend.install_busy": "安装正在进行中，请等待完成后再试",
  "backend.kernel_not_installed": "QwenPaw 内核尚未安装，请在「部署版本」中选择版本安装",
  "backend.service_started": "QwenPaw 已启动",
  "backend.service_already_running": "QwenPaw 已在运行",
  "backend.service_stopped": "QwenPaw 已停止",
  "backend.service_restarted": "QwenPaw 已重启",
  "backend.logs_cleared": "日志已清理",
  "backend.install_started": "安装已开始",
  "backend.kernel_uninstalled": "内核已卸载，用户配置、记忆与管理员认证已保留",
  "backend.native_auth_uninitialized": "未启用原生鉴权或尚无已注册账号",
  "backend.native_auth_mismatch": "原生账号与飞牛入口绑定不一致，未更改",
  "backend.native_auth_unconfirmed": "无法确认账号记录或写入结果，请核对内核状态",
  "backend.native_auth_no_ack": "未收到内核确认，请先用新密码尝试登录",
  "backend.mirror_saved": "下载源已保存",
  "backend.port_in_use": "端口已被内核或管理器使用",
  "backend.web_proxy_disabled": "Web 反代未启用",
  "backend.web_proxy_port_changed": "Web 反代端口已切换",
} as const;

export type MessageKey = keyof typeof zhCN;

const en: Record<MessageKey, string> = {
  // Page / Header
  "app.title": "QwenPaw Console",
  "nav.ariaLabel": "Console sections",
  "nav.overview": "Overview",
  "nav.configuration": "Settings",
  "nav.authorization": "Access & Backup",
  "nav.logs": "Logs",

  // Header entry & badge
  "header.entry.connecting": "Connecting…",
  "header.entry.connectingShort": "Connect",
  "header.entry.check": "Check Status",
  "header.entry.checkShort": "Check",
  "header.entry.recovery": "View Recovery",
  "header.entry.recoveryShort": "Recover",
  "header.entry.install": "Install & Start",
  "header.entry.installShort": "Install",
  "header.entry.start": "Start Service First",
  "header.entry.startShort": "Start",
  "header.entry.chat": "Open Web Chat",
  "header.entry.chatShort": "Chat",

  "header.badge.checking": "Checking…",
  "header.badge.unreachable": "Unreachable",
  "header.badge.recovery": "Recovery Needed",
  "header.badge.uninstalled": "Not Installed",
  "header.badge.running": "Running",
  "header.badge.stopped": "Stopped",

  "header.about": "About",
  "header.aboutTitle": "About & FAQ",
  "header.taskCenter": "Install Task",
  "header.taskCenterAria": "View install task details",
  "header.taskCenterShort": "Task",

  // Theme
  "theme.system": "Auto",
  "theme.light": "Light",
  "theme.dark": "Dark",
  "theme.aria.systemToDark": "Following system (Light), switch to dark mode",
  "theme.aria.systemToLight": "Following system (Dark), switch to light mode",
  "theme.aria.lightToDark": "Light mode, switch to dark mode",
  "theme.aria.lightToSystem": "Light mode, switch to follow system",
  "theme.aria.darkToLight": "Dark mode, switch to light mode",
  "theme.aria.darkToSystem": "Dark mode, switch to follow system",

  // Locale toggle
  "locale.label": "EN",
  "locale.short": "EN",
  "locale.switchAria": "Current language: English, switch to Simplified Chinese",

  // Overview
  "overview.uptimePrefix": "Up {uptime}",
  "overview.refreshTitle": "Refresh service status",
  "overview.connecting": "Connecting to service",
  "overview.connectingDesc": "Fetching service status and resource usage…",
  "overview.metricsAria": "Runtime metrics",
  "overview.cpu": "CPU Usage",
  "overview.memory": "Memory",
  "overview.pid": "Process PID",
  "overview.uptime": "Uptime",
  "overview.detailsAria": "Current configuration summary",
  "overview.currentVersion": "Version",
  "overview.servicePort": "Service Port",
  "overview.authStatus": "Login Auth",
  "overview.targetVersion": "Target Version",
  "overview.bundledPatch": "Bundled patch",
  "overview.selectVersion": "Select a version",
  "overview.startInstall": "Install",
  "overview.changeTargetNote": "You can change the target version in Deployment.",
  "overview.runningInstall": "Install in progress · View progress",
  "overview.startService": "Start",
  "overview.restartService": "Restart",
  "overview.stopService": "Stop",
  "overview.firstUse": "Getting Started",
  "overview.guideStep1": "Click Check Updates in Deployment, choose a target version, and install",
  "overview.guideStep2": "Wait for dependency download, kernel installation, and startup checks",
  "overview.guideStep3": "Click Open Web Chat to start using QwenPaw",

  // Status Card & Panel
  "status.unreachable": "Unreachable",
  "status.recovery": "Recovery Needed",
  "status.uninstalled": "Not Installed",
  "status.running": "Running",
  "status.stopped": "Stopped",
  "status.desc.unreachable": "Unable to fetch service status. Check your network or sign in to fnOS again.{detail}",
  "status.desc.recovery": "Do not reinstall or toggle the service yet. Inspect the logs and preserve state. Reason: {reason}",
  "status.desc.uninstalled": "Install the QwenPaw kernel first to start using AI chat.",
  "status.desc.running": "The kernel process is running. CPU, memory, PID, and uptime are shown below.",
  "status.desc.stopped": "The kernel is installed. Start the service to open Web Chat.",
  "status.viewLogs": "View Runtime Logs",
  "status.waitingSync": "Waiting for first sync",
  "status.lastSync": "Last synced {time}",
  "status.syncFailed": "Sync failed · Retrying automatically",
  "status.versionPending": "Pending",
  "status.versionUninstalled": "Not installed",
  "status.unavailable": "Unavailable",
  "status.invalidSnapshot": "Invalid service status",
  "status.recoveryFallback": "Previous transaction did not complete cleanly",

  // Deployment & Version Picker
  "deploy.title": "Deployment",
  "deploy.checkUpdates": "Check Updates",
  "deploy.currentKernelVersion": "Installed Kernel",
  "deploy.uninstalledTip": "Kernel not installed. Pick a target version and click Install below; click Check Updates if the list is empty.",
  "deploy.runningTip": "Install task is running, view progress",
  "deploy.backupBeforeInstall": "Back up existing data before install",
  "deploy.downgradeWarn": "Target version is older than the installed kernel. Older versions may fail to read newer config or memory.",
  "deploy.unreachableTitle": "Cannot reach management service; action unavailable",
  "deploy.uninstallKernel": "Uninstall Kernel Only",
  "deploy.selectTarget": "Select target version",
  "deploy.action.install": "Install",
  "deploy.action.downgrade": "Downgrade to",
  "deploy.action.upgrade": "Upgrade to",
  "deploy.action.reinstall": "Reinstall",
  "deploy.actionShort.install": "Install",
  "deploy.actionShort.downgrade": "Downgrade",
  "deploy.actionShort.upgrade": "Upgrade",
  "deploy.actionShort.reinstall": "Reinstall",
  "deploy.sort.allDates": "Sorted by release date (newest first)",
  "deploy.sort.partialDates": "Sorted by release date (newest first); undated versions appear after by version number",
  "deploy.sort.noDates": "Release dates unavailable from this source; sorted by version number (highest first)",

  "version.targetLabel": "Target Version",
  "version.loading": "Loading versions…",
  "version.pickHint": "Select target version",
  "version.emptyHint": "Click Check Updates to load versions",
  "version.beta": "Beta",
  "version.stable": "Stable",
  "version.currentInstalled": "Installed",
  "version.sourceUnavailable": "The current download source is unavailable. Switch to another source, or check NAS network, DNS, and proxy settings.",
  "version.staleWarning": "Showing previous results; cannot be used for installation.",
  "version.noRecommended": "This source does not list the version matching the bundled adaptation patch. Pick another version or switch sources.",
  "version.emptyList": "No versions available from this source. Try switching sources or retrying later.",
  "version.checkingToast": "Checking PyPI for available versions…",
  "version.listedToast": "Found {count} versions from the current source.",

  // Mirror Picker
  "mirror.label": "Download Source",
  "mirror.view": "View Source",
  "mirror.aliyun": "Aliyun Mirror",
  "mirror.tsinghua": "Tsinghua Mirror",
  "mirror.ustc": "USTC Mirror",
  "mirror.tencent": "Tencent Cloud Mirror",
  "mirror.pypi": "Official PyPI",
  "mirror.saved": "Download source saved",
  "mirror.saveFailed": "Failed to save download source: {error}",

  // Configuration
  "config.title": "Service Settings",
  "config.language": "Language",
  "config.languageSystem": "System",
  "config.languageZh": "简体中文",
  "config.languageEn": "English",
  "config.theme": "Appearance",
  "config.themeSystem": "System",
  "config.themeLight": "Light",
  "config.themeDark": "Dark",
  "config.uiStyle": "UI Style",
  "config.uiStyleDefault": "Default",
  "config.uiStyleMiuix": "Miuix",
  "config.servicePort": "Service Port",
  "config.changePort": "Change Port",
  "config.portNotSyncedTitle": "Status not synced yet; refresh first",
  "config.proxyPort": "Standalone Web Proxy Port",
  "config.autoAssign": "Auto",
  "config.changeProxyPort": "Change Proxy Port",
  "config.externalLogin": "External Web Login",
  "config.proxyPasswordLegacy": "Legacy proxy password active",
  "config.proxyPasswordNative": "QwenPaw Native Account",
  "config.proxyPasswordUnknown": "Unknown",
  "config.switchToNative": "Switch to Native Login",
  "config.allowExternalProxy": "Allow other devices to access the standalone Web proxy (loopback only by default)",
  "config.proxyUnknownTitle": "Unable to confirm proxy port or access scope; retry",
  "config.authSection": "Authentication",
  "config.webLoginAuth": "Web Login Auth",
  "config.adminIdentity": "Admin Account",
  "config.entryMigrated": "Entry Login Enabled",
  "config.migrateEntry": "Migrate Legacy Account to Entry Login",
  "config.resetAdminPassword": "Reset QwenPaw Admin Password",

  // Auth states
  "auth.initial.label": "Checking…",
  "auth.initial.admin": "-",
  "auth.initial.desc": "This console uses your fnOS account; QwenPaw Web uses its own admin account. Passwords are not synced automatically.",
  "auth.uninitialized.label": "Enabled (Setup Pending)",
  "auth.uninitialized.admin": "No admin created",
  "auth.uninitialized.desc": "Web login is required, but no admin account exists yet. Open Web Chat to complete initial setup; this account is separate from fnOS.",
  "auth.protected.label": "Enabled (Password Required)",
  "auth.protected.admin": "Admin created",
  "auth.protected.desc": "If you forget the password, set a new password here for the existing QwenPaw admin without re-registering or affecting fnOS account, config, or memory.",
  "auth.disabled.label": "Disabled (Public Access)",
  "auth.disabled.admin": "Auth not enabled",
  "auth.disabled.desc": "Anyone who can reach this port can access Web Chat without a password. Set up a QwenPaw admin in Web Chat to protect it.",
  "auth.unknown.label": "Unknown",
  "auth.unknown.admin": "Unreadable",
  "auth.unknown.desc": "Kernel is stopped or status API is unreachable, so auth state cannot be confirmed; this does not mean auth is disabled. Start the kernel to verify.",
  "auth.offline.label": "Unavailable",
  "auth.offline.admin": "Unavailable",
  "auth.offline.desc": "Temporarily unable to reach the management service (restarting or session expired). This does not mean the kernel has stopped; retrying automatically.",

  // Settings Modals & Toasts
  "settings.portNotSyncedToast": "Status not synced yet. Refresh before changing the port.",
  "settings.changePortTitle": "Change Service Port",
  "settings.changePortMsg": "Enter a new QwenPaw Web port (1024–65535). The service will restart automatically after saving.",
  "settings.changePortConfirm": "Save & Restart",
  "settings.newPortLabel": "New Port",
  "settings.invalidPort": "Invalid port. Must be a number between 1024 and 65535.",
  "settings.changePortVerifyTitle": "Confirm Service Port Change",
  "settings.changePortVerifyMsg": "Change the QwenPaw Web port to {port}?\n\nThe service will restart automatically to apply the change.",
  "settings.savingPort": "Saving new port {port}…",
  "settings.changePortFailed": "Failed to change port: {error}",
  "settings.unknownError": "Unknown error",
  "settings.portChanged": "Port changed to {port}, restarting…",
  "settings.proxyUnknownToast": "Unable to confirm proxy port or scope. Please retry.",
  "settings.changeProxyTitle": "Change Web Proxy Port",
  "settings.changeProxyMsg": "Enter 0 to auto-assign a free port, or specify 1024–65535. The port switches only after binding succeeds; failures keep the previous port.",
  "settings.changeProxyConfirm": "Save & Switch",
  "settings.proxyPortLabel": "Proxy Port",
  "settings.proxyPortPlaceholder": "0 for auto-assign",
  "settings.invalidProxyPort": "Invalid port. Enter 0 or 1024–65535.",
  "settings.switchProxyTitle": "Switch Web Proxy Port",
  "settings.switchProxyAutoMsg": "Auto-assign a free port and switch the Web proxy?",
  "settings.switchProxyFixedMsg": "Switch the Web proxy port to {port}?",
  "settings.switchProxyConfirm": "Confirm Switch",
  "settings.proxyScopeUnknown": "Unable to confirm proxy access scope. Please retry.",
  "settings.proxyInvalidBackend": "Backend did not return a valid proxy port",
  "settings.proxyChanged": "Web proxy port switched to {port}",
  "settings.proxyChangeFailed": "Failed to change proxy port: {error}",
  "settings.allowPublicTitle": "Allow External Access to Web Proxy",
  "settings.allowPublicMsg": "Other devices on the same network will be able to access QwenPaw Web via this port. Register your QwenPaw native account locally first. Use an HTTPS reverse proxy for public internet access.",
  "settings.allowPublicConfirm": "Allow External Access",
  "settings.publicEnabled": "External access enabled for Web proxy",
  "settings.publicDisabled": "Web proxy restricted to loopback only",
  "settings.scopeChangeFailed": "Failed to change access scope: {error}",
  "settings.nativeLoginSwitched": "Switched to QwenPaw native login; legacy proxy sessions invalidated",
  "settings.nativeLoginFailed": "Switch not confirmed: {error}",
  "settings.migrateTitle": "Migrate to Entry Login",
  "settings.migrateMsg": "Sign in once from the fnOS desktop with your original QwenPaw account first. After confirmation, the fnOS entry uses the platform admin identity. Standalone proxy uses instance password if set, or native auth otherwise; local service port keeps native login.",
  "settings.migrateConfirm": "Confirm Migration",
  "settings.migrateNoToken": "No legacy session found. Sign in to the original account via fnOS entry in this browser and retry.",
  "settings.migrateDone": "Migrated. Reopen Web Chat to use entry login.",
  "settings.migrateFailed": "Migration not confirmed: {error}",
  "settings.resetDone": "Password reset. Sign in with your existing username and the new password.",
  "settings.resetFailed": "Password reset not confirmed: {error}",

  // Native Login & Reset Password Dialogs
  "nativeLogin.title": "Switch to Native Login",
  "nativeLogin.hint": "Enter your original QwenPaw account, not your fnOS account. The legacy proxy password is disabled only after verification; connected proxy devices will need to sign in again.",
  "nativeLogin.username": "QwenPaw Username",
  "nativeLogin.password": "QwenPaw Password",
  "nativeLogin.submit": "Verify & Switch",

  "resetPwd.title": "Reset QwenPaw Admin Password",
  "resetPwd.hint": "No old password required. Sign in with your existing username and new password after saving; existing native sessions will be invalidated. fnOS account and chat history are unaffected.",
  "resetPwd.newPassword": "New Password",
  "resetPwd.confirmPassword": "Confirm New Password",
  "resetPwd.submit": "Save New Password",
  "resetPwd.blankError": "New password cannot be whitespace only",
  "resetPwd.mismatchError": "Passwords do not match",

  // Authorization & Backup
  "authz.title": "Access & Backup",
  "authz.workspaceDirs": "Workspace Directories",
  "authz.refresh": "Refresh",
  "authz.addDir": "Select fnOS Folder",
  "authz.subtitle": "Authorize project folders via fnOS; unbinding does not delete files.",
  "authz.loading": "Loading workspace directories…",
  "authz.empty": "No project folders bound yet. Select an fnOS folder to use it in chat and files.",
  "authz.openDir": "Open Folder",
  "authz.reauthorize": "Reauthorize",
  "authz.removeBind": "Unbind",
  "authz.backupSection": "Data Backup",
  "authz.backupTitle": "QwenPaw Backup",
  "authz.backupDesc": "Use the upstream backup page to create, import, and restore; backups may contain sensitive credentials.",
  "authz.openBackup": "Open Backup Manager",
  "authz.openBackupReadyTitle": "Open upstream QwenPaw backup manager",
  "authz.openBackupDisabledTitle": "Start QwenPaw to use backup management",
  "authz.offlineRecoverySummary": "How to recover offline if installation fails?",
  "authz.offlineRecoveryP1": "Enable backup in Overview → Deployment before installing. Existing data is archived to ",
  "authz.offlineRecoveryP1End": " in the private app var directory without overwriting older backups. Empty installs produce no archive.",
  "authz.offlineRecoveryP2a": "With NAS admin privileges, copy the tar to a private location outside the app directory. Stop the app, extract to an empty folder, verify ",
  "authz.offlineRecoveryP2b": " and the original ",
  "authz.offlineRecoveryP2c": ", back up the current directory first, then restore with original ownership and permissions. Do not import this tar into the upstream ZIP backup page.",
  "authz.offlineRecoveryP3": "Backups may contain secrets—never share publicly. Authorized external folders, wrapper settings, and runtime venv are excluded; move backups out before a full uninstall.",

  "authz.bindFailed": "Directory binding failed: {error}",
  "authz.readFailed": "Failed to load workspace directories: {error}",
  "authz.opening": "Opening fnOS authorization, waiting for callback…",
  "authz.noSelectableDir": "No selectable directory in authorization result",
  "authz.receivedBinding": "Received authorized folder {path}, binding…",
  "authz.readingExisting": "Reading existing workspace directories…",
  "authz.savingWorkspace": "Saving to QwenPaw workspace…",
  "authz.bindSuccess": "Directory authorized and bound to workspace",
  "authz.noReturn": "no return value",
  "authz.emptyResult": "empty result",
  "authz.openReturned": "openFileManager returned: {detail}; check if File Manager opened",
  "authz.openFailed": "Failed to open folder: {error}{diagnostics}",
  "authz.reauthOk": "Folder reauthorized with read/write access",
  "authz.reauthLimited": "Folder reauthorized, but read/write permissions are still insufficient",
  "authz.removeTitle": "Unbind Directory",
  "authz.removeMsg": "Only removes {path} from the QwenPaw workspace. No folders or files will be deleted.",
  "authz.removeConfirm": "Unbind",
  "authz.removedToast": "Directory unbound; files were not deleted",
  "authz.restoring": "Completed authorization detected, restoring binding…",
  "authz.restored": "Authorized directory binding restored",
  "authz.err.stateMismatch": "Authorization callback state mismatch",
  "authz.err.cancelled": "Authorization cancelled",
  "authz.err.missingDir": "Authorization callback missing directory",
  "authz.err.invalidFormat": "Invalid authorized directory format",
  "authz.err.missingValidDir": "Authorization callback missing valid directory",
  "authz.err.absoluteRequired": "Directory must be an absolute path",
  "authz.err.invalidSegment": "Directory contains invalid path segment",
  "authz.err.loadFailed": "Failed to load workspace directories ({status})",
  "authz.err.saveFailed": "Failed to save workspace directories ({status})",
  "authz.err.checkFailed": "Directory permission check failed ({status})",
  "authz.err.popupBlocked": "Authorization popup was blocked. Allow popups and retry.",
  "authz.err.timeout": "Authorization timed out. Please retry.",
  "authz.surfaceTitle": "fnOS Folder Authorization",
  "authz.diag.type": "getAppMessage type={type}",
  "authz.diag.missing": "missing",
  "authz.diag.none": "none",
  "authz.diag.fields": "getAppMessage keys={keys}, appApi={appApi}",
  "authz.diag.readFailed": "getAppMessage error={error}",

  // Logs
  "logs.title": "Runtime Logs",
  "logs.export": "Export",
  "logs.fileTitle": "QwenPaw runtime log file",
  "logs.autoScroll": "Auto-scroll",
  "logs.tools": "Filter & Tools",
  "logs.lineCount": "{visible} / {total} lines",
  "logs.clearFilter": "Clear",
  "logs.autoRefresh": "Live refresh",
  "logs.copyFiltered": "Copy Filtered",
  "logs.clearLogs": "Clear Logs",
  "logs.filterAria": "Log filter",
  "logs.levelAria": "Log level",
  "logs.level.all": "All",
  "logs.level.warn": "Warn+",
  "logs.level.error": "Error",
  "logs.keywordLabel": "Keyword",
  "logs.keywordPlaceholder": "Filter logs by keyword",
  "logs.resetFilter": "Reset",
  "logs.showingCount": "Showing {visible} / {total} lines",
  "logs.retry": "Retry",
  "logs.terminalAria": "QwenPaw runtime logs",
  "logs.emptyError": "Unable to read logs right now",
  "logs.emptyFiltered": "No logs match the current filter",
  "logs.empty": "(No log output)",
  "logs.readError": "Failed to read logs. Try again.",
  "logs.clearTitle": "Delete Service Logs",
  "logs.clearMsg": "This deletes the stored service logs on the backend, not just the current view. This cannot be undone. Continue?",
  "logs.clearConfirm": "Delete Logs",
  "logs.clearFailedStatus": "Failed to delete logs ({status})",
  "logs.clearedToast": "Requested service log cleanup",
  "logs.clearFailed": "Failed to delete logs: {error}",
  "logs.readFailedStatus": "Failed to read logs ({status})",
  "logs.exportedToast": "Exported {count} log lines",
  "logs.exportFailed": "Failed to export logs: {error}",
  "logs.nothingToCopy": "No logs to copy",
  "logs.clipboardUnsupported": "Clipboard write is not supported by this browser",
  "logs.copiedToast": "Copied {count} log lines",
  "logs.copyFailed": "Failed to copy logs: {error}",

  // Service & Uninstall
  "service.versionUnconfirmed": "Current kernel version is not confirmed yet. Refresh and try again.",
  "service.notInstalled": "QwenPaw kernel is not installed",
  "service.uninstallTitle": "Uninstall QwenPaw Kernel Only",
  "service.uninstallMsg": "This will stop the service and remove the QwenPaw v{version} runtime and version pin.\n\n• User configuration, session memory, and admin credentials are preserved and can be reused after reinstalling.\n• Note: Due to fnOS desktop session caching, hiding the desktop icon may require refreshing the browser page. Continue?",
  "service.uninstallConfirm": "Uninstall Kernel, Keep Data",
  "service.uninstalling": "Uninstalling kernel while keeping user data…",
  "service.uninstalled": "Kernel uninstalled",
  "service.uninstallFailed": "Uninstall request failed: {error}",
  "service.act.start": "Start",
  "service.act.stop": "Stop",
  "service.act.restart": "Restart",
  "service.confirmTitle": "Confirm {action} Service",
  "service.confirmMsg": "{action} the QwenPaw service now? {extra}",
  "service.restartExtra": "Connections will be briefly interrupted during restart.",
  "service.confirmBtn": "Confirm {action}",
  "service.pending": "{action} in progress…",
  "service.done": "{action} command completed",
  "service.failed": "Operation failed: {error}",

  // Install Coordinator & Progress
  "install.paused": "Installation paused: {reason}",
  "install.unreachable": "Cannot confirm management service status; installation unavailable",
  "install.pickFirst": "Fetch the version list and choose a target version first.",
  "install.backupNote": "Existing data will be backed up after stopping the service; backup or verification failure aborts installation.\n\n",
  "install.unpatchedNote": "This package does not include an adaptation patch for this version. Opening QwenPaw Web from the fnOS desktop may fail to load.\n\n",
  "install.confirm.installTitle": "Confirm Kernel Installation",
  "install.confirm.installMsg": "{note}Download and install QwenPaw v{version}? Continue?",
  "install.confirm.installBtn": "Start Install",
  "install.confirm.reinstallTitle": "Reinstall Current Version",
  "install.confirm.reinstallMsg": "{note}Reinstall QwenPaw v{version} in an isolated staging environment. The running service will be interrupted. On failure, the runtime will attempt to roll back, without automatically restoring user data. Continue?",
  "install.confirm.reinstallBtn": "Reinstall",
  "install.confirm.downgradeTitle": "Downgrade Risk Warning",
  "install.confirm.downgradeMsg": "{note}Downgrade from v{current} to v{version}. Older versions may fail to read existing config or session data, or fail to start. Confirm downgrade?",
  "install.confirm.downgradeBtn": "Confirm Downgrade",
  "install.confirm.downgradeCancel": "Cancel & Go Back",
  "install.confirm.upgradeTitle": "Confirm Online Upgrade",
  "install.confirm.upgradeMsg": "{note}Install QwenPaw v{version} and rebuild the environment. The running service will be interrupted. Continue?",
  "install.confirm.upgradeBtn": "Start Upgrade",
  "install.recoveryBlocked": "Previous transaction requires recovery; installation unavailable",
  "install.alreadyRunning": "An install task is already running; switched to progress view",
  "install.changedRetry": "Version or download source changed. Please confirm again.",
  "install.alreadyRunningFallback": "An install task is in progress; switched to progress view",
  "install.notStarted": "Installation not started: {reason}",
  "install.backendRejected": "Rejected by backend",
  "install.acceptFailed": "Installation acceptance failed (HTTP {status})",
  "install.requestFailed": "Installation request failed: {error}",

  "progress.stepsAria": "Installation progress",
  "progress.step.downloading": "Download",
  "progress.step.installing": "Install",
  "progress.step.starting": "Start Check",
  "progress.minimize": "Run in Background",
  "progress.close": "Done & Return",
  "progress.timedOutTitle": "Task Still Running",
  "progress.disconnectedTitle": "Connection Lost",
  "progress.timedOutSub": "The installation is still running in the background. You may close this dialog and check progress or logs later.",
  "progress.disconnectedSub": "Unable to reach supervisor. The task may still be running in the background; close and retry later.",
  "progress.viewingOwnedTitle": "Viewing owned install task…",
  "progress.viewingOwnedSub": "Reloading task status",
  "progress.viewingRunningTitle": "Viewing active install task…",
  "progress.runningTitle": "QwenPaw {action} v{version}…",
  "progress.viewingRunningSub": "Running independently on the NAS backend",
  "progress.runningSub": "Downloading via pip and rebuilding environment; the service will restart automatically",
  "progress.fetchingStatus": "Fetching live install status…",
  "progress.minimizedToast": "Moved to background. Check progress from the console.",
  "progress.readStatusFailed": "Unable to read install status: HTTP {status}",
  "progress.noRunningTask": "No install task is currently running",
  "progress.cannotConnect": "Cannot connect to backend: {error}",
  "progress.waitingPip": "Waiting for pip output…",
  "progress.phase.downloading": "Downloading dependencies…",
  "progress.phase.installing": "Installing kernel…",
  "progress.phase.starting": "Starting and verifying service…",
  "progress.phase.running": "Install task running…",
  "progress.unconfirmedTitle": "Unable to Confirm Task Result",
  "progress.unconfirmedNoReceipt": "The request may have been delivered and completed; close this dialog and check the current version and logs in the console.",
  "progress.unconfirmedMismatch": "The backend task does not match this operation. Close this dialog and inspect install logs in the console.",
  "progress.taskLabel": "Install task",
  "progress.failedTitle": "{action} Encountered an Issue",
  "progress.recoveryError": "Environment recovery incomplete: {error}",
  "progress.recoveryIncomplete": "Environment recovery incomplete; check logs and follow instructions",
  "progress.checkLogsNote": "Check logs to verify environment recovery status",
  "progress.doneTitle": "{action} Completed!",
  "progress.doneSub": "The selected version is ready and the QwenPaw background service is running",
  "progress.doneNoLogs": "Install task finished, no log output captured.",
  "progress.bgFailedToast": "{action} failed; open console for details",
  "progress.bgDoneToast": "{action} completed",

  // Common Modal & About
  "common.cancel": "Cancel",
  "common.confirm": "Confirm",
  "common.close": "Close",
  "common.promptTitle": "Confirmation",
  "common.inputLabel": "Input",
  "common.invalidInput": "Invalid input format. Please check and try again.",
  "common.opFailed": "Operation failed (HTTP {status})",

  "about.title": "About QwenPaw",
  "about.desc": "QwenPaw for fnOS is a third-party deployment wrapper for fnOS handling kernel installation, supervision, and operations; QwenPaw itself is installed on demand from the selected PyPI mirror.",
  "about.linksAria": "Project links",
  "about.upstreamTitle": "Upstream QwenPaw Repository",
  "about.wrapperTitle": "QwenPaw for fnOS Repository",
  "about.faqTitle": "Troubleshooting & FAQ",
  "about.faq1.q": "Cannot open Web Chat",
  "about.faq1.a": "Confirm the kernel is installed and running in Overview; if it still fails, inspect startup output, port, or gateway errors in Logs.",
  "about.faq2.q": "Install or upgrade failed",
  "about.faq2.a": "Check NAS network and DNS, then switch the download source in Overview → Deployment and retry. If a task is already running, open Install Task instead of submitting again.",
  "about.faq3.q": "Forgot QwenPaw admin password",
  "about.faq3.a": "While the kernel is running, open the console as an fnOS admin, go to Settings → Authentication, click Reset QwenPaw Admin Password, and sign in with your existing username and new password. No old password is needed; fnOS account, config, and chat history are unaffected.",
  "about.faq4.q": "Permission or runtime issues after upgrade",
  "about.faq4.a": "Keep runtime logs and user data intact—do not manually delete the runtime environment or shared data directory. Export logs, note the error timestamp and message, and report an issue.",

  // Backend API code messages
  "backend.operation_in_progress": "Another management operation is already in progress. Please try again shortly.",
  "backend.recovery_pending": "The previous transaction requires recovery before new actions can run.",
  "backend.install_busy": "An installation task is already in progress. Wait for it to finish and retry.",
  "backend.kernel_not_installed": "QwenPaw kernel is not installed. Select a version in Deployment first.",
  "backend.service_started": "QwenPaw started",
  "backend.service_already_running": "QwenPaw is already running",
  "backend.service_stopped": "QwenPaw stopped",
  "backend.service_restarted": "QwenPaw restarted",
  "backend.logs_cleared": "Logs cleared",
  "backend.install_started": "Installation started",
  "backend.kernel_uninstalled": "Kernel uninstalled; user config, memory, and admin credentials preserved",
  "backend.native_auth_uninitialized": "Native auth is not enabled or no admin account is registered yet",
  "backend.native_auth_mismatch": "Native account does not match the fnOS entry binding; no changes made",
  "backend.native_auth_unconfirmed": "Unable to confirm account record or write result; verify kernel status",
  "backend.native_auth_no_ack": "No confirmation received from kernel; try signing in with the new password first",
  "backend.mirror_saved": "Download source saved",
  "backend.port_in_use": "Port is already used by the kernel or supervisor",
  "backend.web_proxy_disabled": "Web proxy is not enabled",
  "backend.web_proxy_port_changed": "Web proxy port updated",
};

const dictionaries: Record<Locale, Record<MessageKey, string>> = {
  "zh-CN": zhCN,
  en,
};

export function normalizeLocale(value: unknown): Locale | null {
  if (typeof value !== "string") return null;
  const lower = value.trim().toLowerCase();
  if (!lower) return null;
  if (lower.startsWith("zh")) return "zh-CN";
  if (lower.startsWith("en")) return "en";
  return null;
}

export function detectBrowserLocale(languages?: readonly string[] | null, language?: string | null): Locale {
  const candidates: string[] = [];
  if (languages) {
    for (const item of languages) candidates.push(item);
  }
  if (language) candidates.push(language);
  for (const item of candidates) {
    const matched = normalizeLocale(item);
    if (matched) return matched;
  }
  return "zh-CN";
}

export function readStoredLocale(storage?: Pick<Storage, "getItem"> | null): Locale | null {
  try {
    return normalizeLocale(storage?.getItem(LOCALE_STORAGE_KEY) ?? null);
  } catch {
    return null;
  }
}

export function writeStoredLocale(locale: Locale | null, storage?: Pick<Storage, "setItem" | "removeItem"> | null): void {
  try {
    if (locale === null) storage?.removeItem(LOCALE_STORAGE_KEY);
    else storage?.setItem(LOCALE_STORAGE_KEY, locale);
  } catch {
    /* Storage may be blocked; keep the choice in memory for this page. */
  }
}

export function readLocaleMode(storage?: Pick<Storage, "getItem"> | null): LocaleMode {
  return readStoredLocale(storage) ?? "system";
}

export function resolveInitialLocale(
  storage?: Pick<Storage, "getItem"> | null,
  languages?: readonly string[] | null,
  language?: string | null,
): Locale {
  return readStoredLocale(storage) ?? detectBrowserLocale(languages, language);
}

export function formatMessage(template: string, params?: Record<string, string | number>): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) =>
    Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match,
  );
}

function safeLocaleStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try { return window.localStorage; } catch { return null; }
}

function browserDetectedLocale(): Locale {
  return detectBrowserLocale(
    typeof navigator !== "undefined" ? navigator.languages : null,
    typeof navigator !== "undefined" ? navigator.language : null,
  );
}

function initialLocaleState(): { mode: LocaleMode; locale: Locale } {
  if (typeof window === "undefined") return { mode: "system", locale: "zh-CN" };
  const stored = readStoredLocale(safeLocaleStorage());
  return stored ? { mode: stored, locale: stored } : { mode: "system", locale: browserDetectedLocale() };
}

const initial = initialLocaleState();
export const localeMode = ref<LocaleMode>(initial.mode);
export const currentLocale = ref<Locale>(initial.locale);

export function syncDocumentLocale(locale: Locale = currentLocale.value, mode: LocaleMode = localeMode.value): void {
  if (typeof document === "undefined") return;
  document.documentElement.lang = locale;
  document.documentElement.dataset.localeMode = mode;
}

export function setLocaleMode(mode: LocaleMode, persist = true): Locale {
  localeMode.value = mode;
  const resolved: Locale = mode === "system" ? browserDetectedLocale() : mode;
  currentLocale.value = resolved;
  if (persist) {
    writeStoredLocale(mode === "system" ? null : mode, safeLocaleStorage());
  }
  syncDocumentLocale(resolved, mode);
  return resolved;
}

export function setLocale(locale: Locale, persist = true): void {
  setLocaleMode(locale, persist);
}

export function toggleLocale(): Locale {
  const next: Locale = currentLocale.value === "zh-CN" ? "en" : "zh-CN";
  return setLocaleMode(next, true);
}

export function syncLocaleFromStorage(): void {
  if (typeof window === "undefined") return;
  const stored = readStoredLocale(safeLocaleStorage());
  localeMode.value = stored ?? "system";
  currentLocale.value = stored ?? browserDetectedLocale();
  syncDocumentLocale(currentLocale.value, localeMode.value);
}

export function syncLocaleFromSystem(): void {
  if (localeMode.value !== "system") return;
  currentLocale.value = browserDetectedLocale();
  syncDocumentLocale(currentLocale.value, localeMode.value);
}

export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const dict = dictionaries[currentLocale.value] ?? zhCN;
  return formatMessage(dict[key] ?? zhCN[key], params);
}

export function tFor(locale: Locale, key: MessageKey, params?: Record<string, string | number>): string {
  return formatMessage(dictionaries[locale][key], params);
}

const backendCodeKeys: Readonly<Record<string, MessageKey>> = {
  operation_in_progress: "backend.operation_in_progress",
  recovery_pending: "backend.recovery_pending",
  install_busy: "backend.install_busy",
  kernel_not_installed: "backend.kernel_not_installed",
  service_started: "backend.service_started",
  service_already_running: "backend.service_already_running",
  service_stopped: "backend.service_stopped",
  service_restarted: "backend.service_restarted",
  logs_cleared: "backend.logs_cleared",
  install_started: "backend.install_started",
  kernel_uninstalled: "backend.kernel_uninstalled",
  native_auth_uninitialized: "backend.native_auth_uninitialized",
  native_auth_mismatch: "backend.native_auth_mismatch",
  native_auth_unconfirmed: "backend.native_auth_unconfirmed",
  native_auth_no_ack: "backend.native_auth_no_ack",
  mirror_saved: "backend.mirror_saved",
  port_in_use: "backend.port_in_use",
  web_proxy_disabled: "backend.web_proxy_disabled",
  web_proxy_port_changed: "backend.web_proxy_port_changed",
};

export function localizeBackendMessage(code: unknown, fallback?: string, locale: Locale = currentLocale.value): string {
  if (typeof code === "string" && Object.prototype.hasOwnProperty.call(backendCodeKeys, code)) {
    return tFor(locale, backendCodeKeys[code]);
  }
  return fallback || "";
}
