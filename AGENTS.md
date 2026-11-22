# AGENTS.md

## 项目概述

QwenPaw for fnOS 是将 QwenPaw 部署到飞牛 NAS 的轻量外壳包，不是 QwenPaw 本体仓库。外壳负责安装、升级、配置、进程监管和运维控制台；QwenPaw 内核按需从 PyPI 安装。

核心架构主线：

`fnOS 统一网关 → Rust Gateway / Socket → QwenPaw Web`

fnOS 生命周期回调由薄入口转发至 Rust supervisor；管理控制台与 QwenPaw 主 Web 入口均经 Gateway。生产 CGI 已退役。

外壳与内核版本双轨：除用户已明确批准的 `1.3.3-1` 健康 2.3.0 → 2.3.0.post1 事务迁移外，外壳升级不得隐式升级、降级或重装内核。内核版本由版本钉记录，运行环境重建时按版本钉恢复。

## 当前状态与执行顺序

进行后续管理层工作前，先阅读 `docs/session-handoff-2026-11-04.md`、`docs/management-contract-inventory.md` 和 `docs/management-refactor-plan.md`，再核对当前工作树与相关源码。基线包含大量前序未提交实现和文档；必须按当前工作树判断，保留已有改动，不能只依据 HEAD。

用户可见文案只描述能核对的事实和操作后果。`recommended_version` 是既有 API 字段，表示下载源中与本包适配补丁匹配的版本；不要将它写成版本推荐、飞牛认证或稳定性保证。镜像可达性、安装耗时和数据恢复结果也不作保证。

开发版本以 `manifest` 为准；下列候选记录只对应各自的 FPK。管理台导航降级为单页时保留原生分区锚点，日志支持手动读取，不启用后台日志订阅。目录授权回调仅在结果回传成功后自动返回；清空日志仅在截断成功后报告成功。

**1.5.6-9 旧内核会话 Cookie 清理候选：** fnOS 网关挂载路径中，若 `/api/hub/pawapps/sessions` 的 DELETE 仅对版本钉精确为 `2.3.0` 或 `2.2.2.post1` 的实例返回 405，Gateway 按浏览器收到的 host-only `/app/qwenpaw/web/api/` Cookie 作用域，清除格式匹配的 `qwenpaw_app_<64位小写十六进制>` Cookie；原生成功、其他状态/版本/路径及鉴权失败均原样保留。只撤浏览器 Cookie，不撤销上游令牌。候选 `build/validation/legacy-session.a3uyqB/fpk/qwenpaw-1.5.6-9-x86.fpk`，SHA256 `fb13549a8f69a19f82182401a07fbfa78913ca7627b920ac06ae0f7a45d7ef7d`。隔离 Chromium 的三版首次安装、注册/登录、重启回滚和升级后刷新均验证通过；Rust 全量测试、严格 lint、Console payload 与前端类型检查通过。完整 GitHub CI 工作流未全部执行，fnOS 应用中心真机验收仍未放行。首次测试错误由本地隔离 wrapper 的 PID namespace session ID=0 引起，修正 wrapper 在隔离 PID namespace 内以 `setsid` 启动后全量 Rust 与严格 lint 通过；不修改生产进程身份校验。详情见 `docs/store-submission-1.6.0.md`。

**1.5.6-8 登录主题性能候选：** 默认 Console 在已有认证门禁放行后读取主题，匿名登录页使用本地明暗模式；不改变 Gateway 鉴权。已在隔离环境验证 `-7` → `-8` 的真实 post1 指纹重建。`-8` 的浏览器验收发现两种旧内核会话清理 405，该问题由 `-9` 定向修复；三版冷暖测量和原包摘要见 `docs/store-submission-1.6.0.md`。

**1.5.6-7 三版本兼容验收：** PR #8719 已适配至 `286c6556`，只为官方 `2.3.0.post1` 出货预构建载荷。当前外壳不继承历史 2.3.0 跨版本迁移。`2.3.0` 与 `2.2.2.post1` 用各自 wheel 的 Console。隔离验收及仍阻碍放行的 401/405 见 `docs/store-submission-1.6.0.md`，不能据此宣称全功能兼容或应用中心真机放行。

**1.5.5 iOS 长会话修复候选：** 在 1.5.4 已验证的移动头部、动态标题与上架材料基础上，同步当前 Console 工作树已有的四项长会话缓解：iOS 关闭逐气泡导航测量与 `content-visibility:auto`、历史恢复补分页标记、收窄输入区 DOM 监听。目标回归 54 项、生产构建、包内 Gateway 2 项、FPK 审计及双构建字节一致性通过。候选 `build/candidates/1.5.5-ios-long-chat/qwenpaw-1.5.5-x86.fpk`，SHA256 `ec4be05988f73ac11d190f5c6909776b45dacfbe5dfe3a28854ddc3423677a11`。真机 iOS 私有属性分支与长会话滚动仍须实机验收，不推进或打包 1.6。

**1.5.4 上架筹备（门禁未放行）：** 当前只使用 1.5.x，门禁全部通过前不得推进或打包 1.6；提前生成的 1.6 候选已作废删除。三态主题、中英文界面、后端提示本地化、文案统一、根图标规格修正和真实界面截图已纳入筹备，但真机发现的上游聊天页移动头部挤压及控制台原生标题不随页面变化已修复，并通过源码、隔离复测和最终 1.5.4 FPK 包级验证。最终候选 `build/candidates/1.5.4-mobile-title/qwenpaw-1.5.4-x86.fpk`，SHA256 `a7c9841b431348138b01f310e30509562743c71df9286dd07efaae261182fbec`，双构建字节一致；未执行飞牛应用中心验收，不推进或打包 1.6。申请材料和验收状态见 `docs/store-submission-1.6.0.md`。本地构建与隔离测试不替代最终 FPK 的飞牛应用中心真机验收或上架审核。

**1.5.4 本地验证基线：** 筹备候选 `build/candidates/1.5.4-store-prep/qwenpaw-1.5.4-x86.fpk`，SHA256 `baa15a7d7cbee03081bedb0c9bb6021a6cc887ee49ec83cc2b838d9927423bcf`。前端 191 项、Rust 428+13、严格 Clippy/lint、双浏览器 Gateway、真实反代端口、release API、7 个 SIGKILL 断点、平台回调、真实 venv、包内 Chromium 2 项、FPK 审计及双构建字节一致性通过；musl Gateway 26 项中 24 通过、2 项因隔离环境外部属主能力明确跳过。未执行飞牛应用中心验收。

**1.5.3-5 前端优化交付候选：** 同步主题色，完善日志筛选清除与自动滚底状态，修复原生安装弹窗焦点循环；包含版本弹层、移动布局、组件化、导航及授权回调优化。候选 `build/candidates/1.5.3-5-frontend-final/qwenpaw-1.5.3-5-x86.fpk`，SHA256 `1ae4d51e6d108b04344cde60fe55e451bb1db7be83546e76b80dc8dad8a3d59c`。前端 177 项、类型检查、生产构建和 FPK 审计通过；未执行飞牛应用中心验收。

**1.5.3-4 弹层与移动布局候选：** 版本列表打开时按可视边界定位，移动端下载源列表改为文档流展开，减少浮动遮挡；保留组件化、导航、授权回调及此前 UI 优化，并包含主题色同步、日志筛选清除、自动滚底状态及原生安装弹窗焦点循环修复。候选 `build/candidates/1.5.3-4-dropdown-ui/qwenpaw-1.5.3-4-x86.fpk`，SHA256 `474004b54fd6e81d6aa3711c2f0791457728f08d6cd47cada2c0a8583381563a`。前端 177 项、类型检查、生产构建和 FPK 审计通过；未执行飞牛应用中心验收。

**1.5.3-3 前端细节优化候选：** 优化版本列表的信息分组、键盘滚动留白和首行复位，收紧移动端卡片与日志布局并统一控件形状；保留组件化、导航和授权回调优化。候选 `build/candidates/1.5.3-3-ui-refinement/qwenpaw-1.5.3-3-x86.fpk`，SHA256 `efd4d8a6f10283a0534a30bb5ed7ada298f4ed5396712b3fd6a39246e838650d`。前端 177 项、类型检查、生产构建和 FPK 审计通过；未执行飞牛应用中心验收。

**1.5.3-2 最新前端优化候选：** 统一分区导航的 `pushState` 切换与故障降级，全屏 Web 改用宿主返回手势，授权回调页补齐品牌化双主题；保留组件化、UI 美化和管理交互。候选 `build/candidates/1.5.3-2-navigation-ui/qwenpaw-1.5.3-2-x86.fpk`，SHA256 `3a2afc4ccb60e5f8c0149db2457bf6cfa892b63593ebd4206ef1bae7e01c3048`。前端 177 项、类型检查、生产构建、FPK 审计及包内 Chromium 2 项通过；未执行真实 fnOS App 或应用中心验收。

**1.5.3-1 前端组件化候选：** 新增 `MirrorPicker`、`NativeLoginDialog`、`ResetPasswordDialog`、`PawIcon` 和 `dialogHelpers.ts`，收敛下载源、版本选择、确认框、关于页和品牌图标的重复逻辑；不改变管理 API、鉴权或安装事务。候选 `build/candidates/1.5.3-1-componentized-ui/qwenpaw-1.5.3-1-x86.fpk`，SHA256 `6f67dd895d6e95b22d78b798b95e767936888c75a931bd1b1bfef74792c1e4cc`。前端 177 项、类型检查、生产构建、FPK 审计及包内 Chromium 2 项通过；未执行飞牛应用中心验收。

**1.5.2-1 最新 UI 美化候选：** 对齐 QwenPaw 品牌橙与暖中性色，桌面侧栏加入品牌标识并优化卡片与按钮层次；移动端使用磨砂底栏和分组卡片，保留 DSH 日志阅读器。包含本轮安装引导、状态与关于页文案及 README 重整。候选 `build/candidates/1.5.2-1-ui-polish/qwenpaw-1.5.2-1-x86.fpk`，SHA256 `8806ffec84c5d5055f6f25020f9560f06eb6791de8d5ce37273106a7ad7ccb0c`。前端 177 项、类型检查、完整构建与 FPK 审计通过；未执行飞牛应用中心验收。

**1.5.1 DSH 日志阅读优化：** 移动端日志页按 DSH 的信息层级收敛为阅读器，导出和自动滚底直接可见，筛选、复制、实时刷新与清空收进原生展开区；浅色主题使用轻底等宽日志和可区分的级别语义色，暗色主题保持暗底。保留现有日志窗口、筛选、复制、导出、清空、订阅和移动触控契约。候选 `build/candidates/1.5.1-dsh-logs/qwenpaw-1.5.1-x86.fpk`，SHA256 `bb67ad95668693486c0b6e89f4b21bd4f8ce3cb068a8a25d2e579deef2928270`。前端 177 项、类型检查、生产构建、FPK 审计及包内 Chromium/WebKit 各 2 项通过；未执行 fnOS 应用中心验收。

**1.5.0 PR 跟进与管理台优化：** 跟进 PR #8719（e2cda8ff），在官方 2.3.0.post1 上仅新增 Desktop 子进程前缀隔离、托管 env CLI 前缀和 PawApp 路由上下文解析；独立 Data 插件请求体修复不属于 FPK 物料。管理台合入日志筛选/复制、长列表与生命周期优化，并完善键盘和移动触控交互。候选 `build/candidates/1.5.0-pr8719-ui/qwenpaw-1.5.0-x86.fpk`，SHA256 `72895c2a4b6cd3f567c58499b922a7afc3d83e94fa8b6e61dae21a480d46db5c`。上游新增回归 4 项、前端 177 项、隔离 Rust 428+13、严格 Clippy/lint、Chromium/WebKit Gateway 各 12 通过及 1 条件跳过、包内 Chromium 2 项和 FPK 审计通过；真实语音、独立 Data 插件和 fnOS 应用中心验收未执行。

**1.4.7-3 Voice 子路径修复：** 跟进 PR #8719（a890f6c2），保留 fnOS 自有入口、Cookie 与静态缓存实现，仅补充运行时前缀传递、bundled Voice ZIP 和协议模型。候选 `build/candidates/1.4.7-3-voice/qwenpaw-1.4.7-3-x86.fpk`，SHA256 `71f0093b27f85ea211c67d6067613b838823d952d6a6cb072891c93a285d7384`。Chromium / WebKit 包内完整 Gateway 各 13 项通过，Rust 428+13、严格 lint、FPK 审计通过；同绝对路径双构建字节一致。真实语音、Cloudflare/Twilio 与 fnOS 应用中心验收仍未执行。

**1.4.7-1 子路径与版本选择：** 2.3.0.post1 的本地客户端跟进上游 PR #8719（15614b27），Chrome 桥接的生成地址编码路径，不改变 Gateway 转发或鉴权。概览内单列表替代系统选择器；选中不发起安装。独立 Data 应用和 Chrome 扩展安装器不是 FPK 的物料。

**1.4.6 原生密码恢复：** 管理台配置内给已注册的单实例管理员设置新密码，轮换原生会话签名密钥。不删除账号、不改桌面绑定、不清除旧反代保护、不重启内核。只在匹配适配的受管内核经已核验的私有通道执行；旧清空接口退役，但启动仍需恢复历史未完成的重置事务。

**1.4.5-1 恢复安装修复：** 保留数据重装时，只有属主正确且平台产生的 var 根恰为 0770 才将该根收敛为 0750；不递归、不 chown，其他模式及升级仍 fail closed。候选 `build/candidates/1.4.5-1-retained-reinstall/qwenpaw-1.4.5-1-x86.fpk`，SHA256 `0c930f1ef444509b78d02144ea3760d69d2150743615d4474d88ad21269afac6`。实机以 0770 根恢复安装并自动运行 2.3.0.post1，原账号登录成功；随后删除数据卸载完成，应用目录无文件。不是完整生产发布放行。

**1.4.5 进程监控：** 首个管理快照不等 Python 元数据；版本探测不持进程状态锁。只有前台概览使用单条 `events?watch=metrics` 每秒采样，不用整页轮询代替资源监控；首个 CPU 差值未知时显示占位。运行时间两处共用本地时钟。1.4.5-1 起，未满 24 小时保留秒，满 24 小时按已过的整分钟显示，资源采样周期不变。停止核验与健康检查不得因降低面板延迟而缩短。

**1.4.4 Vue 管理台：** 四分区共用 `web/src/navigation.ts` 的当前视图；标题、导航选中态、分区显隐和日志订阅均从它派生。`#version` 仍定位概览的部署版本。配置和授权控件首次访问时才挂载，离开不销毁；导航故障的完整单页须挂载两区。不可延后处理飞牛授权回调。

**1.4.3-2 测试候选：** 在 PR #8719 的既有适配基础上补齐 Chrome Native Messaging 与 doctor 本地 URL，候选 `build/candidates/1.4.3-2-pr8719/qwenpaw-1.4.3-2-x86.fpk`，SHA256 `79c3eaa4c2aad9b48d36708c17f26a13580d1bd74ab9b2463b87b36aab05323c`。实机全新安装、桌面注册/登录/刷新、1138 个补丁摘要及保留数据卸载通过；恢复安装因平台将 var 改为 0770 而被门禁拒绝，删除数据卸载未执行。Data 与 Chrome 扩展安装器不在此 FPK 内；不是发布放行。

**1.4.3-1 测试候选：** 适配上游 PR #8719（f9bdc13）的子路径修复。应用中心拒绝同版本覆盖后，测试机通过应用中心从 1.4.3 正常升级；内核仍为 2.3.0.post1，实际 venv 的 1136 个补丁文件摘要全部匹配，带前缀的 index.html 及其入口资源已实测。Creator、Data 独立交付；此次升级不是完整插件验收或发布放行。

**1.4.3 工作树：** 新装及未设置独立密码的实例使用 QwenPaw 原账号。已设密码的升级必须保留旧保护；只允许飞牛管理侧在匹配受管内核验证原账号后显式停用，不再提供设置、更换或空密码清除接口。不改变飞牛入口既有身份绑定，不向独立反代授予本机或平台身份。以下旧版本的设置密码操作仅为历史记录。1.4.3 候选已在指定测试机经官方应用中心从 1.3.5-3 升级；桌面配置、本机与独立端口原账号登录通过。此前 WebKit 回归的测试夹具已改为真实 venv 路径解释器；隔离双浏览器通过不等于解除此前安装子进程登记门禁。

**1.4.2 基线：** 首页版本选择常显，一个原生选择框包含所有版本，发布时间排序与升级/降级判定分离。健康首页仍不自动向镜像查询；不做多版本共存或定时查源。面板安装可明确要求创建停机业务数据 tar，失败即中止，不替代异盘备份。1.4.1 的安装子进程登记发布门禁未解除。

**1.4.1 Vue 基线：** `web/src/App.vue` 是管理台唯一 Vue 根，四个分区、确认框、安装进度由预编译模板承载；生产包不向 `window` 导出管理操作。保留单一版本钉与事务安装，不把 DSH 的插件、专属账号库或通用 Shell 加入管理 API。日志分区在前台开启实时刷新时通过既有鉴权 SSE 的 `?watch=logs` 订阅文件变化，不向概览推送日志；15 秒轮询仍作兜底。管理台保留原四个分区；桌面侧栏与手机底栏共用导航。服务和部署版本同属概览，`#version` 只展开版本管理，不新增插件页或更改内核插件。1.4 开发已开始：原 Preact 动态渲染层改为 Vue 3 单文件组件（`web/src/views/`）；配置、授权与备份两页的交互也已迁入 Vue，未访问时不预读反代端口或目录列表（已完成的授权回调仍可主动恢复）。API、轮询与事务继续由 TypeScript 控制器负责，不在组件内重复发请求。`1.4.0-4` 起，独立反代密码可选，只有明确设置才要求验证；未设置走原生认证、不注入 owner，坏记录拒绝开放。NAS 本机服务端口保留原生认证；只出货 `default`，凭证不按端口命名，从 `1.4.0-3` 起，可信内网 HTTP 与外部 HTTPS 反代使用同一实例密码；TLS 由外部反代终止，并保留浏览器 Host（含端口）。不新增传输开关，不默认对外监听。飞牛 UDS 入口不使用反代密码。`1.4.0-1` 是过渡版本：显式核验原账号后持久化本实例的入口身份；只走核验双方进程的私有 UDS，不把原生 Bearer 下发浏览器。未迁移保留旧账号路径，已迁移不回退；后续取消旧登录的版本必须对保留数据先核验迁移记录，不能把缺失记录当新装。`1.4.0-2` 继续保留迁移功能；新设或更换独立反代密码为 6–12 个 Unicode 字符，历史长密码仍可登录。概览不展示、不读取、不轮询日志；只有日志分区启用日志定时器。以下候选记录仍指原包。

**上一代候选：** `1.3.5-3` 在正式升级及已有数据重装时先验证实际 var 目录的应用属主、组和权限（0700/0710/0740/0750），不 chmod/chown 已有根，不递归更改用户 ACL。预检失败只走 stderr 与平台临时错误日志，不得写入应用日志或停止服务。同一 FPK 的包内监督者通过普通用户 POSIX 临时卷 53/53 个平台场景，以及保留 fnOS 卷 ACL 的普通用户升级场景 26/26。Rust 415+13、严格 Clippy/lint、前端 151 项及包内页面 Chromium/WebKit 各 10/10 通过。证据在 `build/validation/1.3.5-3/`（分别见 `platform-nonroot-final.log`、`platform-fnos-nonroot-upgrade.log`）。可销毁测试机已通过飞牛应用中心从 1.2.7-7 升级到 1.3.5-3；实际内核与版本钉均为 2.3.0.post1，端口、日志级别和镜像文件摘要未变，var/home 为 0750，令牌及控制 Socket 为 0600。仅覆盖该机这条升级路径，不代表全量用户数据核验、管理端全部页面验证或正式发布。当时的向导错误地一概称内核版本不变，1.4.0-1 已更正。该历史候选不含 Vue 迁移。

**上一候选：** `1.3.5-2` 修复停止核验：读取 `/proc/PID/stat` 时若进程在打开和读取之间退出，Linux 可返回 ESRCH；仅此错误与 ENOENT 表示该条目已消失，权限或其他读取错误仍然阻断。该改动不等于已证实此前重启锁 EAGAIN 的原因。包内 strict、双浏览器和独立 glibc 压测通过；升级暂不放行。本机 fnOS 卷上 25 个升级回调用例有 15 个检测到 `var/local files` 权限改变；同卷不启动任何应用、仅对父目录 chmod 0750 即可重现。证据见 `build/validation/1.3.5-2/`。管理台沿用 `1.3.5-1`：桌面侧栏和手机底栏使用同一分区导航，部署版本独立成页；健康概览仅显示状态与两行日志，维护数据按页读取，配置/授权页不轮询日志；无 SSE 时保留状态轮询；恢复受阻时首页显示原因而非仅显示停止。保持 Rust 管理核心与 Preact/TypeScript，不引入第二套管理服务。

**上一修复候选：** `1.3.3-3` 未发布，用户已反馈本地升级成功。回滚前必须确认内核已停止，保留数据卸载后的版本钉不能因重装失败丢失。登记短命子进程时，只有确认该身份已退出才可接受 `/proc/PID/exe` 消失；仍存活但身份改变或不可核验时阻断。主会话在隔离环境中验证了 Rust 411+13 项、七个 SIGKILL 断点、前端 146 项和 post1 静态路由 31 项。真机升级和长稳未验证。

**上次候选（2026-11-18）：**当时修复候选为 `1.3.3-2`，以官方 `2.3.0.post1`（`ee58cb1a64f2b21e825c525f73dc44bc7941edea`）为内核适配基线，只携带 post1 完整 Console 载荷；健康 2.3.0 在外壳升级时经同一 venv 事务迁移到 post1，失败回滚并保留用户数据。镜像明确缺少 post1 时，仅本次迁移改用官方 PyPI，原镜像设置不变。登录页不再初始化聊天插件，管理日志按 50 行分组跳过屏外绘制。候选 `build/candidates/1.3.3-2-mirror-fallback-20261118/qwenpaw-1.3.3-2-x86.fpk`，33,512,246 字节，SHA256 `a72ab37a62f23a0440ecefa78371d48696d73f212c20437c594cb6ca2446e575`。post1 Console 693 个测试文件、8926 项通过（2 项预期失败），Rust 406+13、静态路由 31、平台 51 项及 FPK 审计通过；真实反代与 WebKit 隔离测量通过。本机已安装 `1.3.3-1` 并迁移到 post1，`1.3.3-2` 尚未部署；真实 iOS 与 24 小时稳定性仍未完成。

**历史性能与重装修复候选（2026-11-17）：**当时 manifest 版本为 `1.2.7-7`；候选 `build/candidates/1.2.7-7-orphan-reinstall-20261117/qwenpaw-1.2.7-7-x86.fpk`，稳定路径 `build/candidates/latest/qwenpaw-latest-x86.fpk`，23,773,553 字节，SHA256 `5218b74df94c2426ecc7e48f5bd57dff66cb86e5d1917d9af7f5b6eed93bf2e7`，包内 musl supervisor SHA256 `9d9ec7bc00f28b18b258dfc2704df2b84aecd7f974b308b2cf3df3a8e45cfcf6`。

Console 的历史候选以官方 v2.3.0 `0a21644e90d8ba37e20035b3bfb63880872cc2a2` 为基线，源码补丁 SHA256 `b0ba1b6e48df72f6c3f7188dbee23a33ce282ef605f2f46401c64356f92d8c2f`，动态预加载并发限制为 16，保留运行时 base-path 和浏览器 Cookie 清理。安装仅接受完整预构建载荷，不再回退下载其他版本源码。历史哈希与 `/proc/exe` 权限回退保持；已登记且与当前目标一致的 supervisor 直接复用，路径变化才停止旧实例；无登记时仅在存在历史安装证据且目标可执行文件一致时接管唯一 orphan，健康内核不因重装被停止，多实例或身份不明仍失败关闭。

历史候选验证：Console 683 个测试文件、8744 项通过（2 项预期失败）；后端相关 52 项、Rust 388+13 项及严格 Clippy 通过；前端 128 项与类型检查通过，Gateway E2E 9 通过、1 跳过；平台回调全量通过。preflight、build/FPK audit 通过，915 个补丁文件哈希一致。首屏 2.921 MiB raw、0.792 MiB Brotli。无旧 PID 登记的隔离重装 8.9 秒完成。远程应用中心已安装 `-7 running`，内核 2.3.0 正常运行且 recovery_required=false；主 Web 聊天输入与设置深链刷新通过，资源/API/页面错误均为 0，资源峰值 18（不写成严格 16）。这是测试候选，不是生产发布。

历史平台证据保留在 `docs/platform-validation-2026-11-03.md` 和 `docs/unified-gateway-migration-plan.md`。历史文档中的旧 FPK 路径和摘要只用于审计，不表示对应本地产物仍然保留或可在当前源码上复现。

## 协作与工作区边界

- 主会话编写代码、编写回归用例并执行核心测试。子代理只可用于探索、讨论、独立测试及复测，不代写或修改代码。
- 不在工作区外创建目录。运行测试前确认临时目录、依赖缓存、截图和构建输出的位置；无法隔离的检查不运行，也不报告通过。
- `patches/verified-version` 定义出货的 Console 适配基线。打包和默认检查必须使用这套载荷；安装页不得将镜像中最新的稳定版当作已验证版本。

## 工程边界

- 目标是让**自研生产管理层**退出 Python 和承载业务的 Shell。QwenPaw 本体只是外部受管程序，其 Python/Node 运行依赖不得因此删除。
- Rust 继续作为唯一系统管理核心；TypeScript 用于前端、客户端协议及合适的工程工具。不新增与 Rust 职责重叠的 Node 管理服务，不另建第二套安装、配置或恢复实现。
- fnOS 必需的 `cmd/*` 是定位原生程序、传递固定子命令并 `exec` 的薄入口；业务判断、配置写入、联网、安装编排和进程清扫统一进入 Rust。
- upgrade/uninstall 只执行包内 root 所有的 cmd sibling `qwenpaw-supervisor`，不可用即 fail closed `127`；不要恢复持久 helper 或 CGI/action 隐式回退。
- 构建、打包、CI 和测试驱动可保留 Python/Shell，但不得进入生产管理业务链路。
- 受管运行时只能通过集中的 `RuntimeAdapter` 以固定 argv 调用 venv、pip、QwenPaw CLI 及固定探针；不得用 `python -c` 藏 JSON、联网、版本排序或管理策略。
- 保持单 crate 和现有模块主线；HTTP 与私有控制通道共用传输无关管理核心，不做语法级逐行翻译或推倒重写。
- 最低 `os_min_version=1.2.0800` 已由用户批准；未经新的平台验证不得提高最低版本或公开内部管理端口。

管理层主线与 G0–G4 的已批准范围已完成，生产管理 Web 为 Gateway-only。fnOS logout 后旧会话仍有效是已知平台问题，用户确认不阻塞应用验收；不称会话已撤销或问题已修复。官方 PyPI 直连曾出现 15 分钟无输出超时；阿里云真实 `qwenpaw==2.2.2.post1` 安装/卸载通过，不等于官方 PyPI 直连通过。

平台验证要求：前置失败、端口不可用、就绪超时和未触发注入点必须使测试失败；健康端点由受管替身自身持有，停止后不得继续健康。测试须记录被测二进制来源与摘要，目录隔离不等于 `/proc` 隔离，发信号前登记并核验目标身份，无关哨兵进程必须存活。

## 目录与职责

- `manifest`：FPK 版本及应用元数据的单一事实来源。
- `cmd/`：fnOS 生命周期回调仅定位 `qwenpaw-supervisor` 并转发固定 `platform` 子命令；业务逻辑与安全停止由 Rust supervisor 实现。
- `supervisor/`：Rust 常驻监督者，负责内核生命周期、安装事务、配置事务、状态与 HTTP API。
- `supervisor/src/api/`：按状态、生命周期、安装、配置和日志划分的 API 模块。
- `supervisor/src/install/`：安装编排、venv 事务、pip、进程和 I/O；不要恢复成单体 `install.rs`。
- `app/ui/`：Gateway 桌面入口配置与图标；管理控制台和 `qwenpaw.main` 共用 Gateway/Socket。
- `web/`：Vue 3 单文件组件 + TypeScript；Vite 预编译模板，输出自包含的控制台 HTML。
- `wizard/`：安装、升级、配置和卸载向导。
- `config/`：fnOS 权限与资源配置。
- `scripts/`：构建门禁、回归测试、版本同步和审计工具。
- `tools/`：FPK 打包工具。
- `docs/`：设计、交接和平台证据；注意区分历史提案与当前契约。

以下均为生成物或构建目录，不要手工编辑或混入提交：`web/dist/`、`app/www/`、`app/bin/`、`build/`、`supervisor/target/`、`web/node_modules/`。清理旧候选和缓存时不得删除当前候选、源码测试或构建所需工具链。

## 开发命令

在仓库根目录执行，除非命令明确指定子目录：

```bash
# 前端依赖与构建
npm ci --prefix web
npm test --prefix web
npm run build --prefix web

# Rust 检查
cargo test --locked --manifest-path supervisor/Cargo.toml
cargo clippy --locked --manifest-path supervisor/Cargo.toml --all-targets -- -D warnings
cargo build --locked --release --manifest-path supervisor/Cargo.toml

# 一致性与综合门禁
bash scripts/preflight.sh
python3 scripts/gen-audit.py --check
STRICT_CI=1 bash scripts/lint.sh

# 完整 FPK 构建（含 preflight、release 构建、前端构建和包审计）
bash build.sh
```

`STRICT_CI=1` 下不允许因缺少 shellcheck、Node 或 Vite 而静默跳过门禁。优先使用 `npm ci` 和 Cargo `--locked`，不要无意更新锁文件。

以下测试需要已构建的 release 监督者：

```bash
bash scripts/test-venv-recovery.sh
python3 scripts/test-supervisor-api.py
```

它们应使用临时数据目录和隔离进程，不得操作真实部署。`test-venv-recovery.sh` 是 SIGKILL 崩溃恢复测试，不等同于真实断电验证。

前端还提供 `npm run typecheck --prefix web`、`npm run test:e2e --prefix web`、`npm run test:e2e:gateway --prefix web`、`npm run test:e2e:gateway:webkit --prefix web`；执行前确认依赖与环境就绪。命令存在不代表本轮已运行或通过。

## 跨层变更与 CI 门禁

- 大改按可独立验证的行为拆分。每一段先明确哪些行为要变、哪些必须保持，再同步修改实现和契约测试；先跑受影响的用例，合并前仍须跑完整 CI。局部通过不替代发布门禁。
- 测试断言对外承诺的结果，不把偶然的执行时序当契约。例如 `/status` 首次响应不等版本探测；安装、升级或回滚后，应在明确时限内读到与实际运行内核一致的版本。到期仍为空或版本不符必须失败，不能只检查请求成功。
- 浏览器故障注入须区分主动取消与真实请求失败。例如切页关闭 SSE 时，只允许**同一次请求**已收到 `200 text/event-stream` 且因预期操作取消；同 URL 的新订阅建连失败仍须报错。安装响应丢失及进度恢复单独断言，不按 URL 批量忽略错误。
- 编排 CI 时，只让有真实依赖的检查串行（如先构建 release 二进制，再测运行时）。Gateway E2E、静态检查及互不依赖的运行时回归应分别报告结果；拆分后仍为必过项，不得让一个失败遮住其他独立检查。
- 排查红灯先看各任务首个失败步骤、断言的期望值与实际值；同一断言在多个场景失败时先核对共同原因。报告区分失败、通过与未运行；被前序失败跳过的检查不算通过，也不算已证实的故障。修复后先复跑原失败用例，不以自动重试、无依据延长超时或跳过用例换取通过。

## 版本与修改约定

修改版本时以 `manifest` 为起点，并使用 `scripts/sync-version.py` 同步；用 `python3 scripts/sync-version.py --check` 验证。文件清单变化时同步更新 `AUDIT_REPORT.md`，再通过 `gen-audit.py --check` 验证。

自 2026-11-16 起按用户规则控制版本节奏：每个新功能单独推进一次基础版本，只递增最后一段，例如 `1.2.5 → 1.2.6`；该功能的修复和测试从 `1.2.6-1` 开始，后续依次 `-2`、`-3`。大版本或阶段性更新递增中间段并将末段归零，例如 `1.2.5 → 1.3.0`。不得在同一功能上无意义地连续增加修复号，也不得把多个无关新功能塞进同一版本；历史版本保持原样，不回改。

1.3 性能专项按后续用户补充规则执行：每完成一个阶段推进 `1.3.x`，该阶段修复与测试使用 `-x`；未完成门禁不得记为阶段完成。阶段范围与验收记录见 `docs/performance-governance-1.3.md`。

- 先运行 `git status --short`，保留用户和前序会话的未提交修改；不要用 reset、checkout 或 clean 覆盖它们。
- 遵循现有语言与模块边界：Rust 管系统状态和事务，Shell 管 fnOS 生命周期适配，前端使用 Vue 3 + TypeScript 和构建时模板编译，不引入另一套并行管理服务。
- Rust 修改至少运行对应 `cargo test --locked`；涉及所有 target 或准备交付时运行 Clippy 严格检查。
- 日志尾读共用 `supervisor/src/api/logs.rs`：从捕获的文件末尾按 8 KiB 分块回读，足够行数即停、最多 256 KiB，仅截断最终返回行；不得恢复整窗口逐行分配或读取期间无限追读追加内容。对应单元回归为 `api::logs::tests`，真实 release API 回归包含在 `scripts/test-supervisor-api.py`。
- Shell 修改运行 `bash -n` 和综合 lint。现有门禁脚本采用显式收集失败并统一退出的方式，不要盲目加 `set -e` 破坏该模式。
- 前端纯逻辑测试使用 Node 内置 test runner，位于 `web/test/`；前端源码是唯一事实来源，不直接修改编译后的 HTML。
- 新增测试应覆盖真实行为与失败路径，不能只验证 import、编译或固定成功替身。涉及改动时运行现有测试体系，不另建无关测试框架。
- 不自动 commit、push、tag、发布或部署；不执行真实卸载、删除用户数据或修改真实 NAS 服务。

## 必须保持的运行时不变量

### 事务与恢复

- 内核安装先写独立暂存 venv；健康检查、精确版本校验和版本钉写入全部成功后才提交。
- 失败时先停止新实例，再回滚环境和版本钉，最后恢复旧服务；安全收尾完成前不得释放安装互斥。
- 已提交与未提交的恢复分支必须明确。恢复失败要保留证据并阻断自动启动和进一步变更，不得假报成功。
- 端口、日志级别、镜像和桌面入口变更必须走统一事务；状态查询不得写配置。
- 不擅自改变 QwenPaw 用户数据格式；程序环境回滚不等于用户数据回滚，彻底删除不可宣称可回滚。

### 进程与并发

- 启停、重启、安装、卸载、配置变更和版本钉修改必须互斥，冲突如实返回 409。
- 停止内核需覆盖整个进程组及后代；监督者停止期间禁止新内核启动，并等待在途事务安全收尾。
- 对 PID 发信号前必须核对进程身份和 starttime，不能信任陈旧 PID 文件。
- 锁 FD 不得泄漏给后台服务或受管进程；遵守固定锁顺序和有界等待。
- 安装任务归属由受理序号与监督者实例共同确认；响应丢失不能猜测成功。

### 安全与平台兼容

- 外部输入不得拼接进 Shell；pip 等外部命令使用参数数组/argv 和严格白名单。
- 所有删除必须限制在 fnOS `TRIM_*` 路径约束内，禁止硬编码 `/vol*` 全局删除。
- Gateway 管理写操作须保留 CSRF 防护、方法约束和未知路由 404；上游代理请求不得错误套用管理系统 CSRF，须按当前代理契约透传所需授权与响应。
- `supervisor-token`、版本钉等敏感或事务文件保持既有权限与原子写语义。
- 打包采用白名单 staging；构建失败不得复用陈旧二进制冒充新产物，FPK 必须通过内容和执行位审计。
- fnOS 生命周期入口、Gateway 配置和构建脚本的可执行位属于发布契约；生产 CGI 已退役，不恢复 CGI/action 隐式回退。

## 文档使用与交付

- `README.md`：用户功能、安装升级方式和当前候选。
- `docs/management-contract-inventory.md`：当前管理契约，管理层改动前先阅读。
- `docs/management-refactor-plan.md`：整改架构、事务设计、测试矩阵和发布门禁。
- `docs/session-handoff-2026-11-04.md`：当前交接与下一步验收状态。
- `docs/unified-gateway-migration-plan.md`：Gateway 架构、G0–G4 迁移记录和回退纪律。
- `docs/platform-validation-2026-11-03.md`：平台实测证据、最低版本和剩余未验证项。
- `docs/backend-design.md`：历史提案，不作为当前实现依据。

交付时明确列出实际运行的检查、候选产物以及真机、网络或兼容性未验证项。不要依据旧文档、历史候选或单次单元测试宣称整个 FPK 已在 fnOS 生产环境验证。完整删除用户数据不可回滚。
